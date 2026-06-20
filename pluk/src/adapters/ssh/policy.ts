// Command guardrail for the SSH adapter.
//
// Security model: ALLOWLIST, not denylist. A denylist is unwinnable — shell
// metacharacters, command substitution, base64 pipes, and endless aliases give
// infinite ways to phrase a destructive command. So nothing runs unless it is
// explicitly recognized as safe:
//
//   1. Reject any shell metacharacter that enables chaining, redirection,
//      command substitution, or file creation ( ; && || & ` $( ${ > >> < ).
//      Pipes are allowed but every pipeline segment is validated independently.
//   2. The binary (per segment) must be in ALLOW.
//   3. Per-binary subcommand rules: e.g. `docker compose up` is fine,
//      `docker compose down` / `docker exec` are not.
//   4. No argument may reference a sensitive file (.env, private keys, ~/.ssh,
//      ~/.aws, /etc/shadow, …).
//
// THREAT MODEL — read this before trusting it. This guardrail is a strong gate
// against *accidental* and *casual* destructive/sensitive operations and enforces
// a tool allowlist. It is NOT a hard security boundary against an adversary who
// fully controls the command string (e.g. a prompt-injected agent). Static string
// analysis cannot see through the remote shell's own expansions: globbing
// (`cat .e*`) and variable expansion (`$IFS`) can still reach files the sensitive
// scan names. The filename-based rules below DO catch the headline cases (.env,
// private keys) regardless of path. For a true boundary, run as a restricted
// remote user with filesystem permissions. See code-review-pluk.md.
//
// This is a draft: conservative by default. Widen ALLOW as real workflows need.

export type CommandCategory = "read" | "write";

export interface CommandVerdict {
  ok: boolean;
  category: CommandCategory;
  reason?: string;
}

interface BinRule {
  /** If set, only these subcommands (first non-flag arg) are allowed. */
  subAllow?: string[];
  /** Subcommands that are state-changing (gated as "write"). */
  writeSubs?: string[];
  /** Flags/args that escape the sandbox and are never allowed for this binary. */
  forbidArgs?: string[];
  /** Whole command counts as write regardless of subcommand. */
  write?: boolean;
}

// Safe, read-only system/inspection binaries. Most take file/path args, so the
// sensitive-file scan (below) still applies to all of them.
const PLAIN_READ = [
  "ls", "pwd", "whoami", "hostname", "uptime", "date", "uname", "id", "w", "who",
  "df", "du", "free", "arch", "nproc", "lsb_release", "stat", "file", "readlink",
  "realpath", "tree", "cat", "head", "tail", "less", "more", "grep", "egrep",
  "fgrep", "zgrep", "zcat", "wc", "cut", "sort", "uniq", "column", "nl", "tac",
  "ps", "top", "htop", "vmstat", "iostat", "mpstat", "lsof", "ss", "netstat",
  "dmesg", "echo", "printf", "basename", "dirname",
];
// Deliberately excluded: env/printenv (leak secrets), curl/wget (exfil), and any
// shell/interpreter (sh, bash, python, perl, …) — they defeat the allowlist.

const ALLOW: Record<string, BinRule> = {
  ...Object.fromEntries(PLAIN_READ.map((b) => [b, {} as BinRule])),

  // find: useful but -exec/-delete turn it into an arbitrary mutator.
  find: { forbidArgs: ["-exec", "-execdir", "-delete", "-fprint", "-fprintf", "-ok", "-okdir"] },

  // tail/journalctl: -f/--follow never returns, so it would just burn the exec
  // timeout. Forbid follow; everything else is a normal bounded read.
  tail: { forbidArgs: ["-f", "--follow", "-F"] },
  journalctl: { forbidArgs: ["-f", "--follow"] },

  // Container ops: inspection + compose lifecycle, but nothing that grants a
  // shell (exec/run) or destroys state (rm/down/kill/prune).
  docker: {
    // Leaf read commands. Grouped commands (system/volume/image/container/…) are
    // NOT here — they have destructive nested verbs (prune/rm/stop) and are
    // checked separately below against DOCKER_GROUP_READ.
    subAllow: [
      "ps", "images", "logs", "inspect", "stats", "top",
      "version", "info", "port", "diff", "history", "compose",
      "system", "volume", "image", "container", "network", "node", "service",
    ],
    writeSubs: [],
  },
  "docker-compose": {
    subAllow: ["ps", "ls", "logs", "config", "top", "images", "version", "port", "up", "start", "restart"],
    writeSubs: ["up", "start", "restart"],
  },

  // Service status — read only. start/stop/restart/enable/disable are excluded.
  systemctl: {
    subAllow: ["status", "is-active", "is-enabled", "is-failed", "list-units", "list-unit-files", "show", "cat", "get-default"],
  },

  // Git inspection. No push/pull/fetch/commit/checkout/reset/clean.
  git: {
    subAllow: ["status", "log", "diff", "show", "branch", "remote", "describe", "rev-parse", "tag", "blame", "shortlog"],
  },

  // Kubernetes inspection. No mutating verbs, no exec, no `config view` (leaks
  // cluster credentials/tokens).
  kubectl: {
    subAllow: ["get", "describe", "logs", "top", "version", "api-resources", "cluster-info", "explain"],
  },
};

// Nested allowlist for `docker compose <sub>` (compose as a docker subcommand).
const DOCKER_COMPOSE_SUB = ALLOW["docker-compose"]!;

// Grouped docker commands (`docker <group> <verb>`) where only read verbs are
// safe — e.g. `docker volume ls` ✓ but `docker volume rm`/`docker system prune` ✗.
const DOCKER_GROUPS = new Set(["system", "volume", "image", "container", "network", "node", "service"]);
const DOCKER_GROUP_READ = new Set(["ls", "ps", "inspect", "df", "info", "logs", "top", "stats", "history", "port", "diff", "version", "list"]);

// Sensitive files: reading these would leak secrets/keys. Matched against every
// argument token (so cat/grep/tail/less of them is blocked).
const SENSITIVE: RegExp[] = [
  /(^|\/)\.env(\.[\w-]+)?$/i,           // .env, .env.local, .env.production
  /(^|\/)\.env\//i,
  /\bid_(rsa|ed25519|ecdsa|dsa)\b/i,
  /\.(pem|key|p12|pfx|keystore|jks)$/i,
  /(^|\/)\.ssh(\/|$)/i,
  /(^|\/)\.aws(\/|$)/i,
  /(^|\/)\.gnupg(\/|$)/i,
  /(^|\/)\.netrc$/i,
  /(^|\/)\.npmrc$/i,
  /\/etc\/(shadow|gshadow|sudoers)\b/i,
  /(^|\/)credentials$/i,
];

// Shell metacharacters that enable chaining, redirection, substitution, or file
// creation. Lone `|` (pipe) is handled separately and is NOT in this set.
function hasForbiddenMeta(s: string): string | null {
  if (s.includes("||")) return "||";
  if (/[;&`<>]/.test(s)) return (s.match(/[;&`<>]/) || [])[0] ?? "metacharacter";
  if (s.includes("$(")) return "$(";
  if (s.includes("${")) return "${";
  if (/[\n\r]/.test(s)) return "newline";
  // Brace expansion with a comma (`{a,b}`) would let `cat {.env,x}` reach files
  // the sensitive scan names. `{{.X}}` (docker --format) has no comma, so it's
  // unaffected.
  if (/\{[^{}]*,[^{}]*\}/.test(s)) return "{,}";
  return null;
}

// Whitespace tokenizer that respects single/double quotes. Quotes are stripped
// from the returned tokens.
function tokenize(segment: string): string[] {
  const out: string[] = [];
  let cur = "";
  let quote: "'" | '"' | null = null;
  let has = false;
  for (const ch of segment) {
    if (quote) {
      if (ch === quote) quote = null;
      else cur += ch;
      continue;
    }
    if (ch === "'" || ch === '"') { quote = ch; has = true; continue; }
    if (/\s/.test(ch)) { if (has || cur) { out.push(cur); cur = ""; has = false; } continue; }
    cur += ch; has = true;
  }
  if (has || cur) out.push(cur);
  return out;
}

function firstSubcommand(args: string[]): string | undefined {
  return args.find((a) => !a.startsWith("-"));
}

function checkSensitive(tokens: string[]): string | null {
  for (const t of tokens) {
    if (SENSITIVE.some((re) => re.test(t))) return t;
  }
  return null;
}

// Validate one pipeline segment. Returns its category or an error reason.
function checkSegment(segment: string): { category: CommandCategory } | { reason: string } {
  const trimmed = segment.trim();
  if (!trimmed) return { reason: "empty command segment" };

  const tokens = tokenize(trimmed);
  if (tokens.length === 0) return { reason: "empty command" };

  const bin = tokens[0]!.split("/").pop()!; // basename, so /usr/bin/ls -> ls
  const rule = ALLOW[bin];
  if (!rule) return { reason: `command not allowed: "${bin}"` };

  const args = tokens.slice(1);

  const sensitive = checkSensitive(tokens);
  if (sensitive) return { reason: `access to sensitive path is blocked: "${sensitive}"` };

  if (rule.forbidArgs) {
    const bad = args.find((a) => rule.forbidArgs!.includes(a));
    if (bad) return { reason: `flag not allowed for "${bin}": "${bad}"` };
  }

  // docker: handle the nested cases (compose + grouped commands) before the flat
  // subAllow check, since their second token decides read vs. destructive.
  if (bin === "docker") {
    const sub = firstSubcommand(args);
    if (sub === "compose") {
      const composeArgs = args.slice(args.indexOf("compose") + 1);
      const csub = firstSubcommand(composeArgs);
      if (!csub || !DOCKER_COMPOSE_SUB.subAllow!.includes(csub)) {
        return { reason: `docker compose subcommand not allowed: "${csub ?? "(none)"}"` };
      }
      return { category: DOCKER_COMPOSE_SUB.writeSubs!.includes(csub) ? "write" : "read" };
    }
    if (sub && DOCKER_GROUPS.has(sub)) {
      const groupArgs = args.slice(args.indexOf(sub) + 1);
      const verb = firstSubcommand(groupArgs);
      if (!verb || !DOCKER_GROUP_READ.has(verb)) {
        return { reason: `docker ${sub} verb not allowed: "${verb ?? "(none)"}" (read-only verbs only)` };
      }
      return { category: "read" };
    }
  }

  if (rule.subAllow) {
    const sub = firstSubcommand(args);
    if (!sub || !rule.subAllow.includes(sub)) {
      return { reason: `subcommand not allowed for "${bin}": "${sub ?? "(none)"}"` };
    }
    return { category: rule.writeSubs?.includes(sub) ? "write" : "read" };
  }

  return { category: rule.write ? "write" : "read" };
}

/**
 * Evaluate a raw command string against the allowlist. Splits on pipes and
 * validates each segment; the command is allowed only if every segment is.
 * Category is "write" if any segment is state-changing, else "read".
 */
export function evaluateCommand(raw: string): CommandVerdict {
  const command = raw.trim();
  if (!command) return { ok: false, category: "read", reason: "empty command" };
  if (command.length > 4000) return { ok: false, category: "read", reason: "command too long" };

  const meta = hasForbiddenMeta(command);
  if (meta) {
    return { ok: false, category: "read", reason: `shell metacharacter not allowed: "${meta}". Chaining, redirection, and command substitution are blocked.` };
  }

  const segments = command.split("|");
  let category: CommandCategory = "read";
  for (const seg of segments) {
    const res = checkSegment(seg);
    if ("reason" in res) return { ok: false, category: "read", reason: res.reason };
    if (res.category === "write") category = "write";
  }
  return { ok: true, category };
}

/** Human-readable summary of what may run, for tool descriptions / agents. */
export function policySummary(): string {
  const bins = Object.keys(ALLOW).sort();
  return [
    "Allowed (allowlist only): " + bins.join(", ") + ".",
    "docker: inspection + `docker compose up/start/restart/ps/logs/config` — never exec/run/rm/down/kill/prune.",
    "No shell chaining/redirection/substitution (; && || & ` $() > <). Pipes are allowed.",
    "Reading sensitive files (.env, private keys, ~/.ssh, ~/.aws, /etc/shadow, …) is blocked.",
  ].join("\n");
}

/** Sanitize a working directory. Returns null if it contains anything unsafe. */
export function sanitizeWorkingDir(dir: string): string | null {
  if (!dir) return null;
  if (hasForbiddenMeta(dir)) return null;
  if (/[|'"\s]/.test(dir)) return null;
  if (!/^[\w./@~-]+$/.test(dir)) return null;
  if (SENSITIVE.some((re) => re.test(dir))) return null;
  return dir;
}
