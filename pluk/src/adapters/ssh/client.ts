import { Client } from "ssh2";
import { createServer, type Server } from "net";
import { userInfo } from "os";
import type { Integration } from "../../store/integrations.js";
import { onOwnerClose } from "../../mcp/pool.js";
import { connectSSH, evictSharedSSHClient, getSharedSSHClient, type SSHParams } from "../../ssh/client.js";
import { isSshPending, withSshApprovalRetry } from "../../ssh/pending.js";

// Remote command execution over SSH for the ssh adapter. Connections are cached
// per (owner, integration) and reused across tool calls, so an agent-confirm
// (1Password) happens once per owner rather than once per command — same idea
// as the DB driver pool, kept self-contained here.

const IDLE_MS = 5 * 60 * 1000;
const DEFAULT_EXEC_TIMEOUT_MS = 60_000;
export const MAX_COMMAND_TIMEOUT_S = 600;

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
export class CommandTimeoutError extends Error {}

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
        // SIGKILL via sshd first, then close the channel — the remote process
        // must not keep running on the host after the caller's limit.
        stream.signal("KILL");
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

// ── Owner-scoped connection cache ────────────────────────────────────────────

// A live `ssh -L` local port forward: a local listener whose connections are
// tunneled to remoteHost:remotePort through the owner's SSH connection.
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

function key(ownerId: string, integrationId: string): string {
  return `${ownerId}::${integrationId}`;
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
}

async function getClient(ownerId: string, conn: Integration): Promise<Client> {
  const k = key(ownerId, conn.id);
  const existing = pool.get(k);
  if (existing) {
    armIdle(k);
    return existing.client;
  }
  const p = params(conn);
  const client = getSharedSSHClient(ownerId, p);
  const entry: Entry = { client, idleTimer: null, forwards: new Map() };
  pool.set(k, entry);
  armIdle(k);
  // Rebuild on a dropped connection; drop the entry on a failed setup.
  client.then((c) => c.on("close", () => { if (pool.get(k) === entry) evictByKey(k); }))
    .catch(() => { if (pool.get(k) === entry) { if (entry.idleTimer) clearTimeout(entry.idleTimer); pool.delete(k); } });
  return client;
}

/** Run a command on the owner's cached SSH connection. */
export async function runCommand(
  ownerId: string,
  conn: Integration,
  command: string,
  timeoutMs = DEFAULT_EXEC_TIMEOUT_MS,
): Promise<ExecResult> {
  const k = key(ownerId, conn.id);
  return withSshApprovalRetry(async () => {
    try {
      const client = await getClient(ownerId, conn);
      return await execOnce(client, command, timeoutMs);
    } catch (err) {
      // A command timeout leaves the SSH connection healthy — keep it so the next
      // call doesn't trigger a fresh agent (1Password) confirm. A connect still
      // waiting on an interactive approval must also stay pooled, or evicting it
      // would kill the tunnel the moment the user approves. Any other failure
      // may mean a dead connection, so evict and reconnect next time.
      if (!(err instanceof CommandTimeoutError) && !isSshPending(err)) {
        evictByKey(k);
        evictSharedSSHClient(ownerId, params(conn));
      }
      throw err;
    }
  });
}

/** Close all cached SSH connections for an owner (called on reset). */
export function closeOwnerClients(ownerId: string): void {
  for (const k of [...pool.keys()]) {
    if (k.startsWith(`${ownerId}::`)) evictByKey(k);
  }
}

// Tie SSH connection cleanup to the shared owner lifecycle (reload).
onOwnerClose(closeOwnerClients);

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
 * Open an `ssh -L` local port forward over the owner's cached SSH connection,
 * so localhost:<localPort> on this machine reaches remoteHost:remotePort from the
 * remote side. Idempotent per remote target; the forward lives until it is closed
 * or the owner is reset. Omit `requestedLocalPort` to auto-assign a free port.
 */
export async function openForward(
  ownerId: string,
  conn: Integration,
  remoteHost: string,
  remotePort: number,
  requestedLocalPort?: number,
): Promise<ForwardInfo> {
  const k = key(ownerId, conn.id);
  const client = await getClient(ownerId, conn);
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

/** List the open local port forwards for this owner's connection. */
export function listForwards(ownerId: string, conn: Integration): ForwardInfo[] {
  const entry = pool.get(key(ownerId, conn.id));
  if (!entry) return [];
  return [...entry.forwards.values()].map(forwardInfo);
}

/** Close one forward by id (`remoteHost:remotePort`). Returns false if unknown. */
export function closeForward(ownerId: string, conn: Integration, id: string): boolean {
  const k = key(ownerId, conn.id);
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
  const client = await connectSSH(params(conn));
  try {
    await execOnce(client, "echo pluk-ok", 15_000);
  } finally {
    client.end();
  }
}
