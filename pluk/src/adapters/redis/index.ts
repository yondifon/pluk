import { z } from "zod";
import type { Integration } from "../../store/integrations.js";
import { actionAdapter, type ActionTool } from "../kit.js";
import { redisFields } from "./fields.js";
import { redisAccessor, raw, testRedis, type RedisAccessor } from "./client.js";

const AGENT_HINT = "Use this to inspect and edit a Redis datastore — list keys, read values, types and TTLs, check server INFO, and set, expire or delete keys. Use scan (not keys) to list keys safely; get/type/ttl inspect a single key.";

// Redis tools. Each declares its policy category, log line, and command; gating,
// logging, and response shaping are handled by actionAdapter. The accessor opens
// the connection (and SSH tunnel, if configured) lazily on first use and reuses it
// across the session's calls.
function redisTools(acc: RedisAccessor): ActionTool[] {
  return [
    {
      name: "scan",
      description: "Incrementally list keys with SCAN (safe on large keyspaces). Returns a cursor and a page of keys.",
      category: "read",
      schema: {
        cursor: z.string().default("0").describe("Cursor from a previous scan; start at 0"),
        match: z.string().optional().describe("Glob pattern, e.g. user:*"),
        count: z.number().int().min(1).max(10000).default(100).describe("Approximate keys to scan per call"),
      },
      detail: (a) => `scan match=${(a.match as string) ?? "*"} count=${a.count}`,
      run: async (a) => {
        const args: (string | number)[] = [a.cursor as string];
        if (a.match) args.push("MATCH", a.match as string);
        args.push("COUNT", a.count as number);
        const [cursor, keys] = (await raw(await acc.get(), "SCAN", args)) as [string, string[]];
        return { cursor, keys };
      },
    },
    {
      name: "keys",
      description: "List keys matching a pattern with KEYS. Blocks the server on large keyspaces — prefer scan.",
      category: "read",
      defaultEnabled: false,
      schema: { pattern: z.string().default("*").describe("Glob pattern, e.g. user:*") },
      detail: (a) => `keys ${a.pattern}`,
      run: async (a) => raw(await acc.get(), "KEYS", [a.pattern as string]),
    },
    {
      name: "get",
      description: "Get the string value of a key.",
      category: "read",
      schema: { key: z.string().describe("Key name") },
      detail: (a) => `get ${a.key}`,
      run: async (a) => (await acc.get()).get(a.key as string),
    },
    {
      name: "type",
      description: "Get the data type of a key (string/list/set/zset/hash/stream).",
      category: "read",
      schema: { key: z.string().describe("Key name") },
      detail: (a) => `type ${a.key}`,
      run: async (a) => raw(await acc.get(), "TYPE", [a.key as string]),
    },
    {
      name: "ttl",
      description: "Get the remaining time-to-live of a key in seconds (-1 no expiry, -2 missing).",
      category: "read",
      schema: { key: z.string().describe("Key name") },
      detail: (a) => `ttl ${a.key}`,
      run: async (a) => (await acc.get()).ttl(a.key as string),
    },
    {
      name: "info",
      description: "Get server INFO, optionally for a single section (e.g. memory, clients, stats).",
      category: "read",
      defaultEnabled: false,
      schema: { section: z.string().optional().describe("INFO section, e.g. memory") },
      detail: (a) => `info ${(a.section as string) ?? "all"}`,
      run: async (a) => raw(await acc.get(), "INFO", a.section ? [a.section as string] : []),
    },

    // ── Write tools ──────────────────────────────────────────────────────────
    {
      name: "set",
      description: "Set a key's string value, optionally with an expiry in seconds.",
      category: "write",
      schema: {
        key: z.string().describe("Key name"),
        value: z.string().describe("String value"),
        ex: z.number().int().min(1).optional().describe("Expiry in seconds (optional)"),
      },
      detail: (a) => `set ${a.key}${a.ex ? ` (ex=${a.ex})` : ""}`,
      run: async (a) => {
        const client = await acc.get();
        const result = await client.set(a.key as string, a.value as string);
        if (a.ex) await client.expire(a.key as string, a.ex as number);
        return result;
      },
    },
    {
      name: "expire",
      description: "Set a key's time-to-live in seconds.",
      category: "write",
      schema: { key: z.string().describe("Key name"), seconds: z.number().int().min(1).describe("TTL in seconds") },
      detail: (a) => `expire ${a.key} ${a.seconds}`,
      run: async (a) => (await acc.get()).expire(a.key as string, a.seconds as number),
    },
    {
      name: "del",
      description: "Delete a key.",
      category: "delete",
      schema: { key: z.string().describe("Key name") },
      detail: (a) => `del ${a.key}`,
      run: async (a) => (await acc.get()).del(a.key as string),
    },
  ];
}

export const redisAdapter = actionAdapter<RedisAccessor>({
  id: "redis",
  label: "Redis",
  category: "datastore",
  agentHint: AGENT_HINT,
  access:
    "Read keys and values (scan/get/type/ttl/info); set/expire/delete when write is permitted. Every action is policy-checked and recorded in the activity log.",
  start: "scan",
  configFields: redisFields,
  client: (conn, sessionIdRef) => redisAccessor(conn, sessionIdRef),
  testConnection: (conn: Integration) => testRedis(conn),
  tools: (_conn, acc) => redisTools(acc),
});
