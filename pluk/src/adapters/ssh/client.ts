import { Client, utils as sshUtils } from "ssh2";
import type { ConnectConfig } from "ssh2";
import { createServer, type Server } from "net";
import { Duplex } from "stream";
import { readFileSync, existsSync } from "fs";
import { homedir, userInfo } from "os";
import type { Integration } from "../../store/integrations.js";
import { onSessionClose } from "../../mcp/pool.js";
import {
  expandHome,
  parseSSHConfig,
  expandProxyCommand,
  spawnProxySocket,
  resolveAgentSocket,
} from "../../ssh/config.js";
import type { SSHConfigEntry } from "../../ssh/config.js";

// Remote command execution over SSH for the ssh adapter. Connections are cached
// per (session, integration) and reused across tool calls, so an agent-confirm
// (1Password) happens once per session rather than once per command — same idea
// as the DB driver pool, kept self-contained here.

const READY_TIMEOUT_MS = 180_000; // matches the tunnel: room for a 1Password confirm
const IDLE_MS = 5 * 60 * 1000;
const DEFAULT_EXEC_TIMEOUT_MS = 60_000;

export interface ExecResult {
  stdout: string;
  stderr: string;
  code: number | null;
  truncated?: boolean;
}

// Cap captured output so a runaway command (huge file, verbose logs) can't grow
// the buffer without bound. The remote stream is closed once the cap is hit.
const MAX_OUTPUT_BYTES = 1_000_000;

// A command-level timeout means the *channel* gave up, not that the SSH
// connection died — so the cached connection (and its agent auth) is still good
// and must not be evicted. Tag the error so the caller can tell them apart.
class CommandTimeoutError extends Error {}

interface SSHParams {
  host: string;
  port: number;
  user: string;
  authType: "agent" | "key" | "password";
  keyPath?: string;
  password?: string;
}

function params(conn: Integration): SSHParams {
  const c = conn.config;
  return {
    host: String(c.host ?? ""),
    port: Number(c.port ?? 22),
    user: String(c.user ?? "") || userInfo().username,
    authType: (String(c.auth_type ?? "agent") as SSHParams["authType"]),
    keyPath: c.key_path ? String(c.key_path) : undefined,
    password: c.password ? String(c.password) : undefined,
  };
}

// One entry in the ssh2 authHandler list. We offer these in order until one
// authenticates (mirrors how OpenSSH walks agent + identity files).
type AuthMethod =
  | { type: "none"; username: string }
  | { type: "agent"; username: string; agent: string }
  | { type: "publickey"; username: string; key: Buffer; passphrase?: string };

// Default identity files OpenSSH would try, plus the connection's explicit key
// and any IdentityFile from ~/.ssh/config — in preference order.
function keyFileCandidates(p: SSHParams, sshConfig: SSHConfigEntry): string[] {
  const all = [
    p.keyPath ? expandHome(p.keyPath) : null,
    sshConfig.identityFile ?? null,
    `${homedir()}/.ssh/id_ed25519`,
    `${homedir()}/.ssh/id_rsa`,
    `${homedir()}/.ssh/id_ecdsa`,
  ].filter((x): x is string => x !== null);
  return [...new Set(all)];
}

// Read a private key file and return its bytes only if ssh2 can parse it with
// the given passphrase; otherwise null (missing, unreadable, or wrong passphrase).
function parseableKey(path: string, passphrase?: string): Buffer | null {
  if (!existsSync(path)) return null;
  let data: Buffer;
  try { data = readFileSync(path); } catch { return null; }
  const parsed = sshUtils.parseKey(data, passphrase ?? "");
  const ok = Array.isArray(parsed) ? parsed.length > 0 : !(parsed instanceof Error);
  return ok ? data : null;
}

// Build a connected ssh2 Client honoring ~/.ssh/config (HostName/User/Port/
// IdentityFile/IdentityAgent/ProxyCommand). Mirrors the DB tunnel's ssh2 path so
// agent, key, password, and proxied hosts (e.g. Cloudflare Access) all work.
function connect(p: SSHParams): Promise<Client> {
  return new Promise((resolve, reject) => {
    if (!p.host) return reject(new Error("SSH host is missing. Set it in the integration config."));

    const sshConfig = parseSSHConfig(p.host);
    const host = sshConfig.hostName ?? p.host;
    const port = sshConfig.port ?? p.port;
    const username = p.user || sshConfig.user || userInfo().username;

    const client = new Client();
    let settled = false;
    let proxySock: Duplex | undefined;
    const fail = (err: Error) => {
      if (settled) return;
      settled = true;
      clearTimeout(connectTimer);
      proxySock?.destroy();
      client.end();
      reject(err);
    };

    // Backstop for a proxy socket (e.g. cloudflared) that stalls without ever
    // emitting `ready` or `error` — ssh2's readyTimeout only covers the handshake
    // once the transport is up, so a dead `sock` would hang this Promise forever.
    // A touch beyond readyTimeout so a legit slow 1Password confirm still wins.
    const connectTimer = setTimeout(() => {
      fail(new Error(`Couldn't reach ${host}:${port} within ${Math.round(READY_TIMEOUT_MS / 1000)}s — check the host, port, and any SSH proxy (cloudflared).`));
    }, READY_TIMEOUT_MS + 10_000);

    client.on("ready", () => { if (!settled) { settled = true; clearTimeout(connectTimer); resolve(client); } });
    client.on("error", (err) => fail(err));

    const cfg: ConnectConfig = {
      host,
      port,
      username,
      // Room for an interactive agent confirm (1Password) — same budget the tunnel uses.
      readyTimeout: READY_TIMEOUT_MS,
      keepaliveInterval: 30_000,
      keepaliveCountMax: 3,
    };

    // Route through ProxyCommand if the host's ssh config defines one.
    if (sshConfig.proxyCommand) {
      const cmd = expandProxyCommand(sshConfig.proxyCommand, host, port, username);
      proxySock = spawnProxySocket(cmd);
      cfg.sock = proxySock;
    }

    if (p.authType === "password") {
      cfg.password = p.password ?? "";
      cfg.tryKeyboard = true;
      client.on("keyboard-interactive", (_n, _i, _l, prompts, finish) => finish(prompts.map(() => p.password ?? "")));
    } else {
      // publickey: offer the agent AND the default on-disk identity files, just
      // like OpenSSH. ssh2 on its own only tries agent keys when `agent` is set,
      // so a host that accepts only a disk key (e.g. ~/.ssh/id_rsa not loaded in
      // the 1Password agent) fails here even though `ssh` succeeds. An authHandler
      // list makes ssh2 try every method in turn until one authenticates. In
      // "agent" mode the agent leads; in "key" mode the configured key leads.
      const agent = resolveAgentSocket(p.host);
      const keys = keyFileCandidates(p, sshConfig)
        .map((path) => parseableKey(path, p.password))
        .filter((k): k is Buffer => k !== null);

      const methods: AuthMethod[] = [{ type: "none", username }];
      const agentMethod: AuthMethod | null = agent ? { type: "agent", username, agent } : null;
      if (agentMethod && p.authType === "agent") methods.push(agentMethod);
      for (const key of keys) methods.push({ type: "publickey", username, key, passphrase: p.password });
      if (agentMethod && p.authType !== "agent") methods.push(agentMethod);

      if (methods.length === 1) {
        return fail(new Error("No SSH agent or usable private key found. Add a key in the connection settings or load one into your agent."));
      }
      cfg.authHandler = methods as ConnectConfig["authHandler"];
    }

    client.connect(cfg);
  });
}

function execOnce(client: Client, command: string, timeoutMs: number): Promise<ExecResult> {
  return new Promise((resolve, reject) => {
    client.exec(command, (err, stream) => {
      if (err) return reject(err);
      let stdout = "";
      let stderr = "";
      let captured = 0;
      let truncated = false;
      const append = (buf: string, d: Buffer): string => {
        if (truncated) return buf;
        const remaining = MAX_OUTPUT_BYTES - captured;
        const chunk = d.length > remaining ? d.subarray(0, remaining) : d;
        captured += chunk.length;
        if (captured >= MAX_OUTPUT_BYTES) { truncated = true; stream.close(); }
        return buf + chunk.toString();
      };
      const timer = setTimeout(() => {
        stream.close();
        reject(new CommandTimeoutError(`Command timed out after ${Math.round(timeoutMs / 1000)}s`));
      }, timeoutMs);
      stream.on("data", (d: Buffer) => { stdout = append(stdout, d); });
      stream.stderr.on("data", (d: Buffer) => { stderr = append(stderr, d); });
      stream.on("close", (code: number | null) => {
        clearTimeout(timer);
        resolve({ stdout, stderr, code, truncated });
      });
      stream.on("error", (e: Error) => { clearTimeout(timer); reject(e); });
    });
  });
}

// ── Session-scoped connection cache ────────────────────────────────────────────

// A live `ssh -L` local port forward: a local listener whose connections are
// tunneled to remoteHost:remotePort through the session's SSH connection.
interface Forward {
  id: string;
  remoteHost: string;
  remotePort: number;
  localPort: number;
  server: Server;
}

interface Entry {
  client: Promise<Client>;
  // null while the connection is pinned open by one or more active forwards.
  idleTimer: ReturnType<typeof setTimeout> | null;
  forwards: Map<string, Forward>;
}

const pool = new Map<string, Entry>();

function key(sessionId: string, integrationId: string): string {
  return `${sessionId}::${integrationId}`;
}

// (Re)arm idle eviction. Active forwards pin the connection open — an idle
// command stream shouldn't tear down a tunnel the user is still relying on.
function armIdle(k: string): void {
  const entry = pool.get(k);
  if (!entry) return;
  if (entry.idleTimer) { clearTimeout(entry.idleTimer); entry.idleTimer = null; }
  if (entry.forwards.size > 0) return;
  entry.idleTimer = setTimeout(() => evictByKey(k), IDLE_MS);
}

function evictByKey(k: string): void {
  const entry = pool.get(k);
  if (!entry) return;
  pool.delete(k);
  if (entry.idleTimer) clearTimeout(entry.idleTimer);
  for (const fwd of entry.forwards.values()) fwd.server.close();
  entry.forwards.clear();
  entry.client.then((c) => c.end()).catch(() => {});
}

async function getClient(sessionId: string, conn: Integration): Promise<Client> {
  const k = key(sessionId, conn.id);
  const existing = pool.get(k);
  if (existing) {
    armIdle(k);
    return existing.client;
  }
  const client = connect(params(conn));
  const entry: Entry = { client, idleTimer: null, forwards: new Map() };
  pool.set(k, entry);
  armIdle(k);
  // Rebuild on a dropped connection; drop the entry on a failed setup.
  client.then((c) => c.on("close", () => { if (pool.get(k) === entry) evictByKey(k); }))
    .catch(() => { if (pool.get(k) === entry) { if (entry.idleTimer) clearTimeout(entry.idleTimer); pool.delete(k); } });
  return client;
}

/** Run a command on the session's cached SSH connection. */
export async function runCommand(
  sessionId: string,
  conn: Integration,
  command: string,
  timeoutMs = DEFAULT_EXEC_TIMEOUT_MS,
): Promise<ExecResult> {
  const k = key(sessionId, conn.id);
  try {
    const client = await getClient(sessionId, conn);
    return await execOnce(client, command, timeoutMs);
  } catch (err) {
    // A command timeout leaves the SSH connection healthy — keep it so the next
    // call doesn't trigger a fresh agent (1Password) confirm. Any other failure
    // may mean a dead connection, so evict and reconnect next time.
    if (!(err instanceof CommandTimeoutError)) evictByKey(k);
    throw err;
  }
}

/** Close all cached SSH connections for a session (called on session close). */
export function closeSessionClients(sessionId: string): void {
  for (const k of [...pool.keys()]) {
    if (k.startsWith(`${sessionId}::`)) evictByKey(k);
  }
}

// Tie SSH connection cleanup to the shared session lifecycle (close/reload).
onSessionClose(closeSessionClients);

// ── Local port forwarding (ssh -L) ─────────────────────────────────────────────

export interface ForwardInfo {
  id: string;
  remoteHost: string;
  remotePort: number;
  localPort: number;
}

function forwardInfo(f: Forward): ForwardInfo {
  return { id: f.id, remoteHost: f.remoteHost, remotePort: f.remotePort, localPort: f.localPort };
}

// Open a local TCP listener that tunnels each connection to remoteHost:remotePort
// through the given SSH client — the in-process equivalent of `ssh -L`.
function listenForward(client: Client, remoteHost: string, remotePort: number, localPort: number): Promise<Server> {
  return new Promise((resolve, reject) => {
    const server = createServer((socket) => {
      // forwardOut takes a round-trip to open the channel. A client that speaks
      // first (HTTP, Postgres, Redis) can send before it's ready — buffer those
      // early bytes and replay them, or they're silently dropped.
      const early: Buffer[] = [];
      const collect = (d: Buffer) => early.push(d);
      socket.on("data", collect);
      client.forwardOut("127.0.0.1", 0, remoteHost, remotePort, (err, channel) => {
        if (err) { socket.destroy(); return; }
        socket.removeListener("data", collect);
        for (const chunk of early) channel.write(chunk);
        socket.pipe(channel);
        channel.pipe(socket);
        socket.on("close", () => channel.destroy());
        channel.on("close", () => socket.destroy());
        socket.on("error", () => channel.destroy());
        channel.on("error", () => socket.destroy());
      });
    });
    server.once("error", reject);
    server.listen(localPort, "127.0.0.1", () => {
      server.removeListener("error", reject);
      server.on("error", () => {}); // ignore late per-listener errors
      resolve(server);
    });
  });
}

/**
 * Open an `ssh -L` local port forward over the session's cached SSH connection,
 * so localhost:<localPort> on this machine reaches remoteHost:remotePort from the
 * remote side. Idempotent per remote target; the forward lives until it is closed
 * or the session ends. Omit `requestedLocalPort` to auto-assign a free port.
 */
export async function openForward(
  sessionId: string,
  conn: Integration,
  remoteHost: string,
  remotePort: number,
  requestedLocalPort?: number,
): Promise<ForwardInfo> {
  const k = key(sessionId, conn.id);
  const client = await getClient(sessionId, conn);
  const entry = pool.get(k);
  if (!entry) throw new Error("SSH connection closed before the forward could open.");

  const id = `${remoteHost}:${remotePort}`;
  const existing = entry.forwards.get(id);
  if (existing) return forwardInfo(existing);

  let server: Server;
  try {
    server = await listenForward(client, remoteHost, remotePort, requestedLocalPort ?? 0);
  } catch (err) {
    if ((err as { code?: string }).code === "EADDRINUSE") {
      throw new Error(`Local port ${requestedLocalPort} is already in use. Pick another local_port or omit it to auto-assign.`);
    }
    throw err;
  }

  const addr = server.address();
  const localPort = typeof addr === "object" && addr ? addr.port : (requestedLocalPort ?? 0);
  const fwd: Forward = { id, remoteHost, remotePort, localPort, server };
  entry.forwards.set(id, fwd);
  armIdle(k); // pin the connection open while this forward is alive
  return forwardInfo(fwd);
}

/** List the open local port forwards for this session's connection. */
export function listForwards(sessionId: string, conn: Integration): ForwardInfo[] {
  const entry = pool.get(key(sessionId, conn.id));
  if (!entry) return [];
  return [...entry.forwards.values()].map(forwardInfo);
}

/** Close one forward by id (`remoteHost:remotePort`). Returns false if unknown. */
export function closeForward(sessionId: string, conn: Integration, id: string): boolean {
  const k = key(sessionId, conn.id);
  const entry = pool.get(k);
  const fwd = entry?.forwards.get(id);
  if (!entry || !fwd) return false;
  fwd.server.close();
  entry.forwards.delete(id);
  armIdle(k); // resume idle eviction if that was the last forward
  return true;
}

/** One-off connect + command for connection tests (no caching). */
export async function testCommand(conn: Integration): Promise<void> {
  const client = await connect(params(conn));
  try {
    await execOnce(client, "echo pluk-ok", 15_000);
  } finally {
    client.end();
  }
}
