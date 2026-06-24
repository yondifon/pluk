import type { Adapter } from "../types.js";
import type { Integration } from "../../store/integrations.js";
import { createDriver } from "../../db/index.js";
import { registerSqlServer, sqlAgentHint, sqlInstructions } from "./server.js";
import { networkSqlFields, sqliteFields } from "./fields.js";

// Opening a driver and immediately closing it is the connectivity test for the
// whole SQL family (it sets up the SSH tunnel + SSL + auth, same as a real call).
async function testSql(integration: Integration): Promise<void> {
  const driver = await createDriver(integration);
  try {
    await driver.testConnection();
  } finally {
    await driver.close();
  }
}

export const postgresAdapter: Adapter = {
  id: "postgres",
  label: "PostgreSQL",
  category: "database",
  policyKind: "sql",
  agentHint: sqlAgentHint("postgres"),
  configFields: networkSqlFields(5432),
  testConnection: testSql,
  instructions: sqlInstructions,
  register: registerSqlServer,
};

export const mysqlAdapter: Adapter = {
  id: "mysql",
  label: "MySQL",
  category: "database",
  policyKind: "sql",
  agentHint: sqlAgentHint("mysql"),
  configFields: networkSqlFields(3306),
  testConnection: testSql,
  instructions: sqlInstructions,
  register: registerSqlServer,
};

export const sqliteAdapter: Adapter = {
  id: "sqlite",
  label: "SQLite",
  category: "database",
  policyKind: "sql",
  agentHint: sqlAgentHint("sqlite"),
  configFields: sqliteFields,
  testConnection: testSql,
  instructions: sqlInstructions,
  register: registerSqlServer,
};

export const sqlAdapters: Adapter[] = [postgresAdapter, mysqlAdapter, sqliteAdapter];
