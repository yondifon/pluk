import { Client } from "ssh2";
import { createServer } from "net";
import { readFileSync, existsSync } from "fs";
import { homedir } from "os";
import type { ConnectConfig } from "ssh2";

export interface SSHTunnelConfig {
  host: string;
  port: number;
  user: string;
  authType: "agent" | "key" | "password";
  keyPath?: string;
  password?: string;
  remoteHost: string;
  remotePort: number;
}

export interface Tunnel {
  localPort: number;
  close: () => void;
}

function resolveAgentSocket(): string | undefined {
  const configPath = `${homedir()}/.ssh/config`;
  if (existsSync(configPath)) {
    const lines = readFileSync(configPath, "utf8").split("\n");
    for (const line of lines) {
      const m = line.trim().match(/^IdentityAgent\s+"?([^"]+)"?$/i);
      if (m?.[1]) return m[1].replace(/^~/, homedir());
    }
  }
  return process.env.SSH_AUTH_SOCK;
}

export function openSSHTunnel(config: SSHTunnelConfig): Promise<Tunnel> {
  return new Promise((resolve, reject) => {
    const sshClient = new Client();

    sshClient.on("error", reject);

    sshClient.on("ready", () => {
      const forwardServer = createServer((socket) => {
        sshClient.forwardOut(
          "127.0.0.1", 0,
          config.remoteHost, config.remotePort,
          (err, channel) => {
            if (err) { socket.destroy(); return; }
            socket.pipe(channel);
            channel.pipe(socket);
          }
        );
      });

      forwardServer.listen(0, "127.0.0.1", () => {
        const addr = forwardServer.address();
        const localPort = typeof addr === "object" && addr ? addr.port : 0;
        resolve({
          localPort,
          close: () => { forwardServer.close(); sshClient.end(); },
        });
      });

      forwardServer.on("error", (err) => { sshClient.end(); reject(err); });
    });

    const connectCfg: ConnectConfig = {
      host: config.host,
      port: config.port,
      username: config.user,
    };

    if (config.authType === "agent") {
      connectCfg.agent = resolveAgentSocket();
    } else if (config.authType === "key" && config.keyPath) {
      connectCfg.privateKey = readFileSync(config.keyPath);
    } else {
      connectCfg.password = config.password ?? "";
    }

    sshClient.connect(connectCfg);
  });
}
