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

/** Started with npx, whose downloads live in ~/.npm rather than the temp folder macOS clears. */
export const LOCAL_TEMPLATES: LocalTemplate[] = [
  {
    id: "sentry-local",
    name: "Sentry (local)",
    command: "npx",
    args: ["-y", "@sentry/mcp-server@latest"],
    tokenEnv: "SENTRY_ACCESS_TOKEN",
    tokenHint: "Paste a user auth token from Sentry.",
    site: "https://sentry.io",
    summary: "Errors, issues, and releases",
  },
  {
    id: "playwright",
    name: "Playwright",
    command: "npx",
    args: ["-y", "@playwright/mcp@latest"],
    site: "https://playwright.dev",
    summary: "Drive a browser: click, type, and read pages",
  },
  {
    id: "chrome-devtools",
    name: "Chrome DevTools",
    command: "npx",
    args: ["-y", "chrome-devtools-mcp@latest"],
    site: "https://developer.chrome.com",
    summary: "Debug pages, network, and performance in Chrome",
  },
  {
    id: "notion-local",
    name: "Notion (local)",
    command: "npx",
    args: ["-y", "@notionhq/notion-mcp-server"],
    tokenEnv: "NOTION_TOKEN",
    tokenHint: "Paste an integration token from Notion.",
    site: "https://notion.so",
    summary: "Pages, databases, and search",
  },
  {
    id: "figma",
    name: "Figma",
    command: "npx",
    args: ["-y", "figma-developer-mcp", "--stdio"],
    tokenEnv: "FIGMA_API_KEY",
    tokenHint: "Paste a personal access token from Figma.",
    site: "https://figma.com",
    summary: "Layout and styles from Figma files",
  },
  {
    id: "brave-search",
    name: "Brave Search",
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
    command: "npx",
    args: ["-y", "firecrawl-mcp"],
    tokenEnv: "FIRECRAWL_API_KEY",
    tokenHint: "Paste an API key from Firecrawl.",
    site: "https://firecrawl.dev",
    summary: "Scrape and crawl websites",
  },
  {
    id: "heroku",
    name: "Heroku",
    command: "npx",
    args: ["-y", "@heroku/mcp-server"],
    tokenEnv: "HEROKU_API_KEY",
    tokenHint: "Paste an API key from Heroku.",
    site: "https://heroku.com",
    summary: "Apps, dynos, add-ons, and logs",
  },
];

/** How many tiles show before Show all. */
export const SERVERS_SHOWN = 8;
export const LOCAL_SHOWN = 4;

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
