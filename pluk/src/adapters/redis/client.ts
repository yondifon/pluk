// Redis client built on Bun's native RedisClient (bun:redis). Lazy-connects on
// first command and auto-reconnects. Bun exposes get/set/del/exists/expire/ttl/incr
// as methods but NOT keys/scan/type/info — those go through `client.send(CMD, args)`.
// Reference: https://bun.sh/docs/api/redis (Redis 7.2+)
//
// Connections (and any SSH tunnel) are cached per (session, integration) and reused
// across a session's tool calls, then torn down on session close — same lifecycle
// as the SSH command adapter and the DB driver pool. This keeps a tunnel's agent
// confirm (1Password) to once per session and avoids leaking listeners.

import { RedisClient } from "bun";
import type { Integration } from "../../store/integrations.js";
import { onSessionClose } from "../../mcp/pool.js";
import { openSSHTunnel, type Tunnel } from "../../db/ssh.js";

const IDLE_MS = 5 * 60 * 1000;

interface SSHParams {
  host: string;
  port: number;
  user: string;
  authType: "agent" | "key" | "password";
  keyPath?: string;
  passphrase?: string;
}

export interface RedisCfg {
  /** Explicit connection URL (managed providers); wins over host/port. No tunnel. */
  url?: string;
  host: string;
  port: number;
  db: number;
  tls: boolean;
  password: string;
  /** Present when the integration tunnels Redis over SSH. */
  ssh?: SSHParams;
}

export function redisConfig(conn: Integration): RedisCfg {
  const c = conn.config;
  const useSsh = c.use_ssh === true || c.use_ssh === "true";
  const ssh: SSHParams | undefined = useSsh && c.ssh_host
    ? {
        host: String(c.ssh_host),
        port: Number(c.ssh_port ?? 22),
        user: String(c.ssh_user ?? ""),
        authType: String(c.ssh_auth_type ?? "agent") as SSHParams["authType"],
        keyPath: c.ssh_key_path ? String(c.ssh_key_path) : undefined,
        passphrase: c.ssh_password ? String(c.ssh_password) : undefined,
      }
    : undefined;

  // A pre-built URL (e.g. Upstash) is used directly — but only without a tunnel,
  // which needs an explicit host/port to forward to.
  const explicit = String(c.url ?? "").trim();
  if (explicit && !ssh) {
    return { url: explicit, host: "", port: 0, db: 0, tls: false, password: "" };
  }

  const host = String(c.host ?? "").trim();
  if (!host) throw new Error("Redis host is missing. Set it in the integration config.");
  return {
    host,
    port: Number(c.port ?? 6379),
    db: Number(c.db ?? 0),
    tls: c.tls === true || c.tls === "true",
    password: String(c.password ?? ""),
    ssh,
  };
}

export function buildUrl(scheme: "redis" | "rediss", host: string, port: number, db: number, password: string): string {
  const auth = password ? `:${encodeURIComponent(password)}@` : "";
  return `${scheme}://${auth}${host}:${port}/${db}`;
}

interface Resource {
  client: RedisClient;
  tunnel?: Tunnel;
}

/** Open a client (and an SSH tunnel if configured). The caller owns teardown. */
async function open(cfg: RedisCfg, onFatal?: () => void): Promise<Resource> {
  if (cfg.url && !cfg.ssh) return { client: new RedisClient(cfg.url) };

  if (cfg.ssh) {
    const tunnel = await openSSHTunnel(
      {
        host: cfg.ssh.host,
        port: cfg.ssh.port,
        user: cfg.ssh.user,
        authType: cfg.ssh.authType,
        keyPath: cfg.ssh.keyPath,
        passphrase: cfg.ssh.passphrase,
        remoteHost: cfg.host,
        remotePort: cfg.port,
      },
      onFatal,
    );
    // The local hop to the forwarded port is plaintext; Redis AUTH still applies.
    const url = buildUrl("redis", "127.0.0.1", tunnel.localPort, cfg.db, cfg.password);
    return { client: new RedisClient(url), tunnel };
  }

  const url = buildUrl(cfg.tls ? "rediss" : "redis", cfg.host, cfg.port, cfg.db, cfg.password);
  return { client: new RedisClient(url) };
}

// ── Session-scoped connection cache ────────────────────────────────────────────

interface Entry {
  resource: Promise<Resource>;
  idleTimer: ReturnType<typeof setTimeout>;
}

const pool = new Map<string, Entry>();

function key(sessionId: string, integrationId: string): string {
  return `${sessionId}::${integrationId}`;
}

function evictByKey(k: string): void {
  const entry = pool.get(k);
  if (!entry) return;
  pool.delete(k);
  clearTimeout(entry.idleTimer);
  entry.resource.then(({ client, tunnel }) => { client.close(); tunnel?.close(); }).catch(() => {});
}

function getResource(sessionId: string, conn: Integration): Promise<Resource> {
  const k = key(sessionId, conn.id);
  const existing = pool.get(k);
  if (existing) {
    clearTimeout(existing.idleTimer);
    existing.idleTimer = setTimeout(() => evictByKey(k), IDLE_MS);
    return existing.resource;
  }
  // Self-heal: a dropped SSH tunnel evicts the entry so the next call rebuilds it.
  const resource = open(redisConfig(conn), () => { if (pool.get(k) === entry) evictByKey(k); });
  const entry: Entry = { resource, idleTimer: setTimeout(() => evictByKey(k), IDLE_MS) };
  pool.set(k, entry);
  resource.catch(() => { if (pool.get(k) === entry) { clearTimeout(entry.idleTimer); pool.delete(k); } });
  return resource;
}

/** Close all cached Redis connections/tunnels for a session (on session close). */
export function closeSessionClients(sessionId: string): void {
  for (const k of [...pool.keys()]) {
    if (k.startsWith(`${sessionId}::`)) evictByKey(k);
  }
}

// Tie Redis connection + tunnel cleanup to the shared session lifecycle.
onSessionClose(closeSessionClients);

/** A lazy, session-scoped handle to the connection. Tools await `get()`; the tunnel
 *  (if any) is opened on first use and reused across the session's calls. */
export interface RedisAccessor {
  get(): Promise<RedisClient>;
}

export function redisAccessor(conn: Integration, sessionIdRef: { value: string }): RedisAccessor {
  return { get: async () => (await getResource(sessionIdRef.value, conn)).client };
}

/** Run a command Bun doesn't expose as a method (SCAN/KEYS/TYPE/INFO). */
export function raw(client: RedisClient, cmd: string, args: (string | number)[]): Promise<unknown> {
  return client.send(cmd, args.map(String));
}

/** One-off connect + PING for connection tests (no caching; tears down after). */
export async function testRedis(conn: Integration): Promise<void> {
  const { client, tunnel } = await open(redisConfig(conn));
  try {
    await client.send("PING", []);
  } finally {
    client.close();
    tunnel?.close();
  }
}
