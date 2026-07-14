import type { Adapter } from "../types.js";
import type { Integration } from "../../store/integrations.js";
import { createDriver } from "../../db/index.js";
import { registerSqlServer, sqlAgentHint, sqlInstructions, sqlToolSpecs } from "./server.js";
import { networkSqlFields, sqliteFields } from "./fields.js";
import { handleSqlApi, handleSqlLogApi } from "./api.js";
import { humanizeSqlError } from "./errors.js";
import { evictDriverEverywhere } from "./pool.js";

// Opening a driver and immediately closing it is the connectivity test for the
// whole SQL family (it sets up the SSH tunnel + SSL + auth, same as a real call).
async function testSql(integration: Integration): Promise<void> {
  // Test doubles as a force-refresh: tear down any stuck or pending-approval
  // connection the agent has open for this integration so we reconnect from
  // scratch and re-trigger the SSH prompt, instead of validating a poisoned
  // pool entry.
  evictDriverEverywhere(integration.id);
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
  toolSpecs: sqlToolSpecs(),
  agentHint: sqlAgentHint("postgres"),
  configFields: networkSqlFields(5432),
  testConnection: testSql,
  humanizeError: humanizeSqlError,
  handleApi: handleSqlApi,
  handleGlobalApi: handleSqlLogApi,
  instructions: sqlInstructions,
  register: registerSqlServer,
};

export const mysqlAdapter: Adapter = {
  id: "mysql",
  label: "MySQL",
  category: "database",
  policyKind: "sql",
  toolSpecs: sqlToolSpecs(),
  agentHint: sqlAgentHint("mysql"),
  configFields: networkSqlFields(3306),
  testConnection: testSql,
  humanizeError: humanizeSqlError,
  handleApi: handleSqlApi,
  handleGlobalApi: handleSqlLogApi,
  instructions: sqlInstructions,
  register: registerSqlServer,
};

export const sqliteAdapter: Adapter = {
  id: "sqlite",
  label: "SQLite",
  category: "database",
  policyKind: "sql",
  toolSpecs: sqlToolSpecs(),
  agentHint: sqlAgentHint("sqlite"),
  configFields: sqliteFields,
  testConnection: testSql,
  humanizeError: humanizeSqlError,
  handleApi: handleSqlApi,
  handleGlobalApi: handleSqlLogApi,
  instructions: sqlInstructions,
  register: registerSqlServer,
};

export const sqlAdapters: Adapter[] = [postgresAdapter, mysqlAdapter, sqliteAdapter];
