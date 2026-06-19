import type { Adapter } from "../types.js";
import type { Integration } from "../../store/integrations.js";
import { linearGraphQL } from "./client.js";
import { linearFields } from "./fields.js";
import { buildLinearServer } from "./server.js";

export const linearAdapter: Adapter = {
  id: "linear",
  label: "Linear",
  category: "issue-tracker",
  policyKind: "action",
  configFields: linearFields,
  async testConnection(integration: Integration): Promise<void> {
    const apiKey = String(integration.config.api_key ?? "");
    await linearGraphQL<{ viewer: { id: string } }>(apiKey, `{ viewer { id name } }`);
  },
  buildServer: buildLinearServer,
};
