import { MCP_TYPE } from "../integration-detail/types.ts";

interface TemplateBase {
  /** Stable key, also the tile's handle in the DOM. */
  id: string;
  name: string;
  /** The line under the name. */
  summary: string;
  /** The vendor's own site, whose icon the tile shows. */
  site: string;
}

/** A server Pluk can reach with nothing but its address. */
export interface RemoteTemplate extends TemplateBase {
  url: string;
  /** Set when the server only works once the person pastes a token in. */
  tokenHint?: string;
}

/** A server Pluk starts on this Mac. One that needs a key reads it from `tokenEnv`. */
export interface LocalTemplate extends TemplateBase {
  category: string;
  command: string;
  args: string[];
  tokenEnv?: string;
  tokenHint?: string;
}

export type ServerTemplate = RemoteTemplate | LocalTemplate;

export const SERVER_TEMPLATES: RemoteTemplate[] = [
  {
    id: "github",
    name: "GitHub",
    url: "https://api.githubcopilot.com/mcp/",
    site: "https://github.com",
    summary: "Repos, issues, pull requests, and code search",
    tokenHint: "Paste a personal access token from GitHub.",
  },
  {
    id: "linear",
    name: "Linear",
    url: "https://mcp.linear.app/mcp",
    site: "https://linear.app",
    summary: "Issues, projects, and cycles",
  },
  {
    id: "sentry",
    name: "Sentry",
    url: "https://mcp.sentry.dev/mcp",
    site: "https://sentry.io",
    summary: "Errors, issues, and releases",
  },
  {
    id: "slack",
    name: "Slack",
    url: "https://mcp.slack.com/mcp",
    site: "https://slack.com",
    summary: "Channels, messages, and search",
  },
  {
    id: "notion",
    name: "Notion",
    url: "https://mcp.notion.com/mcp",
    site: "https://notion.so",
    summary: "Pages, databases, and search",
  },
  {
    id: "context7",
    name: "Context7",
    url: "https://mcp.context7.com/mcp",
    site: "https://context7.com",
    summary: "Up-to-date library docs",
  },
  {
    id: "supabase",
    name: "Supabase",
    url: "https://mcp.supabase.com/mcp",
    site: "https://supabase.com",
    summary: "Projects, tables, and SQL",
  },
  {
    id: "atlassian",
    name: "Atlassian",
    url: "https://mcp.atlassian.com/v2/mcp",
    site: "https://atlassian.com",
    summary: "Jira issues and Confluence pages",
  },
  {
    id: "stripe",
    name: "Stripe",
    url: "https://mcp.stripe.com",
    site: "https://stripe.com",
    summary: "Payments, customers, and docs",
  },
  {
    id: "cloudflare",
    name: "Cloudflare",
    url: "https://mcp.cloudflare.com/mcp",
    site: "https://cloudflare.com",
    summary: "Workers, DNS, and account resources",
  },
  {
    id: "neon",
    name: "Neon",
    url: "https://mcp.neon.tech/mcp",
    site: "https://neon.tech",
    summary: "Postgres projects and branches",
  },
  {
    id: "deepwiki",
    name: "DeepWiki",
    url: "https://mcp.deepwiki.com/mcp",
    site: "https://deepwiki.com",
    summary: "Ask questions about any public repo",
  },
  {
    id: "exa",
    name: "Exa",
    url: "https://mcp.exa.ai/mcp",
    site: "https://exa.ai",
    summary: "Web and code search",
  },
  {
    id: "posthog",
    name: "PostHog",
    url: "https://mcp.posthog.com/mcp",
    site: "https://posthog.com",
    summary: "Product analytics and feature flags",
  },
  {
    id: "huggingface",
    name: "Hugging Face",
    url: "https://huggingface.co/mcp",
    site: "https://huggingface.co",
    summary: "Models, datasets, and Spaces",
  },
];

export const LOCAL_CATEGORIES = [
  "Browser & testing",
  "Search & web",
  "Developer tools",
  "Databases",
  "Cloud & infrastructure",
  "Productivity & docs",
  "Design",
  "AI & memory",
] as const;

export const LOCAL_TEMPLATES: LocalTemplate[] = [
  {
    id: "playwright",
    name: "Playwright",
    category: "Browser & testing",
    command: "npx",
    args: ["-y", "@playwright/mcp@latest"],
    site: "https://playwright.dev",
    summary: "Drive a browser: click, type, and read pages",
  },
  {
    id: "chrome-devtools",
    name: "Chrome DevTools",
    category: "Browser & testing",
    command: "npx",
    args: ["-y", "chrome-devtools-mcp@latest"],
    site: "https://developer.chrome.com",
    summary: "Debug pages, network, and performance in Chrome",
  },
  {
    id: "qase",
    name: "Qase",
    category: "Browser & testing",
    command: "npx",
    args: ["-y", "@qase/mcp-server"],
    tokenEnv: "QASE_API_TOKEN",
    tokenHint: "Paste an API token from Qase.",
    site: "https://qase.io",
    summary: "Test cases, runs, and defects",
  },
  {
    id: "brave-search",
    name: "Brave Search",
    category: "Search & web",
    command: "npx",
    args: ["-y", "@brave/brave-search-mcp-server", "--transport", "stdio"],
    tokenEnv: "BRAVE_API_KEY",
    tokenHint: "Paste an API key from Brave Search.",
    site: "https://brave.com",
    summary: "Web, news, and image search",
  },
  {
    id: "perplexity",
    name: "Perplexity",
    category: "Search & web",
    command: "npx",
    args: ["-y", "@perplexity-ai/mcp-server"],
    tokenEnv: "PERPLEXITY_API_KEY",
    tokenHint: "Paste an API key from Perplexity.",
    site: "https://perplexity.ai",
    summary: "Web answers with sources",
  },
  {
    id: "tavily",
    name: "Tavily",
    category: "Search & web",
    command: "npx",
    args: ["-y", "tavily-mcp@latest"],
    tokenEnv: "TAVILY_API_KEY",
    tokenHint: "Paste an API key from Tavily.",
    site: "https://tavily.com",
    summary: "Web search and page extraction",
  },
  {
    id: "firecrawl",
    name: "Firecrawl",
    category: "Search & web",
    command: "npx",
    args: ["-y", "firecrawl-mcp"],
    tokenEnv: "FIRECRAWL_API_KEY",
    tokenHint: "Paste an API key from Firecrawl.",
    site: "https://firecrawl.dev",
    summary: "Scrape and crawl websites",
  },
  {
    id: "duckduckgo",
    name: "DuckDuckGo",
    category: "Search & web",
    command: "npx",
    args: ["-y", "@ericthered926/duckduckgo-mcp-server"],
    site: "https://duckduckgo.com",
    summary: "Web and news search",
  },
  {
    id: "fetch",
    name: "Fetch",
    category: "Search & web",
    command: "uvx",
    args: ["mcp-server-fetch"],
    site: "https://modelcontextprotocol.io",
    summary: "Fetch and convert web content",
  },
  {
    id: "sentry-local",
    name: "Sentry (local)",
    category: "Developer tools",
    command: "npx",
    args: ["-y", "@sentry/mcp-server@latest"],
    tokenEnv: "SENTRY_ACCESS_TOKEN",
    tokenHint: "Paste a user auth token from Sentry.",
    site: "https://sentry.io",
    summary: "Errors, issues, and releases",
  },
  {
    id: "everything",
    name: "Everything",
    category: "Developer tools",
    command: "npx",
    args: ["-y", "@modelcontextprotocol/server-everything"],
    site: "https://modelcontextprotocol.io",
    summary: "Protocol features and test tools",
  },
  {
    id: "context7-local",
    name: "Context7 (local)",
    category: "Developer tools",
    command: "npx",
    args: ["-y", "@upstash/context7-mcp"],
    site: "https://context7.com",
    summary: "Up-to-date library docs",
  },
  {
    id: "desktop-commander",
    name: "Desktop Commander",
    category: "Developer tools",
    command: "npx",
    args: ["-y", "@wonderwhy-er/desktop-commander@latest"],
    site: "https://desktopcommander.ai",
    summary: "Files, processes, and terminal automation",
  },
  {
    id: "git-mcp",
    name: "Git MCP",
    category: "Developer tools",
    command: "npx",
    args: ["-y", "@cyanheads/git-mcp-server@latest"],
    site: "https://github.com/cyanheads/git-mcp-server",
    summary: "Inspect and automate Git workflows",
  },
  {
    id: "gitlab",
    name: "GitLab",
    category: "Developer tools",
    command: "npx",
    args: ["-y", "@zereight/mcp-gitlab@latest"],
    tokenEnv: "GITLAB_PERSONAL_ACCESS_TOKEN",
    tokenHint: "Paste a personal access token from GitLab.",
    site: "https://gitlab.com",
    summary: "Projects, issues, pipelines, and releases",
  },
  {
    id: "eslint",
    name: "ESLint",
    category: "Developer tools",
    command: "npx",
    args: ["-y", "@eslint/mcp"],
    site: "https://eslint.org",
    summary: "Inspect and fix lint problems",
  },
  {
    id: "next-devtools",
    name: "Next.js DevTools",
    category: "Developer tools",
    command: "npx",
    args: ["-y", "next-devtools-mcp@latest"],
    site: "https://nextjs.org",
    summary: "Inspect and debug Next.js apps",
  },
  {
    id: "git",
    name: "Git",
    category: "Developer tools",
    command: "uvx",
    args: ["mcp-server-git"],
    site: "https://git-scm.com",
    summary: "Inspect history and manage commits",
  },
  {
    id: "time",
    name: "Time",
    category: "Developer tools",
    command: "uvx",
    args: ["mcp-server-time"],
    site: "https://modelcontextprotocol.io",
    summary: "Local time and timezone conversion",
  },
  {
    id: "postman",
    name: "Postman",
    category: "Developer tools",
    command: "npx",
    args: ["-y", "@postman/postman-mcp-server"],
    tokenEnv: "POSTMAN_API_KEY",
    tokenHint: "Paste an API key from Postman.",
    site: "https://postman.com",
    summary: "Collections, environments, and API workspaces",
  },
  {
    id: "kibana",
    name: "Kibana",
    category: "Developer tools",
    command: "npx",
    args: ["-y", "@tocharianou/mcp-server-kibana"],
    tokenEnv: "KIBANA_API_KEY",
    tokenHint: "Paste an API key from Kibana.",
    site: "https://www.elastic.co/kibana",
    summary: "Dashboards, indices, and Kibana data",
  },
  {
    id: "postgres",
    name: "PostgreSQL",
    category: "Databases",
    command: "npx",
    args: ["-y", "@yawlabs/postgres-mcp@latest"],
    tokenEnv: "DATABASE_URL",
    tokenHint: "Paste a PostgreSQL connection string.",
    site: "https://www.postgresql.org",
    summary: "Query PostgreSQL with read-only safeguards",
  },
  {
    id: "heroku",
    name: "Heroku",
    category: "Cloud & infrastructure",
    command: "npx",
    args: ["-y", "@heroku/mcp-server"],
    tokenEnv: "HEROKU_API_KEY",
    tokenHint: "Paste an API key from Heroku.",
    site: "https://heroku.com",
    summary: "Apps, dynos, add-ons, and logs",
  },
  {
    id: "hostinger",
    name: "Hostinger",
    category: "Cloud & infrastructure",
    command: "npx",
    args: ["-y", "hostinger-api-mcp"],
    tokenEnv: "HOSTINGER_API_TOKEN",
    tokenHint: "Paste an API token from Hostinger.",
    site: "https://www.hostinger.com",
    summary: "Domains, hosting, and VPS management",
  },
  {
    id: "kubernetes",
    name: "Kubernetes",
    category: "Cloud & infrastructure",
    command: "npx",
    args: ["-y", "mcp-server-kubernetes"],
    site: "https://kubernetes.io",
    summary: "Manage Kubernetes clusters through kubectl",
  },
  {
    id: "currents",
    name: "Currents",
    category: "Cloud & infrastructure",
    command: "npx",
    args: ["-y", "@currents/mcp"],
    tokenEnv: "CURRENTS_API_KEY",
    tokenHint: "Paste an API key from Currents.",
    site: "https://currents.dev",
    summary: "End-to-end test evidence and analysis",
  },
  {
    id: "aikido",
    name: "Aikido",
    category: "Cloud & infrastructure",
    command: "npx",
    args: ["-y", "@aikidosec/mcp"],
    tokenEnv: "AIKIDO_API_KEY",
    tokenHint: "Paste a personal access token from Aikido.",
    site: "https://www.aikido.dev",
    summary: "Code and secrets security scanning",
  },
  {
    id: "stripe-local",
    name: "Stripe (local)",
    category: "Cloud & infrastructure",
    command: "npx",
    args: ["-y", "@stripe/mcp"],
    tokenEnv: "STRIPE_SECRET_KEY",
    tokenHint: "Paste an API key from Stripe.",
    site: "https://stripe.com",
    summary: "Payments, customers, and docs",
  },
  {
    id: "notion-local",
    name: "Notion (local)",
    category: "Productivity & docs",
    command: "npx",
    args: ["-y", "@notionhq/notion-mcp-server"],
    tokenEnv: "NOTION_TOKEN",
    tokenHint: "Paste an integration token from Notion.",
    site: "https://notion.so",
    summary: "Pages, databases, and search",
  },
  {
    id: "obsidian",
    name: "Obsidian",
    category: "Productivity & docs",
    command: "npx",
    args: ["-y", "obsidian-mcp-server"],
    tokenEnv: "OBSIDIAN_API_KEY",
    tokenHint: "Paste an API key from Obsidian.",
    site: "https://obsidian.md",
    summary: "Read and edit Obsidian notes",
  },
  {
    id: "mantine",
    name: "Mantine",
    category: "Productivity & docs",
    command: "npx",
    args: ["-y", "@mantine/mcp-server"],
    site: "https://mantine.dev",
    summary: "Mantine components and documentation",
  },
  {
    id: "clickup",
    name: "ClickUp",
    category: "Productivity & docs",
    command: "npx",
    args: ["-y", "@taazkareem/clickup-mcp-server@latest"],
    tokenEnv: "CLICKUP_API_KEY",
    tokenHint: "Paste an API key from ClickUp.",
    site: "https://clickup.com",
    summary: "Tasks, docs, and chat",
  },
  {
    id: "todoist",
    name: "Todoist",
    category: "Productivity & docs",
    command: "npx",
    args: ["-y", "@doist/todoist-mcp"],
    tokenEnv: "TODOIST_API_KEY",
    tokenHint: "Paste an API token from Todoist.",
    site: "https://todoist.com",
    summary: "Tasks, projects, and labels",
  },
  {
    id: "asana",
    name: "Asana",
    category: "Productivity & docs",
    command: "npx",
    args: ["-y", "@roychri/mcp-server-asana"],
    tokenEnv: "ASANA_ACCESS_TOKEN",
    tokenHint: "Paste a personal access token from Asana.",
    site: "https://asana.com",
    summary: "Tasks, projects, and workflows",
  },
  {
    id: "contentful",
    name: "Contentful",
    category: "Productivity & docs",
    command: "npx",
    args: ["-y", "@contentful/mcp-server"],
    tokenEnv: "CONTENTFUL_MANAGEMENT_ACCESS_TOKEN",
    tokenHint: "Paste a personal access token from Contentful.",
    site: "https://www.contentful.com",
    summary: "Content, entries, and environments",
  },
  {
    id: "figma",
    name: "Figma",
    category: "Design",
    command: "npx",
    args: ["-y", "figma-developer-mcp", "--stdio"],
    tokenEnv: "FIGMA_API_KEY",
    tokenHint: "Paste a personal access token from Figma.",
    site: "https://figma.com",
    summary: "Layout and styles from Figma files",
  },
  {
    id: "memory",
    name: "Memory",
    category: "AI & memory",
    command: "npx",
    args: ["-y", "@modelcontextprotocol/server-memory"],
    site: "https://modelcontextprotocol.io",
    summary: "Persistent entities and relationships",
  },
  {
    id: "sequential-thinking",
    name: "Sequential Thinking",
    category: "AI & memory",
    command: "npx",
    args: ["-y", "@modelcontextprotocol/server-sequential-thinking"],
    site: "https://modelcontextprotocol.io",
    summary: "Structured step-by-step reasoning",
  },
  {
    id: "z-ai",
    name: "Z.AI",
    category: "AI & memory",
    command: "npx",
    args: ["-y", "@z_ai/mcp-server"],
    tokenEnv: "Z_AI_API_KEY",
    tokenHint: "Paste an API key from Z.AI.",
    site: "https://z.ai",
    summary: "AI search and generation tools",
  },
];

/** How many tiles show before Show all. */
export const SERVERS_SHOWN = 8;

/** Two addresses reaching the same server, give or take a trailing slash. */
function sameServer(a: string, b: string): boolean {
  const trim = (url: string) => url.trim().toLowerCase().replace(/\/+$/, "");
  return trim(a) === trim(b);
}

export function isAdded(template: ServerTemplate, serverUrls: string[]): boolean {
  return "url" in template && serverUrls.some((url) => sameServer(url, template.url));
}

/** `Linear`, then `Linear 2`, so a second one never lands on a name in use. */
export function availableName(base: string, taken: string[]): string {
  if (!taken.includes(base)) return base;
  let suffix = 2;
  while (taken.includes(`${base} ${suffix}`)) suffix++;
  return `${base} ${suffix}`;
}

/** What one click on a server tile needs from the app around it. */
export interface ServerHost {
  /** The names already in use, so the new one can step around them. */
  takenNames: string[];
  create(payload: {
    name: string;
    type: string;
    config: Record<string, string>;
    environment: null;
  }): Promise<{ id: string }>;
  /** Opens the integration that was just created. */
  reveal(id: string): void | Promise<void>;
  /** Opens the form on the server's own fields, so a token can be pasted in or a command checked. */
  prefill(name: string, template: ServerTemplate): void;
}

export async function addServer(template: ServerTemplate, host: ServerHost): Promise<void> {
  const name = availableName(template.name, host.takenNames);
  if (!("url" in template) || template.tokenHint) {
    host.prefill(name, template);
    return;
  }
  const created = await host.create({
    name,
    type: MCP_TYPE,
    config: { url: template.url },
    environment: null,
  });
  await host.reveal(created.id);
}
