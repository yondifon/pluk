import { z } from "zod";
import type { Integration } from "../../store/integrations.js";
import { actionAdapter, type ActionTool } from "../kit.js";
import { herdFields } from "./fields.js";
import { assertFeature, createSite, destroySite, herdConfig, listSites, testHerd, type HerdConfig } from "./client.js";

const AGENT_HINT =
  "Use this to test a branch on its own local URL. create_site makes a git worktree of the app, links the untracked paths it needs to boot (vendor, node_modules, build output), copies .env with APP_URL repointed, and serves it from Herd at <feature>.<site>.<tld>. destroy_site tears the site down; the branch survives.";

/** The config is read per call rather than at register time: the app path and
 *  Herd binary are local state that can change under a long-lived session. */
type ConfigRef = () => HerdConfig;

function herdTools(cfg: ConfigRef): ActionTool[] {
  return [
    {
      name: "list_sites",
      description: "List the feature sites for this app — feature, branch, URL and worktree path.",
      category: "read",
      run: () => listSites(cfg()),
    },
    {
      name: "create_site",
      description:
        "Create a feature site: a git worktree of the app on its own branch, with untracked paths linked and a Herd URL. Returns the URL to test.",
      category: "write",
      schema: {
        feature: z.string().describe("Feature name; becomes the subdomain and the worktree folder, e.g. checkout-fix"),
        branch: z.string().optional().describe("Branch to check out; created from base when it doesn't exist. Defaults to the feature name"),
        base: z.string().optional().describe("Git ref to branch from when the branch is new. Defaults to HEAD"),
      },
      detail: (a) => `create_site ${a.feature}`,
      run: (a) =>
        createSite(cfg(), assertFeature(a.feature as string), {
          branch: a.branch as string | undefined,
          base: a.base as string | undefined,
        }),
    },
    {
      name: "destroy_site",
      description: "Tear down a feature site: unlink it from Herd and remove its worktree. The branch is kept.",
      category: "delete",
      schema: {
        feature: z.string().describe("Feature name used with create_site"),
        force: z.boolean().default(false).describe("Remove the worktree even when it has uncommitted changes"),
      },
      detail: (a) => `destroy_site ${a.feature}`,
      run: (a) => destroySite(cfg(), assertFeature(a.feature as string), a.force as boolean),
    },
  ];
}

export const herdAdapter = actionAdapter<ConfigRef>({
  id: "herd",
  label: "Laravel Herd",
  category: "local-dev",
  agentHint: AGENT_HINT,
  access:
    "Lists feature sites; creates and destroys them when write/delete are permitted. Creating and destroying a site runs git and herd against the app on this machine — every action is policy-checked and recorded in the activity log.",
  start: "list_sites",
  configFields: herdFields,
  client: (conn) => () => herdConfig(conn),
  testConnection: (conn: Integration) => testHerd(conn),
  tools: (_conn, cfg) => herdTools(cfg),
});
