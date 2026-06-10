import { Client, utils as sshUtils } from "ssh2";
import { createConnection, createServer } from "net";
import { spawn } from "child_process";
import { Duplex } from "stream";
import { readFileSync, existsSync } from "fs";
import { homedir, userInfo } from "os";
import type { ConnectConfig } from "ssh2";

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

// ── SSH config helpers ────────────────────────────────────────────────────────

function expandHome(p: string): string {
  return p.startsWith("~/") ? `${homedir()}/${p.slice(2)}` : p;
}

interface SSHConfigEntry {
  hostName?: string;
  identityFile?: string;
  identityAgent?: string;
  proxyCommand?: string;
  user?: string;
  port?: number;
}

function parseSSHConfig(targetHost: string): SSHConfigEntry {
  const configPath = `${homedir()}/.ssh/config`;
  if (!existsSync(configPath)) return {};

  const lines = readFileSync(configPath, "utf8").split("\n");
  const result: SSHConfigEntry = {};
  let inMatchingBlock = true;

  for (const raw of lines) {
    const line = raw.trim();
    if (!line || line.startsWith("#")) continue;

    const parts = splitCommand(line);
    const key = parts[0];
    const val = parts.slice(1).join(" ");

    if (key?.toLowerCase() === "host") {
      inMatchingBlock = val.split(/\s+/).some((p) => matchSSHPattern(p, targetHost));
      continue;
    }

    if (!inMatchingBlock) continue;

    switch (key?.toLowerCase()) {
      case "hostname":
        if (!result.hostName) result.hostName = val;
        break;
      case "identityfile":
        if (!result.identityFile) result.identityFile = expandHome(val);
        break;
      case "identityagent":
        if (!result.identityAgent) result.identityAgent = expandHome(val);
        break;
      case "proxycommand":
        if (!result.proxyCommand && val.toLowerCase() !== "none") result.proxyCommand = val;
        break;
      case "user":
        if (!result.user) result.user = val;
        break;
      case "port":
        if (!result.port) result.port = parseInt(val, 10);
        break;
    }
  }

  return result;
}

function matchSSHPattern(pattern: string, host: string): boolean {
  if (pattern === "*") return true;
  const re = new RegExp(
    "^" + pattern.replace(/[.+^${}()|[\]\\]/g, "\\$&").replace(/\*/g, ".*").replace(/\?/g, ".") + "$",
    "i"
  );
  return re.test(host);
}

// Expand %h %p %r substitutions in a ProxyCommand template.
function expandProxyCommand(template: string, host: string, port: number, user: string): string {
  return template
    .replace(/%h/g, host)
    .replace(/%p/g, String(port))
    .replace(/%r/g, user)
    .replace(/%u/g, user);
}

function splitCommand(command: string): string[] {
  const args: string[] = [];
  let current = "";
  let quote: "'" | "\"" | null = null;
  let escaping = false;

  for (const char of command) {
    if (escaping) {
      current += char;
      escaping = false;
      continue;
    }

    if (char === "\\") {
      escaping = true;
      continue;
    }

    if (quote) {
      if (char === quote) quote = null;
      else current += char;
      continue;
    }

    if (char === "'" || char === "\"") {
      quote = char;
      continue;
    }

    if (/\s/.test(char)) {
      if (current) {
        args.push(current);
        current = "";
      }
      continue;
    }

    current += char;
  }

  if (escaping) current += "\\";
  if (current) args.push(current);
  return args;
}

// Spawn a ProxyCommand and return a Duplex stream backed by its stdin/stdout.
function spawnProxySocket(command: string): Duplex {
  const [prog, ...args] = splitCommand(command);
  if (!prog) throw new Error("ProxyCommand is empty");

  const child = spawn(prog, args, {
    stdio: ["pipe", "pipe", "inherit"],
    env: process.env,
  });

  const sock = new Duplex({
    read() {
      child.stdout!.resume();
    },
    write(chunk, _enc, cb) {
      child.stdin!.write(chunk, cb);
    },
    final(cb) {
      child.stdin!.end(cb);
    },
    destroy(err, cb) {
      if (!child.killed) child.kill();
      cb(err);
    },
  });

  child.stdout!.on("data", (data: Buffer) => {
    if (!sock.push(data)) child.stdout!.pause();
  });
  child.stdout!.on("error", (err) => sock.destroy(err));
  child.stdin!.on("error", (err) => sock.destroy(err));
  child.stdout!.on("end", () => sock.push(null));
  child.on("error", (err) => sock.destroy(err));
  child.on("close", (code) => {
    if (!sock.destroyed) {
      if (code !== 0) sock.destroy(new Error(`ProxyCommand exited with code ${code}`));
      else sock.push(null);
    }
  });

  return sock;
}

function resolveAgentSocket(host: string): string | undefined {
  const fromConfig = parseSSHConfig(host).identityAgent;
  if (fromConfig) return fromConfig;
  return process.env.SSH_AUTH_SOCK;
}

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
        if (Date.now() - started > timeoutMs) reject(err);
        else setTimeout(tryConnect, 200);
      });
    };

    tryConnect();
  });
}

async function openOpenSSHTunnel(
  config: SSHTunnelConfig,
  sshConfig: SSHConfigEntry,
  username: string
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
    await waitForPort(localPort);
  } catch (err) {
    child.kill();
    // Wait up to 1s for the child to flush all stderr before reading it
    await Promise.race([childClosed, new Promise<void>((r) => setTimeout(r, 1000))]);
    const stderr = Buffer.concat(stderrChunks).toString().trim();
    // Filter out SSH's unhelpful "closed by UNKNOWN" noise; surface proxy errors first
    const lines = stderr.split(/\r?\n/).filter(l => l && !/closed by UNKNOWN/i.test(l));
    throw new Error(lines.join("\n") || (err as Error).message);
  }

  console.log(`[pluk] tunnel ready on localhost:${localPort}`);

  return {
    localPort,
    close: () => child.kill(),
  };
}

// ── Tunnel ────────────────────────────────────────────────────────────────────

export async function openSSHTunnel(config: SSHTunnelConfig): Promise<Tunnel> {
  const sshConfig = parseSSHConfig(config.host);
  const username = config.user || sshConfig.user || userInfo().username;

  if (sshConfig.proxyCommand) {
    // Cloudflare Access and other proxy tunnels can fail transiently on DNS or auth — retry
    let lastErr: Error | undefined;
    for (let attempt = 1; attempt <= 3; attempt++) {
      try {
        return await openOpenSSHTunnel(config, sshConfig, username);
      } catch (err) {
        lastErr = err as Error;
        if (attempt < 3) {
          console.warn(`[pluk] OpenSSH tunnel attempt ${attempt} failed: ${lastErr.message}. Retrying in 2s…`);
          await new Promise((r) => setTimeout(r, 2000));
        }
      }
    }
    throw lastErr!;
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
          forwardServer.close();
        });
        resolve({
          localPort,
          close: () => { forwardServer.close(); proxySock?.destroy(); sshClient.end(); },
        });
      });

      forwardServer.on("error", fail);
    });

    const host = sshConfig.hostName ?? config.host;

    const connectCfg: ConnectConfig = {
      host,
      port: sshConfig.port ?? config.port,
      username,
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
