import type { Integration } from "../../store/integrations.js";
import { listSavedQueries, createSavedQuery, deleteSavedQuery } from "../../store/savedQueries.js";
import { listMaskedColumns, addMaskedColumn, removeMaskedColumn } from "../../store/maskedColumns.js";
import { cancelQuery } from "./pool.js";

async function readJsonBody<T>(req: Request): Promise<T | null> {
  try {
    return (await req.json()) as T;
  } catch {
    return null;
  }
}

function conflictOrThrow(err: unknown, message: string): Response {
  if (/UNIQUE constraint/i.test((err as Error).message)) {
    return Response.json({ ok: false, error: message }, { status: 409 });
  }
  throw err;
}

export async function handleSqlApi(conn: Integration, req: Request, subpath: string): Promise<Response | null> {
  const savedMatch = subpath.match(/^\/saved_queries(?:\/([^/]+))?$/);
  if (savedMatch) {
    const savedName = savedMatch[1] ? decodeURIComponent(savedMatch[1]) : undefined;

    if (req.method === "GET") {
      return Response.json({ ok: true, queries: listSavedQueries(conn.id) });
    }

    if (req.method === "POST") {
      const body = await readJsonBody<{ name?: string; sql?: string }>(req);
      if (!body) return Response.json({ ok: false, error: "Invalid JSON body" }, { status: 400 });
      if (!body.name?.trim() || !body.sql?.trim()) {
        return Response.json({ ok: false, error: "name and sql required" }, { status: 400 });
      }
      try {
        const query = createSavedQuery({ connection_id: conn.id, name: body.name.trim(), sql: body.sql });
        return Response.json({ ok: true, query });
      } catch (err) {
        return conflictOrThrow(err, "A saved query with that name already exists.");
      }
    }

    if (req.method === "DELETE" && savedName) {
      return Response.json({ ok: deleteSavedQuery(conn.id, savedName) });
    }

    return new Response("Method not allowed", { status: 405 });
  }

  const maskMatch = subpath.match(/^\/masked_columns(?:\/([^/]+))?$/);
  if (maskMatch) {
    const columnName = maskMatch[1] ? decodeURIComponent(maskMatch[1]) : undefined;

    if (req.method === "GET") {
      return Response.json({ ok: true, columns: listMaskedColumns(conn.id) });
    }

    if (req.method === "POST") {
      const body = await readJsonBody<{ column_name?: string }>(req);
      if (!body) return Response.json({ ok: false, error: "Invalid JSON body" }, { status: 400 });
      if (!body.column_name?.trim()) {
        return Response.json({ ok: false, error: "column_name required" }, { status: 400 });
      }
      const column = addMaskedColumn(conn.id, body.column_name.trim());
      return Response.json({ ok: true, column });
    }

    if (req.method === "DELETE" && columnName) {
      return Response.json({ ok: removeMaskedColumn(conn.id, columnName) });
    }

    return new Response("Method not allowed", { status: 405 });
  }

  return null;
}

export function handleSqlLogApi(req: Request, path: string): Response | null {
  const cancelId = path.match(/^\/api\/log\/(\d+)\/cancel$/)?.[1];
  if (!cancelId || req.method !== "POST") return null;
  return Response.json({ ok: cancelQuery(Number(cancelId)) });
}
