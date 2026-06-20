import { spawn } from "child_process";
import { Duplex } from "stream";
import { readFileSync, existsSync } from "fs";
import { homedir } from "os";

// Shared SSH config + auth resolution, used by both the DB tunnel (db/ssh.ts) and
// the SSH command adapter (adapters/ssh/client.ts). Parses ~/.ssh/config, resolves
// the agent socket (honoring IdentityAgent), and spawns ProxyCommand sockets so a
// host behind e.g. Cloudflare Access works the same everywhere.

export function expandHome(p: string): string {
  return p.startsWith("~/") ? `${homedir()}/${p.slice(2)}` : p;
}

export interface SSHConfigEntry {
  hostName?: string;
  identityFile?: string;
  identityAgent?: string;
  proxyCommand?: string;
  user?: string;
  port?: number;
}

export function parseSSHConfig(targetHost: string): SSHConfigEntry {
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

export function matchSSHPattern(pattern: string, host: string): boolean {
  if (pattern === "*") return true;
  const re = new RegExp(
    "^" + pattern.replace(/[.+^${}()|[\]\\]/g, "\\$&").replace(/\*/g, ".*").replace(/\?/g, ".") + "$",
    "i"
  );
  return re.test(host);
}

// Expand %h %p %r substitutions in a ProxyCommand template.
export function expandProxyCommand(template: string, host: string, port: number, user: string): string {
  return template
    .replace(/%h/g, host)
    .replace(/%p/g, String(port))
    .replace(/%r/g, user)
    .replace(/%u/g, user);
}

export function splitCommand(command: string): string[] {
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
export function spawnProxySocket(command: string): Duplex {
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

export function resolveAgentSocket(host: string): string | undefined {
  const fromConfig = parseSSHConfig(host).identityAgent;
  if (fromConfig) return fromConfig;
  return process.env.SSH_AUTH_SOCK;
}
