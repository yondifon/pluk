import { createConnection } from "net";
import { existsSync } from "fs";
import { homedir } from "os";
import { parseSSHConfig } from "./config.js";

// SSH agent reachability. Auth through an agent can only work — and an approval
// prompt can only appear — if requests actually reach a live agent. A
// GUI-launched app inherits whatever SSH_AUTH_SOCK launchd set, which is often
// an empty or stale agent, while the user's 1Password agent listens on its own
// well-known socket. So before claiming anything about approvals, probe the
// candidates with a real agent request (SSH_AGENTC_REQUEST_IDENTITIES) and pick
// the first one that can plausibly sign. The probe doubles as the wake-up call:
// a locked 1Password pops its unlock window on exactly this kind of request.

export const SSH_AGENT_UNREACHABLE_CODE = "SSH_AGENT_UNREACHABLE";

const PROBE_TIMEOUT_MS = 2_000;

const SSH_AGENTC_REQUEST_IDENTITIES = 11;
const SSH_AGENT_IDENTITIES_ANSWER = 12;

export type AgentProbe =
  | { state: "keys"; keys: number } // answered the identity list, has keys
  | { state: "empty" } // answered with zero keys — reachable but can't sign
  | { state: "mute" } // connected but no answer — e.g. a locked agent holding a prompt
  | { state: "dead"; error: string };

export function probeAgentSocket(path: string, timeoutMs = PROBE_TIMEOUT_MS): Promise<AgentProbe> {
  return new Promise((resolve) => {
    const sock = createConnection(path);
    let connected = false;
    let buf = Buffer.alloc(0);
    let done = false;
    const finish = (result: AgentProbe) => {
      if (done) return;
      done = true;
      clearTimeout(timer);
      sock.destroy();
      resolve(result);
    };
    const timer = setTimeout(
      () => finish(connected ? { state: "mute" } : { state: "dead", error: "connect timed out" }),
      timeoutMs
    );
    sock.on("connect", () => {
      connected = true;
      sock.write(Buffer.from([0, 0, 0, 1, SSH_AGENTC_REQUEST_IDENTITIES]));
    });
    sock.on("data", (chunk: Buffer) => {
      buf = Buffer.concat([buf, chunk]);
      if (buf.length < 5) return;
      if (buf[4] !== SSH_AGENT_IDENTITIES_ANSWER) return finish({ state: "mute" });
      if (buf.length < 9) return;
      const keys = buf.readUInt32BE(5);
      finish(keys > 0 ? { state: "keys", keys } : { state: "empty" });
    });
    sock.on("error", (e) => finish({ state: "dead", error: e.message }));
    sock.on("close", () => finish({ state: "dead", error: "agent closed the connection" }));
  });
}

function wellKnownAgentSockets(): string[] {
  return [
    `${homedir()}/Library/Group Containers/2BUA8C4S2C.com.1password/t/agent.sock`,
    `${homedir()}/.1password/agent.sock`,
  ].filter(existsSync);
}

export function agentSocketCandidates(host: string): string[] {
  const fromConfig = parseSSHConfig(host).identityAgent;
  const all = [fromConfig, ...wellKnownAgentSockets(), process.env.SSH_AUTH_SOCK]
    .filter((p): p is string => Boolean(p));
  return [...new Set(all)];
}

export interface LiveAgent {
  socket: string;
  probe: AgentProbe;
}

// Pick the agent socket most likely to complete a signature: one that lists
// keys wins immediately. Otherwise a mute one (a locked 1Password — the probe
// just asked it to unlock) is worth waiting on, since signing can still wake
// it into prompting. An agent that answered with zero keys can never sign, so
// it is never picked over nothing — a dead one can neither sign nor prompt.
export function pickLiveAgent(probed: LiveAgent[]): LiveAgent | undefined {
  return probed.find((p) => p.probe.state === "mute");
}

export async function resolveLiveAgent(host: string): Promise<LiveAgent | undefined> {
  const probed: LiveAgent[] = [];
  for (const socket of agentSocketCandidates(host)) {
    const probe = await probeAgentSocket(socket);
    console.log(
      `[pluk] SSH agent probe: ${socket} -> ${probe.state}${probe.state === "dead" ? ` (${probe.error})` : ""}`
    );
    if (probe.state === "keys") return { socket, probe };
    if (probe.state !== "dead") probed.push({ socket, probe });
  }
  return pickLiveAgent(probed);
}

export function agentUnreachableError(): Error {
  const err = new Error(
    "Can't reach your SSH key agent — no agent socket answered, so no approval prompt can appear. " +
    "Open and unlock 1Password (with its SSH agent enabled), or load the key into ssh-agent, then retry."
  );
  (err as Error & { code: string }).code = SSH_AGENT_UNREACHABLE_CODE;
  return err;
}
