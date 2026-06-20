import type { Adapter } from "../types.js";
import type { Integration } from "../../store/integrations.js";
import { sshFields } from "./fields.js";
import { testCommand } from "./client.js";
import { buildSshServer, registerSshServer } from "./server.js";

export const sshAdapter: Adapter = {
  id: "ssh",
  label: "SSH",
  category: "infrastructure",
  policyKind: "action",
  configFields: sshFields,
  async testConnection(integration: Integration): Promise<void> {
    await testCommand(integration);
  },
  buildServer: buildSshServer,
  register: registerSshServer,
};
