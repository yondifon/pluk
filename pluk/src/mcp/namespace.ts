import type { z } from "zod";
import type {
  CallToolResult,
  GetPromptResult,
  McpServer,
  ReadResourceCallback,
  ResourceMetadata,
  ToolAnnotations,
} from "@modelcontextprotocol/server";

// A group exposes several integrations through one MCP server. Their tool/prompt/
// resource names collide (two SQL DBs both register "query"), so in group mode we
// register each member through a namespaced host that prefixes every name with a
// per-member slug. Single-integration endpoints register on the bare McpServer
// and are unaffected.

type Args<S extends z.ZodRawShape> = z.infer<z.ZodObject<S>>;

/**
 * The subset of the MCP server surface an adapter uses to register its own tools,
 * prompts and resources. Positional by design: the SDK's own registration API is
 * config-object shaped (`registerTool(name, { description, inputSchema }, cb)`),
 * and `toolHost()` below is the single place that translates — so an SDK signature
 * change costs one function here, not an edit in every adapter.
 */
export interface ToolHost {
  tool<S extends z.ZodRawShape>(
    name: string,
    description: string,
    schema: S,
    annotations: ToolAnnotations,
    cb: (args: Args<S>, extra?: unknown) => CallToolResult | Promise<CallToolResult>,
  ): unknown;
  tool<S extends z.ZodRawShape>(
    name: string,
    description: string,
    schema: S,
    cb: (args: Args<S>, extra?: unknown) => CallToolResult | Promise<CallToolResult>,
  ): unknown;
  tool(
    name: string,
    description: string,
    annotations: ToolAnnotations,
    cb: (extra?: unknown) => CallToolResult | Promise<CallToolResult>,
  ): unknown;
  tool(
    name: string,
    description: string,
    cb: (extra?: unknown) => CallToolResult | Promise<CallToolResult>,
  ): unknown;
  prompt<S extends z.ZodRawShape>(
    name: string,
    description: string,
    schema: S,
    cb: (args: Args<S>) => GetPromptResult | Promise<GetPromptResult>,
  ): unknown;
  prompt(
    name: string,
    description: string,
    cb: () => GetPromptResult | Promise<GetPromptResult>,
  ): unknown;
  resource(name: string, uri: string, config: ResourceMetadata, cb: ReadResourceCallback): unknown;
  resource(name: string, uri: string, cb: ReadResourceCallback): unknown;
}

// A `tool()` call passes an optional zod shape and optional annotations before the
// callback, so the middle arguments are told apart by shape: annotations carry only
// the spec's hint keys, a zod shape carries anything else.
const ANNOTATION_KEYS = new Set(["title", "readOnlyHint", "destructiveHint", "idempotentHint", "openWorldHint"]);

function isAnnotations(value: unknown): boolean {
  if (typeof value !== "object" || value === null) return false;
  const keys = Object.keys(value);
  return keys.length > 0 && keys.every((k) => ANNOTATION_KEYS.has(k));
}

/** Adapt a real McpServer to the positional `ToolHost` shape adapters register against. */
export function toolHost(server: McpServer): ToolHost {
  const reg = server as unknown as {
    registerTool: (name: string, config: unknown, cb: unknown) => unknown;
    registerPrompt: (name: string, config: unknown, cb: unknown) => unknown;
    registerResource: (name: string, uri: string, config: unknown, cb: unknown) => unknown;
  };
  return {
    tool: ((name: string, description: string, ...rest: unknown[]) => {
      const cb = rest[rest.length - 1];
      const middle = rest.slice(0, -1);
      const annotations = middle.find(isAnnotations);
      const inputSchema = middle.find((m) => !isAnnotations(m));
      return reg.registerTool(name, { description, inputSchema, annotations }, cb);
    }) as ToolHost["tool"],
    prompt: ((name: string, description: string, schemaOrCb: unknown, maybeCb?: unknown) =>
      typeof schemaOrCb === "function"
        ? reg.registerPrompt(name, { description }, schemaOrCb)
        : reg.registerPrompt(name, { description, argsSchema: schemaOrCb }, maybeCb)) as ToolHost["prompt"],
    resource: ((name: string, uri: string, configOrCb: unknown, maybeCb?: unknown) =>
      typeof configOrCb === "function"
        ? reg.registerResource(name, uri, {}, configOrCb)
        : reg.registerResource(name, uri, configOrCb, maybeCb)) as ToolHost["resource"],
  };
}

/** Slugify a member name into a tool-name-safe prefix segment. */
export function slug(name: string): string {
  return name.toLowerCase().replace(/[^a-z0-9]+/g, "_").replace(/^_+|_+$/g, "") || "member";
}

/** Prefix a resource URI so two members' URIs (e.g. `schema://full`) stay unique. */
function namespaceUri(ns: string, uri: string): string {
  const sep = uri.indexOf("://");
  if (sep === -1) return `${ns}+${uri}`;
  return `${uri.slice(0, sep)}://${ns}/${uri.slice(sep + 3)}`;
}

/**
 * Wrap a host so tool/prompt/resource registrations are prefixed with `ns`.
 * Names become `${ns}__${name}`; resource URIs are namespaced too.
 */
export function namespacedHost(host: ToolHost, ns: string): ToolHost {
  const prefix = (name: string) => `${ns}__${name}`;
  return {
    tool: ((name: string, ...rest: unknown[]) =>
      (host.tool as (...a: unknown[]) => unknown)(prefix(name), ...rest)) as ToolHost["tool"],
    prompt: ((name: string, ...rest: unknown[]) =>
      (host.prompt as (...a: unknown[]) => unknown)(prefix(name), ...rest)) as ToolHost["prompt"],
    resource: ((name: string, uri: string, ...rest: unknown[]) =>
      (host.resource as (...a: unknown[]) => unknown)(prefix(name), namespaceUri(ns, uri), ...rest)) as ToolHost["resource"],
  };
}
