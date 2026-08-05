import type { ConfigField } from "../types.js";

export const herdFields: ConfigField[] = [
  {
    key: "site", label: "Base Site", type: "text", group: "App",
    placeholder: "the Herd site name, e.g. shop",
    help: "The site Herd already serves the app on. Feature sites are served at <feature>.<site>.<tld>. Blank works when Herd serves a single app.",
  },
  {
    key: "app_path", label: "App Path", type: "text", group: "App",
    placeholder: "found from the site above",
    help: "Override the folder behind the site — only needed when Herd doesn't serve the app.",
  },
  {
    key: "tld", label: "TLD", type: "text", group: "App",
    placeholder: "defaults to Herd's TLD",
  },
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
