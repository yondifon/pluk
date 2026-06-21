import { Client, utils as sshUtils } from "ssh2";
import type { ConnectConfig } from "ssh2";
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

interface Entry {
  client: Promise<Client>;
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
  entry.client.then((c) => c.end()).catch(() => {});
}

async function getClient(sessionId: string, conn: Integration): Promise<Client> {
  const k = key(sessionId, conn.id);
  const existing = pool.get(k);
  if (existing) {
    clearTimeout(existing.idleTimer);
    existing.idleTimer = setTimeout(() => evictByKey(k), IDLE_MS);
    return existing.client;
  }
  const client = connect(params(conn));
  const entry: Entry = { client, idleTimer: setTimeout(() => evictByKey(k), IDLE_MS) };
  pool.set(k, entry);
  // Rebuild on a dropped connection; drop the entry on a failed setup.
  client.then((c) => c.on("close", () => { if (pool.get(k) === entry) evictByKey(k); }))
    .catch(() => { if (pool.get(k) === entry) { clearTimeout(entry.idleTimer); pool.delete(k); } });
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

/** One-off connect + command for connection tests (no caching). */
export async function testCommand(conn: Integration): Promise<void> {
  const client = await connect(params(conn));
  try {
    await execOnce(client, "echo pluk-ok", 15_000);
  } finally {
    client.end();
  }
}
