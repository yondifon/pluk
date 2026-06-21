import type { Adapter } from "../types.js";
import type { Integration } from "../../store/integrations.js";
import { linearGraphQL } from "./client.js";
import { linearFields } from "./fields.js";
import { buildLinearServer, registerLinearServer, LINEAR_AGENT_HINT, linearInstructions } from "./server.js";

export const linearAdapter: Adapter = {
  id: "linear",
  label: "Linear",
  category: "issue-tracker",
  policyKind: "action",
  agentHint: LINEAR_AGENT_HINT,
  configFields: linearFields,
  async testConnection(integration: Integration): Promise<void> {
    const apiKey = String(integration.config.api_key ?? "");
    await linearGraphQL<{ viewer: { id: string } }>(apiKey, `{ viewer { id name } }`);
  },
  instructions: linearInstructions,
  buildServer: buildLinearServer,
  register: registerLinearServer,
};
