import { isSshPending } from "../../ssh/pending.js";

export type SqlErrorCategory = "auth_failed" | "tunnel_failed" | "query_failed" | "connection_failed" | "pending_approval";

export interface SqlErrorInfo {
  category: SqlErrorCategory;
  message: string;
  hint?: string;
  code?: string;
}

export function classifySqlError(err: unknown): SqlErrorInfo {
  const e = err as { message?: string; code?: string };
  const msg = e?.message ?? String(err);
  const code = e?.code;

  if (isSshPending(err)) {
    return {
      category: "pending_approval",
      message: "SSH connection is waiting for approval.",
      hint: "Approve the 1Password/SSH agent prompt (or finish the proxy login), then retry. If no prompt appears, click Test in Pluk to force a fresh connection.",
      code,
    };
  }

  if (/communication with agent failed|agent refused operation|signing failed .* agent|SSH_AUTH_SOCK|open agent|could not open a connection to your authentication agent|No reply from server/i.test(msg)) {
    return {
      category: "auth_failed",
      message: "Can't reach your SSH key agent.",
      hint: "Unlock 1Password or load the SSH key into ssh-agent, then retry.",
      code,
    };
  }

  if (/Permission denied \(publickey\)|no matching (?:host )?key|no mutual signature|All configured authentication methods failed/i.test(msg)) {
    return {
      category: "auth_failed",
      message: "SSH rejected the key.",
      hint: "Check the SSH user and make sure the agent has a key this host accepts.",
      code,
    };
  }

  if (/connection reset by peer|cloudflared|ProxyCommand exited|did not become ready|unexpected EOF|process exited before tunnel/i.test(msg)) {
    return {
      category: "tunnel_failed",
      message: "SSH proxy connection dropped.",
      hint: "Retry to re-authenticate the proxy session, especially for Cloudflare Access.",
      code,
    };
  }

  if (code === "28P01" || code === "28000" || /password authentication failed|SASL authentication failed/i.test(msg)) {
    return { category: "auth_failed", message: "Database authentication failed.", hint: "Check username and password.", code };
  }

  if (code === "3D000" || /database .* does not exist/i.test(msg)) {
    return { category: "connection_failed", message: "Database not found.", hint: "Check the database name.", code };
  }

  if (code === "ECONNREFUSED" || /ECONNREFUSED/i.test(msg)) {
    return {
      category: "connection_failed",
      message: "Connection refused.",
      hint: "Check host, port, firewall, and SSH tunnel config.",
      code,
    };
  }

  if (code === "ENOTFOUND" || /no such host|name or service not known/i.test(msg)) {
    return { category: "connection_failed", message: "Host not found.", hint: "Check the host name.", code };
  }

  if (/self.signed|certificate|\bssl\b|\btls\b/i.test(msg)) {
    return { category: "connection_failed", message: "SSL error.", hint: "Check SSL mode and certificates.", code };
  }

  if (/timed out|connection timeout|timeout expired/i.test(msg)) {
    return {
      category: "connection_failed",
      message: "Timed out.",
      hint: "Check host, port, SSH tunnel, and firewall/VPC rules.",
      code,
    };
  }

  if (/no usable private key|cannot parse privatekey|encrypted.*passphrase|bad passphrase/i.test(msg)) {
    return {
      category: "auth_failed",
      message: "SSH key problem.",
      hint: "Check key path and passphrase.",
      code,
    };
  }

  if (/host key|hostkey/i.test(msg)) {
    return { category: "auth_failed", message: "SSH host key was rejected.", code };
  }

  return { category: "query_failed", message: msg, code };
}

export function humanizeSqlError(err: unknown): string {
  const info = classifySqlError(err);
  return info.hint ? `${info.message} ${info.hint}` : `${info.message} (see Logs for details)`;
}

export function formatSqlError(err: unknown): string {
  return `Error: ${JSON.stringify({ error: classifySqlError(err) }, null, 2)}`;
}
