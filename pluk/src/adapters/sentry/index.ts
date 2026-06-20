import type { Adapter } from "../types.js";
import type { Integration } from "../../store/integrations.js";
import { sentryFields } from "./fields.js";
import { sentryConfig, sentryRequest } from "./client.js";
import { buildSentryServer, registerSentryServer } from "./server.js";

export const sentryAdapter: Adapter = {
  id: "sentry",
  label: "Sentry",
  category: "observability",
  policyKind: "action",
  configFields: sentryFields,
  async testConnection(integration: Integration): Promise<void> {
    const cfg = sentryConfig(integration);
    // Cheapest authenticated call that validates token + org slug.
    await sentryRequest(cfg, "GET", `/organizations/${cfg.org}/`);
  },
  buildServer: buildSentryServer,
  register: registerSentryServer,
};
