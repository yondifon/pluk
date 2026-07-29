import type { ConfigField } from "../types.js";

export const sparkFields: ConfigField[] = [
  {
    key: "spark_bin", label: "Spark CLI", type: "text", group: "Spark",
    placeholder: "/usr/local/bin/spark",
    help: "The spark binary installed by Spark Desktop. Spark Desktop must be running.",
  },
  {
    key: "timeout_seconds", label: "Timeout (s)", type: "number", group: "Spark", default: 30,
    help: "How long a spark command may run before it is killed.",
  },

  {
    key: "default_account", label: "Account", type: "text", group: "Defaults",
    placeholder: "you@example.com",
    help: "Used as the from address when a draft doesn't name one.",
  },
  {
    key: "default_folder", label: "Folder", type: "text", group: "Defaults",
    placeholder: "Inbox",
    help: "Folder listed by list_emails when none is given, e.g. you@example.com:Archive.",
  },
  {
    key: "default_team", label: "Team", type: "text", group: "Defaults",
    help: "Team used for comments and team actions when you belong to several.",
  },

  {
    key: "max_page_size", label: "Max Page Size", type: "number", group: "Limits", default: 25,
    help: "Caps how many emails, meetings or templates one call may return — Spark prints full bodies.",
  },
];
