// Turn a raw driver/SSH/network error into one short, actionable sentence for
// the UI. The full technical detail still goes to the log (logError) — this is
// the human-facing line shown on a connection's status and the test result, so
// a failure reads as "what do I do" rather than a stack-trace fragment.

export function humanizeConnError(err: unknown): string {
  const e = err as { message?: string; code?: string };
  const msg = e?.message ?? String(err);
  const code = e?.code;

  // SSH key agent (1Password, ssh-agent) — unreachable or locked. This is the
  // common "it worked yesterday" failure: the agent holds the key but won't
  // sign because the app is locked or not running. Name the fix.
  if (/communication with agent failed|agent refused operation|signing failed .* agent|SSH_AUTH_SOCK|open agent|could not open a connection to your authentication agent/i.test(msg)) {
    return "Can't reach your SSH key agent. If you use 1Password, make sure it's unlocked (and SSH agent enabled), then retry.";
  }
  if (/Permission denied \(publickey\)|no matching (?:host )?key|no mutual signature/i.test(msg)) {
    return "SSH rejected the key. The agent has no key this host accepts — if you use 1Password, unlock it and check the key is enabled for this host.";
  }

  // Authentication — Postgres (28P01/28000) and PgBouncer (reports a bad SCRAM
  // password as a protocol violation, "SASL authentication failed").
  if (code === "28P01" || code === "28000" || /password authentication failed|SASL authentication failed/i.test(msg)) {
    return "Authentication failed — check the username and password.";
  }

  if (code === "3D000" || /database .* does not exist/i.test(msg)) {
    return "Database not found — check the database name.";
  }

  if (code === "ECONNREFUSED" || /ECONNREFUSED/i.test(msg)) {
    return "Connection refused — check the host and port, and that the server is reachable (through the SSH tunnel, if used).";
  }
  if (code === "ENOTFOUND" || /no such host|name or service not known/i.test(msg)) {
    return "Host not found — check the host name. If using an SSH proxy (cloudflared), this is often transient — try again.";
  }

  // Cloudflare Access / ProxyCommand session expiry or a dropped tunnel — the
  // proxy binary exits or the forwarded socket dies mid-handshake.
  if (/connection reset by peer|cloudflared|ProxyCommand exited|did not become ready|unexpected EOF|process exited before tunnel|No reply from server/i.test(msg)) {
    return "SSH proxy connection dropped. If this host uses Cloudflare Access, your session may have expired — retry to re-authenticate.";
  }

  if (/self.signed|certificate|\bssl\b|\btls\b/i.test(msg)) {
    return "SSL error — try a different SSL mode, or disable SSL if the server doesn't require it.";
  }

  if (/timed out|connection timeout|timeout expired/i.test(msg)) {
    return "Timed out — the server didn't respond. Check the host/port, the SSH tunnel, and any firewall/VPC rules.";
  }

  if (/All configured authentication methods failed/i.test(msg)) {
    return "SSH authentication failed — check the user and that your key or agent is set up for this host.";
  }
  if (/no usable private key|cannot parse privatekey|encrypted.*passphrase|bad passphrase/i.test(msg)) {
    return "SSH key problem — the key couldn't be read or the passphrase is wrong. Check the key path and passphrase.";
  }
  if (/host key|hostkey/i.test(msg)) {
    return "SSH host key was rejected by the server.";
  }

  // Unknown — keep it short and point at the log for the full trace.
  return `${msg} (see Logs for details)`;
}
