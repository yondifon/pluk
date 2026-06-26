import { Client, utils as sshUtils } from "ssh2";
import { createConnection, createServer } from "net";
import { spawn } from "child_process";
import { Duplex } from "stream";
import { readFileSync, existsSync } from "fs";
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
import { getSharedSSHClient, evictSharedSSHClient, type SSHParams } from "../ssh/client.js";

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

class TunnelReadinessTimeout extends Error {}

// Auth/agent failures are deterministic — a retry or a longer wait can't fix a
// missing key, a locked agent, or a rejected pubkey. Detect them so the tunnel
// fails fast and loud instead of burning the retry loop / handshake budget.
function isAuthError(message: string): boolean {
  return /permission denied|communication with agent failed|signing failed|publickey|no supported authentication|authentication failed|too many authentication failures/i.test(message);
}

function sharedParams(config: SSHTunnelConfig, username: string): SSHParams {
  return {
    host: config.host,
    port: config.port,
    user: username,
    authType: config.authType,
    keyPath: config.keyPath,
    password: config.passphrase,
  };
}

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

async function openOpenSSHTunnel(
  config: SSHTunnelConfig,
  sshConfig: SSHConfigEntry,
  username: string,
  readinessTimeoutMs: number,
  onFatal?: () => void
): Promise<Tunnel> {
  const localPort = await reserveLocalPort();
  const args = [
    "-N",
    "-S", "none",
    "-o", "ControlMaster=no",
    "-o", "ControlPath=none",
    "-o", "ControlPersist=no",
    "-o", "ExitOnForwardFailure=yes",
    "-o", "ServerAliveInterval=30",
    "-o", "ServerAliveCountMax=3",
    "-L", `127.0.0.1:${localPort}:${config.remoteHost}:${config.remotePort}`,
  ];

  if (username) args.push("-l", username);
  args.push("-p", String(sshConfig.port ?? config.port));
  if (config.authType === "key" && config.keyPath) args.push("-i", expandHome(config.keyPath));

  // Point ssh at the resolved agent (IdentityAgent from ~/.ssh/config, e.g. the
  // 1Password socket, else SSH_AUTH_SOCK) explicitly. A GUI-launched app inherits
  // the empty macOS launchd agent in SSH_AUTH_SOCK; without this, ssh would query
  // that keyless agent and fail with "communication with agent failed" instead of
  // using the 1Password keys. -o overrides config/env, so the agent is deterministic.
  if (config.authType === "agent") {
    const agentSock = resolveAgentSocket(config.host);
    // ssh parses the -o value with its own tokenizer, so a socket path with
    // spaces (e.g. 1Password's "~/Library/Group Containers/…/agent.sock") must
    // be quoted inside the option string or ssh errors "extra arguments".
    if (agentSock) args.push("-o", `IdentityAgent="${agentSock}"`);
  }

  args.push(config.host);

  console.log(`[pluk] OpenSSH tunnel: ssh ${args.join(" ")}`);

  const child = spawn("ssh", args, {
    stdio: ["ignore", "ignore", "pipe"],
    env: process.env,
  });

  // Collect stderr asynchronously; child may still write after kill()
  const stderrChunks: Buffer[] = [];
  child.stderr.on("data", (data: Buffer) => stderrChunks.push(data));

  const childClosed = new Promise<void>((res) => child.on("close", () => res()));

  try {
    // ProxyCommand auth (e.g. Cloudflare Access) can require an interactive
    // browser confirm on first use. Short-circuit if ssh dies first, but don't
    // kill a live auth prompt before the shared SSH setup deadline.
    const childDied = childClosed.then(() => { throw new Error("ssh process exited before tunnel was ready"); });
    await Promise.race([waitForPort(localPort, readinessTimeoutMs), childDied]);
  } catch (err) {
    child.kill();
    // Wait up to 1s for the child to flush all stderr before reading it
    await Promise.race([childClosed, new Promise<void>((r) => setTimeout(r, 1000))]);
    const stderr = Buffer.concat(stderrChunks).toString().trim();
    // Filter out SSH's unhelpful "closed by UNKNOWN" noise; surface proxy errors first
    const lines = stderr.split(/\r?\n/).filter(l => l && !/closed by UNKNOWN/i.test(l));
    const message = lines.join("\n") || (err as Error).message;
    if (err instanceof TunnelReadinessTimeout) throw new TunnelReadinessTimeout(message);
    throw new Error(message);
  }

  console.log(`[pluk] tunnel ready on localhost:${localPort}`);

  // Self-heal: if the ssh process dies after the tunnel is up (server idle
  // disconnect, dropped NAT mapping, network loss), notify so the driver is
  // rebuilt instead of leaving a dead local listener that hangs every query.
  let intentional = false;
  childClosed.then(() => { if (!intentional) onFatal?.(); });

  return {
    localPort,
    close: () => { intentional = true; child.kill(); },
  };
}

async function openSharedClientTunnel(
  config: SSHTunnelConfig,
  sessionId: string,
  username: string,
  onFatal?: () => void
): Promise<Tunnel> {
  const params = sharedParams(config, username);
  const sshClient = await getSharedSSHClient(sessionId, params);
  const forwardServer = createServer((socket) => {
    sshClient.forwardOut(
      "127.0.0.1", 0,
      config.remoteHost, config.remotePort,
      (err, channel) => {
        if (err) {
          socket.destroy();
          evictSharedSSHClient(sessionId, params);
          onFatal?.();
          return;
        }
        socket.pipe(channel);
        channel.pipe(socket);
        socket.on("close", () => channel.destroy());
        channel.on("close", () => socket.destroy());
      }
    );
  });

  return new Promise((resolve, reject) => {
    forwardServer.once("error", reject);
    forwardServer.listen(0, "127.0.0.1", () => {
      forwardServer.removeListener("error", reject);
      forwardServer.on("error", () => {});
      const addr = forwardServer.address();
      const localPort = typeof addr === "object" && addr ? addr.port : 0;
      let intentional = false;
      sshClient.once("close", () => {
        forwardServer.close();
        if (!intentional) onFatal?.();
      });
      resolve({
        localPort,
        close: () => {
          intentional = true;
          forwardServer.close();
        },
      });
    });
  });
}

// ── Tunnel ────────────────────────────────────────────────────────────────────

export async function openSSHTunnel(
  config: SSHTunnelConfig,
  sessionIdOrFatal?: string | (() => void),
  maybeOnFatal?: () => void
): Promise<Tunnel> {
  const sshConfig = parseSSHConfig(config.host);
  const username = config.user || sshConfig.user || userInfo().username;
  const sessionId = typeof sessionIdOrFatal === "string" ? sessionIdOrFatal : undefined;
  const onFatal = typeof sessionIdOrFatal === "function" ? sessionIdOrFatal : maybeOnFatal;

  if (sessionId) {
    return openSharedClientTunnel(config, sessionId, username, onFatal);
  }

  if (sshConfig.proxyCommand) {
    // Cloudflare Access and other proxy tunnels can fail transiently on DNS or auth — retry
    let lastErr: Error | undefined;
    const deadline = Date.now() + HANDSHAKE_TIMEOUT_MS;
    for (let attempt = 1; attempt <= 3; attempt++) {
      const started = Date.now();
      const remaining = deadline - started;
      if (remaining <= 0) break;
      try {
        return await openOpenSSHTunnel(config, sshConfig, username, remaining, onFatal);
      } catch (err) {
        lastErr = err as Error;
        // An auth/agent failure won't clear on retry — surface it now.
        if (isAuthError(lastErr.message)) break;
        const failedFast = Date.now() - started < FAST_RETRY_WINDOW_MS;
        if (attempt < 3 && failedFast && !(lastErr instanceof TunnelReadinessTimeout)) {
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
