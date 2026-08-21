import { Client, utils as sshUtils } from "ssh2";
import { createConnection, createServer } from "net";
import { spawn } from "child_process";
import { Duplex } from "stream";
import { readFileSync, existsSync, mkdirSync } from "fs";
import { homedir, userInfo } from "os";
import type { ConnectConfig } from "ssh2";
import {
  expandHome,
  parseSSHConfig,
  expandProxyCommand,
  spawnProxySocket,
  resolveAgentSocket,
  type SSHConfigEntry,
} from "../ssh/config.js";
import { agentUnreachableError, resolveLiveAgent } from "../ssh/agent.js";
import { isSshAuthError } from "../ssh/pending.js";

export interface SSHTunnelConfig {
  host: string;
  port: number;
  user: string;
  authType: "agent" | "key" | "password";
  keyPath?: string;
  passphrase?: string; // key passphrase (key auth) or SSH password (password auth)
  remoteHost: string;
  remotePort: number;
}

export interface Tunnel {
  localPort: number;
  close: () => void;
}

// SSH handshake budget. Long enough for interactive agent/proxy auth (1Password
// confirm or Cloudflare browser approval), but still bounded.
const HANDSHAKE_TIMEOUT_MS = 180_000;
const FAST_RETRY_WINDOW_MS = 10_000;

// Connection multiplexing. Every tunnel to the same host+port+user rides one
// persistent master, so authentication — and with it the agent signature the
// 1Password prompt guards — happens once per CONTROL_PERSIST window instead of
// once per tunnel. `%C` hashes (local host, host, port, user) to 40 chars; the
// whole path must stay under the 104-byte sun_path limit, which rules out the
// longer `%h-%p-%r` template. pluk keeps its own socket rather than joining the
// user's (~/.ssh/control-*) so it never tears down an interactive session.
const CONTROL_DIR = `${homedir()}/.pluk`;
const CONTROL_PATH = `${CONTROL_DIR}/cm-%C`;
const CONTROL_PERSIST = "10m";
const CONTROL_CMD_TIMEOUT_MS = 10_000;
const MASTER_POLL_MS = 30_000;

class TunnelReadinessTimeout extends Error {}

// ── SSH config helpers ────────────────────────────────────────────────────────
// (parseSSHConfig, ProxyCommand, agent resolution) live in ../ssh/config.js,
// shared with the SSH command adapter.

function reserveLocalPort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const server = createServer();
    server.listen(0, "127.0.0.1", () => {
      const addr = server.address();
      const port = typeof addr === "object" && addr ? addr.port : 0;
      server.close(() => resolve(port));
    });
    server.on("error", reject);
  });
}

function waitForPort(port: number, timeoutMs = 15_000): Promise<void> {
  return new Promise((resolve, reject) => {
    const started = Date.now();

    const tryConnect = () => {
      const socket = createConnection({ host: "127.0.0.1", port });
      socket.once("connect", () => {
        socket.destroy();
        resolve();
      });
      socket.once("error", (err) => {
        socket.destroy();
        if (Date.now() - started > timeoutMs) {
          reject(new TunnelReadinessTimeout(
            `SSH tunnel did not become ready within ${Math.round(timeoutMs / 1000)}s: ${err.message}`
          ));
        }
        else setTimeout(tryConnect, 200);
      });
    };

    tryConnect();
  });
}

/** Run a short-lived ssh command to completion, capturing stderr. Used for the
 *  master's own lifecycle (`-O check` / `-O forward` / `-O cancel`) and for
 *  starting it — none of these hold a tunnel, so all of them terminate. */
function runSSHCommand(args: string[], timeoutMs: number): Promise<{ code: number; stderr: string }> {
  return new Promise((resolve) => {
    const child = spawn("ssh", args, { stdio: ["ignore", "ignore", "pipe"], env: process.env });
    const chunks: Buffer[] = [];
    child.stderr.on("data", (d: Buffer) => chunks.push(d));
    const timer = setTimeout(() => child.kill(), timeoutMs);
    const done = (code: number, extra?: string) => {
      clearTimeout(timer);
      const stderr = [Buffer.concat(chunks).toString().trim(), extra]
        .filter((l): l is string => Boolean(l))
        .join("\n")
        .split(/\r?\n/)
        // SSH's "closed by UNKNOWN" noise buries the real cause.
        .filter((l) => l && !/closed by UNKNOWN/i.test(l))
        .join("\n");
      resolve({ code, stderr });
    };
    child.on("close", (code) => done(code ?? 1));
    child.on("error", (e) => done(1, (e as Error).message));
  });
}

/** The args that identify one master: they feed `%C`, so every command that has
 *  to find the same socket — start, check, forward, cancel — must pass them. */
function masterTarget(config: SSHTunnelConfig, sshConfig: SSHConfigEntry, username: string): string[] {
  const args = ["-o", `ControlPath=${CONTROL_PATH}`];
  if (username) args.push("-l", username);
  args.push("-p", String(sshConfig.port ?? config.port));
  return args;
}

const masterStarts = new Map<string, Promise<void>>();

/** Bring up the shared master, or confirm the running one. This is the only step
 *  that authenticates, so it is the only step that can block on a 1Password
 *  approval — once per CONTROL_PERSIST window rather than once per tunnel. */
async function ensureMaster(
  config: SSHTunnelConfig,
  target: string[],
  timeoutMs: number,
): Promise<void> {
  const key = [...target, config.host].join(" ");
  const inFlight = masterStarts.get(key);
  // Two tunnels opening at once must not both try to own the socket.
  if (inFlight) return inFlight;

  const start = (async () => {
    mkdirSync(CONTROL_DIR, { recursive: true });
    const check = await runSSHCommand(["-O", "check", ...target, config.host], CONTROL_CMD_TIMEOUT_MS);
    if (check.code === 0) return; // already up: no auth, no prompt, no wait

    // ControlMaster=auto + ControlPersist makes ssh fork the master into the
    // background and exit once it is ready, so this command returns when the
    // master is usable. `auto` (not `-M`) keeps it idempotent if another process
    // won the race: it attaches, finds nothing to run under -N, and exits 0.
    const args = [
      "-N", "-f",
      "-o", "ControlMaster=auto",
      "-o", `ControlPersist=${CONTROL_PERSIST}`,
      "-o", "ServerAliveInterval=30",
      "-o", "ServerAliveCountMax=3",
      ...target,
    ];
    // A key tunnel authenticates with the file it was given and nothing else:
    // the agent is never consulted, so a locked or refusing 1Password cannot
    // fail a connection that needs no signature from it, and no default
    // ~/.ssh key is offered beside the configured one.
    if (config.authType === "key" && config.keyPath) {
      args.push("-i", expandHome(config.keyPath), "-o", "IdentitiesOnly=yes", "-o", "IdentityAgent=none");
    }

    // Point ssh at a probed, live agent socket explicitly. A GUI-launched app
    // inherits whatever SSH_AUTH_SOCK launchd set — often an empty or stale
    // agent — so trusting the env means ssh queries a socket that can never
    // sign or prompt. The probe walks IdentityAgent from ~/.ssh/config, then
    // SSH_AUTH_SOCK, then the well-known 1Password sockets, and the probe's own
    // agent request is what wakes a locked 1Password into showing its unlock
    // window. No live socket at all is a hard auth failure: fail now, before
    // ssh burns the handshake budget waiting on a prompt that cannot appear.
    // -o overrides config/env, so the agent is deterministic.
    if (config.authType === "agent") {
      const agent = await resolveLiveAgent(config.host);
      if (!agent) throw agentUnreachableError();
      console.log(`[pluk] SSH agent socket: ${agent.socket} (${agent.probe.state})`);
      // ssh parses the -o value with its own tokenizer, so a socket path with
      // spaces (e.g. 1Password's "~/Library/Group Containers/…/agent.sock") must
      // be quoted inside the option string or ssh errors "extra arguments".
      args.push("-o", `IdentityAgent="${agent.socket}"`);
    }
    args.push(config.host);

    console.log(`[pluk] OpenSSH master: ssh ${args.join(" ")}`);
    const started = await runSSHCommand(args, timeoutMs);
    if (started.code !== 0) throw new Error(started.stderr || `ssh master failed (exit ${started.code})`);
  })();

  masterStarts.set(key, start);
  try {
    await start;
  } finally {
    masterStarts.delete(key);
  }
}

async function openOpenSSHTunnel(
  config: SSHTunnelConfig,
  sshConfig: SSHConfigEntry,
  username: string,
  readinessTimeoutMs: number,
  onFatal?: () => void
): Promise<Tunnel> {
  const target = masterTarget(config, sshConfig, username);
  const started = Date.now();
  await ensureMaster(config, target, readinessTimeoutMs);

  const localPort = await reserveLocalPort();
  const spec = `127.0.0.1:${localPort}:${config.remoteHost}:${config.remotePort}`;

  // The forward belongs to the master, not to a child of ours: it outlives any
  // single ssh invocation and is removed by `-O cancel`, never by killing a pid.
  const fwd = await runSSHCommand(["-O", "forward", "-L", spec, ...target, config.host], CONTROL_CMD_TIMEOUT_MS);
  if (fwd.code !== 0) throw new Error(fwd.stderr || `ssh -O forward failed (exit ${fwd.code})`);

  const remaining = Math.max(1_000, readinessTimeoutMs - (Date.now() - started));
  try {
    await waitForPort(localPort, remaining);
  } catch (err) {
    await runSSHCommand(["-O", "cancel", "-L", spec, ...target, config.host], CONTROL_CMD_TIMEOUT_MS);
    throw err;
  }

  console.log(`[pluk] tunnel ready on localhost:${localPort}`);

  // Self-heal: a master that dies (server idle disconnect, dropped NAT mapping,
  // network loss) takes every forward with it, and there is no child exit to
  // watch any more — poll it so the driver is rebuilt instead of left with a
  // dead local listener that hangs every query.
  let closed = false;
  const poll = setInterval(async () => {
    if (closed) return;
    const alive = await runSSHCommand(["-O", "check", ...target, config.host], CONTROL_CMD_TIMEOUT_MS);
    if (closed || alive.code === 0) return;
    clearInterval(poll);
    onFatal?.();
  }, MASTER_POLL_MS);
  poll.unref?.();

  return {
    localPort,
    close: () => {
      closed = true;
      clearInterval(poll);
      // Drop just this forward; the master stays up for the next tunnel.
      void runSSHCommand(["-O", "cancel", "-L", spec, ...target, config.host], CONTROL_CMD_TIMEOUT_MS);
    },
  };
}

// ── Tunnel ────────────────────────────────────────────────────────────────────

export async function openSSHTunnel(
  config: SSHTunnelConfig,
  ownerIdOrFatal?: string | (() => void),
  maybeOnFatal?: () => void
): Promise<Tunnel> {
  const sshConfig = parseSSHConfig(config.host);
  const username = config.user || sshConfig.user || userInfo().username;
  const onFatal = typeof ownerIdOrFatal === "function" ? ownerIdOrFatal : maybeOnFatal;

  // Route agent/key tunnels through the system `ssh` binary. The in-process ssh2
  // forwardOut channel opens but silently fails to pass data under Bun: the
  // driver connects to a live-looking local port that never delivers a byte and
  // dies on the connect timeout. OpenSSH forwards reliably and drives the
  // 1Password agent via IdentityAgent. Password auth can't be fed to OpenSSH
  // non-interactively; encrypted key files also need ssh2's passphrase support.
  if (config.authType === "agent" || (config.authType === "key" && !config.passphrase)) {
    // proxyCommand tunnels (e.g. Cloudflare Access) can fail transiently on DNS
    // or auth — retry within the handshake budget. Direct tunnels get one shot.
    const attempts = sshConfig.proxyCommand ? 3 : 1;
    let lastErr: Error | undefined;
    const deadline = Date.now() + HANDSHAKE_TIMEOUT_MS;
    for (let attempt = 1; attempt <= attempts; attempt++) {
      const started = Date.now();
      const remaining = deadline - started;
      if (remaining <= 0) break;
      try {
        return await openOpenSSHTunnel(config, sshConfig, username, remaining, onFatal);
      } catch (err) {
        lastErr = err as Error;
        // An auth/agent failure won't clear on retry — surface it now.
        if (isSshAuthError(lastErr)) break;
        const failedFast = Date.now() - started < FAST_RETRY_WINDOW_MS;
        if (attempt < attempts && failedFast && !(lastErr instanceof TunnelReadinessTimeout)) {
          console.warn(`[pluk] OpenSSH tunnel attempt ${attempt} failed: ${lastErr.message}. Retrying in 2s…`);
          await new Promise((r) => setTimeout(r, 2000));
        }
        else break;
      }
    }
    throw lastErr ?? new Error("SSH tunnel did not become ready before connect deadline");
  }

  return new Promise((resolve, reject) => {
    const sshClient = new Client();
    let settled = false;
    let proxySock: Duplex | undefined;

    const fail = (err: Error) => {
      if (settled) return;
      settled = true;
      proxySock?.destroy();
      sshClient.end();
      reject(err);
    };

    sshClient.on("error", (err) => {
      console.error(`[pluk] SSH error (${config.host}): ${err.message}`);
      fail(err);
    });

    sshClient.on("ready", () => {
      console.log(`[pluk] SSH connected → ${config.host}, forwarding ${config.remoteHost}:${config.remotePort}`);
      const forwardServer = createServer((socket) => {
        sshClient.forwardOut(
          "127.0.0.1", 0,
          config.remoteHost, config.remotePort,
          (err, channel) => {
            if (err) {
              console.error(`[pluk] forwardOut error: ${err.message}`);
              socket.destroy();
              return;
            }
            socket.pipe(channel);
            channel.pipe(socket);
            socket.on("close", () => channel.destroy());
            channel.on("close", () => socket.destroy());
          }
        );
      });

      forwardServer.listen(0, "127.0.0.1", () => {
        const addr = forwardServer.address();
        const localPort = typeof addr === "object" && addr ? addr.port : 0;
        console.log(`[pluk] tunnel ready on localhost:${localPort}`);
        settled = true;
        sshClient.removeAllListeners("error");
        sshClient.on("error", (err) => {
          console.error("[pluk] SSH tunnel error:", err.message);
        });
        // Self-heal: a dropped SSH connection (keepalive timeout, server idle
        // disconnect, network loss) emits 'close'. Tear the local listener down
        // and notify so the driver is rebuilt — otherwise the listener lingers
        // and every later query hangs against a dead tunnel.
        let intentional = false;
        sshClient.on("close", () => {
          forwardServer.close();
          if (!intentional) onFatal?.();
        });
        resolve({
          localPort,
          close: () => { intentional = true; forwardServer.close(); proxySock?.destroy(); sshClient.end(); },
        });
      });

      forwardServer.on("error", fail);
    });

    const host = sshConfig.hostName ?? config.host;

    const connectCfg: ConnectConfig = {
      host,
      port: sshConfig.port ?? config.port,
      username,
      // ssh2's readyTimeout defaults to 20s. Agent auth (e.g. 1Password SSH
      // agent) blocks on an interactive confirm prompt during the handshake; a
      // user who takes longer than 20s to approve hits ssh2's own timeout even
      // though the pool grants SSH setup a far larger budget. Align with that
      // budget so the prompt — not the library — sets the deadline.
      readyTimeout: HANDSHAKE_TIMEOUT_MS,
      // Match the OpenSSH path's ServerAliveInterval=30/CountMax=3 so an idle
      // tunnel is kept alive and a dead peer is detected.
      keepaliveInterval: 30_000,
      keepaliveCountMax: 3,
    };

    // Route through ProxyCommand if configured (e.g. Cloudflare Access).
    if (sshConfig.proxyCommand) {
      const cmd = expandProxyCommand(sshConfig.proxyCommand, host, connectCfg.port ?? 22, username);
      console.log(`[pluk] ProxyCommand: ${cmd}`);
      proxySock = spawnProxySocket(cmd);
      connectCfg.sock = proxySock;
    }

    if (config.authType === "agent") {
      connectCfg.agent = resolveAgentSocket(config.host);
    } else if (config.authType === "key") {
      const agent = resolveAgentSocket(config.host);
      if (agent) connectCfg.agent = agent;

      const candidates = [
        config.keyPath ? expandHome(config.keyPath) : null,
        sshConfig.identityFile ?? null,
        `${homedir()}/.ssh/id_ed25519`,
        `${homedir()}/.ssh/id_rsa`,
      ].filter((p): p is string => p !== null).filter(existsSync);

      if (candidates.length === 0) {
        reject(new Error("No SSH private key found. Set a key path in the connection settings."));
        return;
      }

      let resolvedKey: Buffer | null = null;
      let resolvedPath: string | null = null;

      for (const candidate of candidates) {
        let keyData: Buffer;
        try { keyData = readFileSync(candidate); } catch { continue; }
        const parsed = sshUtils.parseKey(keyData, config.passphrase ?? "");
        const ok = Array.isArray(parsed) ? parsed.length > 0 : !(parsed instanceof Error);
        if (ok) { resolvedKey = keyData; resolvedPath = candidate; break; }
      }

      if (!resolvedKey) {
        const tried = candidates.join(", ");
        reject(new Error(
          config.passphrase
            ? `Bad passphrase for keys tried: ${tried}`
            : `All candidate keys are encrypted — set a passphrase. Tried: ${tried}`
        ));
        return;
      }

      console.log(`[pluk] SSH key: ${resolvedPath}`);
      connectCfg.privateKey = resolvedKey;
      if (config.passphrase) connectCfg.passphrase = config.passphrase;
    } else {
      connectCfg.password = config.passphrase ?? "";
      connectCfg.tryKeyboard = true;
    }

    if (connectCfg.tryKeyboard) {
      sshClient.on("keyboard-interactive", (_name, _instructions, _lang, prompts, finish) => {
        finish(prompts.map(() => config.passphrase ?? ""));
      });
    }

    sshClient.connect(connectCfg);
  });
}
