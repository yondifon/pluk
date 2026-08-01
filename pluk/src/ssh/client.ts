import { Client, utils as sshUtils } from "ssh2";
import type { ConnectConfig } from "ssh2";
import { readFileSync, existsSync } from "fs";
import { homedir, userInfo } from "os";
import { Duplex } from "stream";
import { onOwnerClose } from "../mcp/pool.js";
import {
  SSH_CONNECT_WAIT_MS,
  clearConnectEpisode,
  connectWaitError,
  isSshStalled,
  recordConnectFailure,
} from "./pending.js";
import {
  expandHome,
  parseSSHConfig,
  expandProxyCommand,
  spawnProxySocket,
  resolveAgentSocket,
} from "./config.js";
import type { SSHConfigEntry } from "./config.js";

const READY_TIMEOUT_MS = 180_000;

export interface SSHParams {
  host: string;
  port: number;
  user: string;
  authType: "agent" | "key" | "password";
  keyPath?: string;
  password?: string;
}

type AuthMethod =
  | { type: "none"; username: string }
  | { type: "agent"; username: string; agent: string }
  | { type: "publickey"; username: string; key: Buffer; passphrase?: string };

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

function parseableKey(path: string, passphrase?: string): Buffer | null {
  if (!existsSync(path)) return null;
  let data: Buffer;
  try { data = readFileSync(path); } catch { return null; }
  const parsed = sshUtils.parseKey(data, passphrase ?? "");
  const ok = Array.isArray(parsed) ? parsed.length > 0 : !(parsed instanceof Error);
  return ok ? data : null;
}

export function connectSSH(p: SSHParams): Promise<Client> {
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

    const connectTimer = setTimeout(() => {
      fail(new Error(`Couldn't reach ${host}:${port} within ${Math.round(READY_TIMEOUT_MS / 1000)}s — check the host, port, and any SSH proxy (cloudflared).`));
    }, READY_TIMEOUT_MS + 10_000);

    client.on("ready", () => { if (!settled) { settled = true; clearTimeout(connectTimer); resolve(client); } });
    client.on("error", (err) => fail(err));

    const cfg: ConnectConfig = {
      host,
      port,
      username,
      readyTimeout: READY_TIMEOUT_MS,
      keepaliveInterval: 30_000,
      keepaliveCountMax: 3,
    };

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

interface Entry {
  client: Promise<Client>;
  startedAt: number;
  settled: boolean;
  interactive: boolean; // agent/key auth can block on an approval prompt
}

const pool = new Map<string, Entry>();

function sharedKey(ownerId: string, p: SSHParams): string {
  const sshConfig = parseSSHConfig(p.host);
  const host = sshConfig.hostName ?? p.host;
  const port = sshConfig.port ?? p.port;
  const username = p.user || sshConfig.user || userInfo().username;
  return [
    ownerId,
    host,
    port,
    username,
    p.authType,
    p.keyPath ?? "",
    p.password ? "password-set" : "",
  ].join("::");
}

export function getSharedSSHClient(ownerId: string, p: SSHParams): Promise<Client> {
  const key = sharedKey(ownerId, p);
  const existing = pool.get(key);
  // Wait on the attempt already running rather than racing a second one beside
  // it: eviction can't cancel an unsettled connect (its close only fires once
  // the promise settles), so falling through here used to stack connections.
  if (existing) return awaitReady(key, existing);

  const client = connectSSH(p);
  const entry: Entry = { client, startedAt: Date.now(), settled: false, interactive: p.authType !== "password" };
  client.then(
    () => { entry.settled = true; clearConnectEpisode(key); },
    (e) => { entry.settled = true; recordConnectFailure(key, e); },
  );
  pool.set(key, entry);
  client.then((c) => c.on("close", () => { if (pool.get(key) === entry) evictByKey(key); }))
    .catch(() => { if (pool.get(key) === entry) pool.delete(key); });
  return awaitReady(key, entry);
}

// Bound a caller's wait on an in-flight connect that may be blocked on an
// interactive approval (1Password confirm, proxy browser login). The connect
// keeps running; once approved it stays pooled for the next call. Once the pool
// has claimed "waiting for approval" too many times for this key with nothing
// connecting, drop the doomed attempt and report the real failure instead.
function awaitReady(key: string, entry: Entry): Promise<Client> {
  if (entry.settled || !entry.interactive) return entry.client;
  return Promise.race([
    entry.client,
    new Promise<Client>((_, reject) => setTimeout(() => {
      if (entry.settled) return; // connect already landed; the race is over
      const err = connectWaitError(key);
      if (isSshStalled(err) && pool.get(key) === entry) evictByKey(key);
      reject(err);
    }, SSH_CONNECT_WAIT_MS)),
  ]);
}

export function evictSharedSSHClient(ownerId: string, p: SSHParams): void {
  evictByKey(sharedKey(ownerId, p));
}

function evictByKey(key: string): void {
  const entry = pool.get(key);
  if (!entry) return;
  pool.delete(key);
  entry.client.then((c) => c.end()).catch(() => {});
}

export function closeOwnerSSHClients(ownerId: string): void {
  for (const key of [...pool.keys()]) {
    if (key.startsWith(`${ownerId}::`)) evictByKey(key);
  }
}

onOwnerClose(closeOwnerSSHClients);
