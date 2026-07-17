import type { ConfigField } from "../types.js";

export const herdFields: ConfigField[] = [
  {
    key: "app_path", label: "App Path", type: "text", group: "App", required: true,
    placeholder: "/Users/you/Herd/app",
    help: "The Laravel app's git repository — the folder Herd already serves.",
  },
  {
    key: "site", label: "Base Site", type: "text", group: "App",
    placeholder: "defaults to the app folder name",
    help: "Feature sites are served at <feature>.<site>.<tld>.",
  },
  { key: "tld", label: "TLD", type: "text", group: "App", default: "test" },
  { key: "secure", label: "Serve over HTTPS", type: "toggle", group: "App", default: true },

  {
    key: "worktree_root", label: "Worktree Root", type: "text", group: "Worktree",
    placeholder: "defaults to ../<app>-worktrees",
    help: "Where feature worktrees are created, one folder per feature.",
  },
  {
    key: "link_paths", label: "Linked Paths", type: "text", group: "Worktree",
    default: "vendor, node_modules, public/build",
    help: "Untracked paths symlinked from the app into each worktree (comma separated).",
  },
  {
    key: "env_file", label: "Env File", type: "text", group: "Worktree", default: ".env",
    help: "Copied into the worktree with APP_URL rewritten to the feature URL. Blank to skip.",
  },

  {
    key: "herd_bin", label: "Herd CLI", type: "text", group: "Herd",
    placeholder: "defaults to Herd's bundled binary",
  },
];
