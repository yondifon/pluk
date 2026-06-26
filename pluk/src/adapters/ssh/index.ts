import type { Adapter } from "../types.js";
import type { Integration } from "../../store/integrations.js";
import { sshFields } from "./fields.js";
import { testCommand } from "./client.js";
import { registerSshServer, SSH_AGENT_HINT, sshInstructions, sshToolSpecs } from "./server.js";
import { handleSshApi } from "./api.js";
import { humanizeSshError } from "./errors.js";

export const sshAdapter: Adapter = {
  id: "ssh",
  label: "SSH",
  category: "infrastructure",
  policyKind: "none",
  toolSpecs: sshToolSpecs(),
  agentHint: SSH_AGENT_HINT,
  configFields: sshFields,
  async testConnection(integration: Integration): Promise<void> {
    await testCommand(integration);
  },
  humanizeError: humanizeSshError,
  handleApi: handleSshApi,
  instructions: sshInstructions,
  register: registerSshServer,
};
