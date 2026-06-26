import type { Integration } from "../../store/integrations.js";
import { listSavedCommands, createSavedCommand, deleteSavedCommand } from "../../store/savedCommands.js";

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

export async function handleSshApi(conn: Integration, req: Request, subpath: string): Promise<Response | null> {
  const savedMatch = subpath.match(/^\/saved_commands(?:\/([^/]+))?$/);
  if (!savedMatch) return null;

  const savedName = savedMatch[1] ? decodeURIComponent(savedMatch[1]) : undefined;

  if (req.method === "GET") {
    return Response.json({ ok: true, commands: listSavedCommands(conn.id) });
  }

  if (req.method === "POST") {
    const body = await readJsonBody<{ name?: string; command?: string; working_dir?: string }>(req);
    if (!body) return Response.json({ ok: false, error: "Invalid JSON body" }, { status: 400 });
    if (!body.name?.trim() || !body.command?.trim()) {
      return Response.json({ ok: false, error: "name and command required" }, { status: 400 });
    }
    try {
      const command = createSavedCommand({
        connection_id: conn.id,
        name: body.name.trim(),
        command: body.command,
        working_dir: body.working_dir?.trim() || null,
      });
      return Response.json({ ok: true, command });
    } catch (err) {
      return conflictOrThrow(err, "A saved command with that name already exists.");
    }
  }

  if (req.method === "DELETE" && savedName) {
    return Response.json({ ok: deleteSavedCommand(conn.id, savedName) });
  }

  return new Response("Method not allowed", { status: 405 });
}
