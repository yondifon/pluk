import type { Client } from "ssh2";
import type { Driver, RelationshipInfo, SSHConfig, TableStats } from "./index.js";
import { evictSharedSSHClient, getSharedSSHClient, type SSHParams } from "../ssh/client.js";
import { isSshPending } from "../ssh/pending.js";

const DEFAULT_TIMEOUT_MS = 30_000;
const MAX_OUTPUT_BYTES = 1_000_000;

function shellQuote(value: string): string {
  return `'${value.replace(/'/g, `'\\''`)}'`;
}

function quoteIdent(value: string): string {
  return `"${value.replace(/"/g, '""')}"`;
}

function sqlQuote(value: string): string {
  return `'${value.replace(/'/g, "''")}'`;
}

function sshParams(cfg: SSHConfig): SSHParams {
  if (!cfg.ssh_host) throw new Error("SQLite SSH host is missing. Set it in the connection settings.");
  return {
    host: cfg.ssh_host,
    port: cfg.ssh_port ?? 22,
    user: cfg.ssh_user ?? "",
    authType: cfg.ssh_auth_type ?? "agent",
    keyPath: cfg.ssh_key_path,
    password: cfg.ssh_password,
  };
}

function exec(client: Client, command: string, timeoutMs: number): Promise<string> {
  return new Promise((resolve, reject) => {
    client.exec(command, (err, stream) => {
      if (err) return reject(err);
      let stdout = "";
      let stderr = "";
      let captured = 0;
      const timer = setTimeout(() => {
        stream.close();
        reject(new Error(`Command timed out after ${Math.round(timeoutMs / 1000)}s`));
      }, timeoutMs);

      const append = (current: string, chunk: Buffer): string => {
        const remaining = MAX_OUTPUT_BYTES - captured;
        if (remaining <= 0) return current;
        const kept = chunk.length > remaining ? chunk.subarray(0, remaining) : chunk;
        captured += kept.length;
        if (captured >= MAX_OUTPUT_BYTES) stream.close();
        return current + kept.toString();
      };

      stream.on("data", (chunk: Buffer) => { stdout = append(stdout, chunk); });
      stream.stderr.on("data", (chunk: Buffer) => { stderr = append(stderr, chunk); });
      stream.on("close", (code: number | null) => {
        clearTimeout(timer);
        if (code === 0) resolve(stdout);
        else reject(new Error((stderr || stdout || `sqlite3 exited with code ${code}`).trim()));
      });
      stream.on("error", (e: Error) => { clearTimeout(timer); reject(e); });
    });
  });
}

export function createRemoteSqliteDriver(cfg: SSHConfig, sessionId: string): Driver {
  const params = sshParams(cfg);
  const filename = cfg.filename;
  if (!filename) throw new Error("SQLite path is missing. Set the remote database file path.");
  const remotePath = filename;

  async function runJson(sql: string, timeoutMs = DEFAULT_TIMEOUT_MS, readonly = false): Promise<Record<string, unknown>[]> {
    const client = await getSharedSSHClient(sessionId, params);
    // -readonly opens the database file read-only, so the sqlite3 process itself
    // refuses any write — the engine-level guard for read-only connections.
    const command = `sqlite3 ${readonly ? "-readonly " : ""}-json ${shellQuote(remotePath)} ${shellQuote(sql)}`;
    try {
      const output = (await exec(client, command, timeoutMs)).trim();
      return output ? JSON.parse(output) as Record<string, unknown>[] : [];
    } catch (err) {
      // A connect awaiting an interactive approval stays pooled — evicting it
      // would close the connection the moment the user approves.
      if (!isSshPending(err)) evictSharedSSHClient(sessionId, params);
      throw err;
    }
  }

  // The remote driver shells out to `sqlite3`, which has no bind-parameter
  // channel, so params can't be sent safely (inlining would be an injection
  // vector). Reject them instead of silently dropping and returning wrong rows.
  function rejectParams(params?: unknown[]): void {
    if (params && params.length > 0) {
      throw new Error("Bind parameters are not supported for SQLite over SSH. Inline literal values instead.");
    }
  }

  return {
    async query(sql, params = [], opts) {
      rejectParams(params);
      const rows = await runJson(sql, opts?.timeoutMs);
      return { rows, fields: rows[0] ? Object.keys(rows[0]) : undefined };
    },

    async queryReadOnly(sql, params = [], opts) {
      rejectParams(params);
      const rows = await runJson(sql, opts?.timeoutMs, true);
      return { rows, fields: rows[0] ? Object.keys(rows[0]) : undefined };
    },

    async explain(sql) {
      return { rows: await runJson("EXPLAIN QUERY PLAN " + sql) };
    },

    async listTables() {
      const rows = await runJson("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name");
      return rows.map((r) => String(r.name ?? ""));
    },

    async describeTable(table) {
      const rows = await runJson(`PRAGMA table_info(${quoteIdent(table)})`);
      return rows.map((r) => ({
        column: String(r.name ?? ""),
        type: String(r.type ?? ""),
        nullable: Number(r.notnull ?? 0) === 0,
      }));
    },

    async sampleTable(table, limit) {
      const rows = await runJson(`SELECT * FROM ${quoteIdent(table)} LIMIT ${limit}`);
      return { rows, fields: rows[0] ? Object.keys(rows[0]) : undefined };
    },

    async searchSchema(term) {
      const pattern = `%${term.replace(/%/g, "\\%").replace(/_/g, "\\_")}%`;
      const tables = await runJson(`SELECT name FROM sqlite_master WHERE type='table' AND name LIKE ${sqlQuote(pattern)} ESCAPE '\\' ORDER BY name`);
      const results: { kind: "table" | "column"; table: string; column?: string; type?: string }[] = [];
      for (const row of tables) results.push({ kind: "table", table: String(row.name ?? "") });

      for (const table of await this.listTables()) {
        for (const col of await this.describeTable(table)) {
          if (col.column.includes(term)) results.push({ kind: "column", table, column: col.column, type: col.type });
        }
      }
      results.sort((a, b) => a.table.localeCompare(b.table) || a.kind.localeCompare(b.kind) || (a.column ?? "").localeCompare(b.column ?? ""));
      return results;
    },

    async listRelationships(table) {
      const tables = table ? [table] : await this.listTables();
      const results: RelationshipInfo[] = [];
      for (const t of tables) {
        const rows = await runJson(`PRAGMA foreign_key_list(${quoteIdent(t)})`);
        for (const row of rows) {
          results.push({
            from_table: t,
            from_column: String(row.from ?? ""),
            to_table: String(row.table ?? ""),
            to_column: String(row.to ?? ""),
            constraint_name: `fk_${t}_${row.id ?? 0}`,
          });
        }
      }
      return results;
    },

    async tableStats(table): Promise<TableStats> {
      const indexes = await runJson(`PRAGMA index_list(${quoteIdent(table)})`);
      return {
        table,
        estimatedRows: null,
        sizeBytes: null,
        indexes: await Promise.all(indexes.map(async (idx) => {
          const name = String(idx.name ?? "");
          const cols = await runJson(`PRAGMA index_info(${quoteIdent(name)})`);
          return { name, columns: cols.map((c) => String(c.name ?? "")), unique: Number(idx.unique ?? 0) === 1 };
        })),
      };
    },

    async listSchemas() {
      return ["main"];
    },

    async listDatabases() {
      const rows = await runJson("PRAGMA database_list");
      return rows.map((r) => String(r.name ?? ""));
    },

    async getFullSchema() {
      const lines: string[] = [];
      for (const table of await this.listTables()) {
        const cols = await this.describeTable(table);
        const fks = await this.listRelationships(table);
        lines.push(`TABLE ${table} (`);
        for (const col of cols) lines.push(`  ${col.column} ${col.type} ${col.nullable ? "NULL" : "NOT NULL"}`);
        lines.push(")");
        for (const fk of fks) lines.push(`FK ${table}.${fk.from_column} -> ${fk.to_table}.${fk.to_column}`);
        lines.push("");
      }
      return lines.join("\n").trim();
    },

    async testConnection() {
      await runJson("SELECT 1 AS ok", 15_000);
    },

    async close() {
      evictSharedSSHClient(sessionId, params);
    },
  };
}
