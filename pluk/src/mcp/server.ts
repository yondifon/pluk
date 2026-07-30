import { createMcpHandler, type McpHttpHandler, type McpServer } from "@modelcontextprotocol/server";
import { openOwner, closeOwner } from "./pool.js";
import { logError } from "../log.js";

// MCP HTTP entry, protocol revision 2026-07-28. The revision is stateless: no
// initialize handshake, no Mcp-Session-Id, one fresh server per request built by
// the factory. `legacy: "reject"` makes this endpoint modern-only — a 2025-era
// client is answered with the unsupported-protocol-version error naming what we
// serve, rather than silently getting a different protocol.
//
// Target-agnostic: the caller supplies a factory that builds the McpServer (a
// single integration's adapter server, or a group's aggregated server). Long-lived
// resources (driver pools, SSH tunnels, forwards) are keyed by owner id in pool.ts,
// since a stateless request carries no identity of its own.

interface Owner {
  handler: McpHttpHandler;
  // Replaced on every request so each per-request server is built from current DB
  // state: the handler is cached per owner, the config it bakes in is not.
  makeServer: ServerFactory;
}

const owners = new Map<string, Owner>();

/** Build the MCP server for one request. */
export type ServerFactory = () => McpServer;

/**
 * Drop an owner's pooled resources so credential/config edits take effect: aborts
 * in-flight calls and evicts the adapter-owned drivers, tunnels and forwards keyed
 * to it (a standalone integration and the same integration inside a group are
 * different owners → different pools → isolated connections). With `ownerId`,
 * resets only that integration/group; without it, resets all. Returns the count.
 */
export async function resetOwners(ownerId?: string): Promise<number> {
  const ids = [...owners.keys()].filter((id) => !ownerId || id === ownerId);
  for (const id of ids) {
    const owner = owners.get(id);
    owners.delete(id);
    closeOwner(id); // abort in-flight calls + notify adapter-owned pools
    try { await owner?.handler.close(); } catch { /* best-effort */ }
  }
  return ids.length;
}

export async function handleMcpRequest(req: Request, ownerId: string, makeServer: ServerFactory): Promise<Response> {
  let owner = owners.get(ownerId);
  if (owner) {
    owner.makeServer = makeServer;
  } else {
    const created: Owner = {
      makeServer,
      handler: createMcpHandler(() => created.makeServer(), {
        legacy: "reject",
        onerror: (err) => logError("MCP request failed", err, { ownerId }),
      }),
    };
    owners.set(ownerId, created);
    owner = created;
  }

  openOwner(ownerId);

  try {
    return await owner.handler.fetch(req);
  } catch (err) {
    logError("MCP request failed", err, { ownerId });
    return new Response("MCP request failed", { status: 500 });
  }
}
