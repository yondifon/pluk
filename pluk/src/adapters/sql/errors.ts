import { isSshPending, isSshStalled } from "../../ssh/pending.js";
import { SSH_AGENT_UNREACHABLE_CODE } from "../../ssh/agent.js";

export type SqlErrorCategory = "auth_failed" | "tunnel_failed" | "query_failed" | "connection_failed" | "pending_approval";

// Distinct from SSH_AGENT_UNREACHABLE: this fires when a live agent answered
// but declined to sign, most often 1Password waiting on an approval it never got.
export const SSH_AGENT_DENIED_CODE = "SSH_AGENT_DENIED";

export interface SqlErrorInfo {
  category: SqlErrorCategory;
  message: string;
  hint?: string;
  // Always present, so callers can branch on it. Driver codes (Postgres
  // SQLSTATE, ECONNREFUSED, …) pass through; otherwise a stable pluk code.
  code: string;
}

export function classifySqlError(err: unknown): SqlErrorInfo {
  const e = err as { message?: string; code?: string };
  const msg = e?.message ?? String(err);
  const code = e?.code;

  if (isSshPending(err)) {
    return {
      category: "pending_approval",
      message: "SSH connection is waiting on an approval.",
      hint: "Approve the 1Password or proxy sign-in prompt, then retry. If none is visible, click Test in Pluk to start a fresh connection.",
      code: code ?? "SSH_CONNECT_PENDING",
    };
  }

  if (isSshStalled(err)) {
    return {
      category: "tunnel_failed",
      message: msg,
      hint: "The stuck attempt was dropped — retry to open a brand-new SSH connection. If it keeps failing, check the host/proxy is reachable and your SSH agent is unlocked.",
      code: code ?? "SSH_CONNECT_STALLED",
    };
  }

  if (code === SSH_AGENT_DENIED_CODE || /agent refused operation|signing failed .* agent/i.test(msg)) {
    return {
      category: "auth_failed",
      message: "Your SSH agent refused to sign.",
      hint: "Check 1Password for a pending approval, or unlock it, then retry.",
      code: SSH_AGENT_DENIED_CODE,
    };
  }

  if (code === SSH_AGENT_UNREACHABLE_CODE || /communication with agent failed|SSH_AUTH_SOCK|open agent|could not open a connection to your authentication agent|No reply from server/i.test(msg)) {
    return {
      category: "auth_failed",
      message: "Can't reach your SSH key agent.",
      hint: "Open and unlock 1Password (with its SSH agent enabled), or load the key into ssh-agent, then retry.",
      code: SSH_AGENT_UNREACHABLE_CODE,
    };
  }

  if (/Permission denied \(publickey\)|no matching (?:host )?key|no mutual signature|All configured authentication methods failed/i.test(msg)) {
    return {
      category: "auth_failed",
      message: "SSH rejected the key.",
      hint: "Check the SSH user and make sure the agent has a key this host accepts.",
      code: code ?? "SSH_KEY_REJECTED",
    };
  }

  if (/connection reset by peer|cloudflared|ProxyCommand exited|did not become ready|unexpected EOF|process exited before tunnel/i.test(msg)) {
    return {
      category: "tunnel_failed",
      message: "SSH proxy connection dropped.",
      hint: "Retry to re-authenticate the proxy session, especially for Cloudflare Access.",
      code: code ?? "SSH_TUNNEL_DROPPED",
    };
  }

  if (code === "28P01" || code === "28000" || /password authentication failed|SASL authentication failed/i.test(msg)) {
    return { category: "auth_failed", message: "Database authentication failed.", hint: "Check username and password.", code: code ?? "DB_AUTH_FAILED" };
  }

  if (code === "3D000" || /database .* does not exist/i.test(msg)) {
    return { category: "connection_failed", message: "Database not found.", hint: "Check the database name.", code: code ?? "DB_NOT_FOUND" };
  }

  if (code === "ECONNREFUSED" || /ECONNREFUSED/i.test(msg)) {
    return {
      category: "connection_failed",
      message: "Connection refused.",
      hint: "Check host, port, firewall, and SSH tunnel config.",
      code: code ?? "ECONNREFUSED",
    };
  }

  if (code === "ENOTFOUND" || /no such host|name or service not known/i.test(msg)) {
    return { category: "connection_failed", message: "Host not found.", hint: "Check the host name.", code: code ?? "ENOTFOUND" };
  }

  if (/self.signed|certificate|\bssl\b|\btls\b/i.test(msg)) {
    return { category: "connection_failed", message: "SSL error.", hint: "Check SSL mode and certificates.", code: code ?? "SSL_ERROR" };
  }

  if (/timed out|connection timeout|timeout expired/i.test(msg)) {
    return {
      category: "connection_failed",
      message: "Timed out.",
      hint: "Check host, port, SSH tunnel, and firewall/VPC rules.",
      code: code ?? "TIMEOUT",
    };
  }

  if (/no usable private key|cannot parse privatekey|encrypted.*passphrase|bad passphrase/i.test(msg)) {
    return {
      category: "auth_failed",
      message: "SSH key problem.",
      hint: "Check key path and passphrase.",
      code: code ?? "SSH_KEY_INVALID",
    };
  }

  if (/host key|hostkey/i.test(msg)) {
    return { category: "auth_failed", message: "SSH host key was rejected.", code: code ?? "SSH_HOST_KEY_REJECTED" };
  }

  return { category: "query_failed", message: msg, code: code ?? "QUERY_FAILED" };
}

export function humanizeSqlError(err: unknown): string {
  const info = classifySqlError(err);
  return info.hint ? `${info.message} ${info.hint}` : `${info.message} (see Logs for details)`;
}

export function formatSqlError(err: unknown): string {
  return `Error: ${JSON.stringify({ error: classifySqlError(err) }, null, 2)}`;
}
