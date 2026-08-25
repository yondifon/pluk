import { z } from "zod";
import type { Integration } from "../../store/integrations.js";
import { actionAdapter, type ActionTool, type CommandOutput } from "../kit.js";
import { applyOnly, onlySchema, onlyValue, type FieldMap } from "../onlyProjection.js";
import { herdFields } from "./fields.js";
import { assertFeature, createSite, destroySite, herdConfig, listSites, testHerd, worktreePath, type HerdConfig } from "./client.js";

const AGENT_HINT =
  "Use this to test a branch on its own local URL. create_site makes a git worktree of the app, links the untracked paths it needs to boot (vendor, node_modules, build output), copies .env with APP_URL repointed, and serves it from Herd at <feature>.<site>.<tld>. destroy_site tears the site down; the branch survives.";

/** The config is read per call rather than at register time: the app path and
 *  Herd binary are local state that can change under a long-lived session. */
type ConfigRef = () => HerdConfig;

const LIST_SITES_MAP: FieldMap = { fields: ["value", "command"], default: ["value"], presets: { command: ["command"] } };

function herdTools(cfg: ConfigRef): ActionTool[] {
  const command = (args: string[]): string => {
    const quote = (value: string): string => /^[A-Za-z0-9_./:@%+=,-]+$/.test(value) ? value : `'${value.replace(/'/g, "'\\''")}'`;
    return args.map(quote).join(" ");
  };

  return [
    {
      name: "list_sites",
      description: "List the feature sites for this app — feature, branch, URL and worktree path.",
      category: "read",
      schema: { only: onlySchema(["command"]) },
      run: async (a) => {
        const c = cfg();
        const result = {
          value: await listSites(c),
          command: command(["git", "-C", c.appPath, "worktree", "list", "--porcelain"]),
        } satisfies CommandOutput;
        return applyOnly(result, onlyValue(a), LIST_SITES_MAP);
      },
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
      run: async (a) => {
        const c = cfg();
        const feature = assertFeature(a.feature as string);
        const branch = (a.branch as string | undefined)?.trim() || feature;
        return {
          value: await createSite(c, feature, {
            branch: a.branch as string | undefined,
            base: a.base as string | undefined,
          }),
          command: command(["git", "-C", c.appPath, "worktree", "add", worktreePath(c, feature), branch]),
        } satisfies CommandOutput;
      },
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
      run: async (a) => {
        const c = cfg();
        const feature = assertFeature(a.feature as string);
        return {
          value: await destroySite(c, feature, a.force as boolean),
          command: command(["git", "-C", c.appPath, "worktree", "remove", ...(a.force ? ["--force"] : []), worktreePath(c, feature)]),
        } satisfies CommandOutput;
      },
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
