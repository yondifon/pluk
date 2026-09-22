import { MCP_TYPE } from "../integration-detail/types.ts";

/** A server Pluk can reach with nothing but its address. */
export interface ServerTemplate {
  /** Stable key, also the tile's handle in the DOM. */
  id: string;
  name: string;
  url: string;
  /** The line under the name. */
  summary: string;
  /** Set when the server only works once the person pastes a token in. */
  tokenHint?: string;
}

export const SERVER_TEMPLATES: ServerTemplate[] = [
  {
    id: "github",
    name: "GitHub",
    url: "https://api.githubcopilot.com/mcp/",
    summary: "Repos, issues, pull requests, and code search",
    tokenHint: "Paste a personal access token from GitHub.",
  },
  {
    id: "linear",
    name: "Linear",
    url: "https://mcp.linear.app/mcp",
    summary: "Issues, projects, and cycles",
  },
  {
    id: "sentry",
    name: "Sentry",
    url: "https://mcp.sentry.dev/mcp",
    summary: "Errors, issues, and releases",
  },
  {
    id: "slack",
    name: "Slack",
    url: "https://mcp.slack.com/mcp",
    summary: "Channels, messages, and search",
  },
  {
    id: "notion",
    name: "Notion",
    url: "https://mcp.notion.com/mcp",
    summary: "Pages, databases, and search",
  },
  {
    id: "context7",
    name: "Context7",
    url: "https://mcp.context7.com/mcp",
    summary: "Up-to-date library docs",
  },
  {
    id: "supabase",
    name: "Supabase",
    url: "https://mcp.supabase.com/mcp",
    summary: "Projects, tables, and SQL",
  },
  {
    id: "atlassian",
    name: "Atlassian",
    url: "https://mcp.atlassian.com/v2/mcp",
    summary: "Jira issues and Confluence pages",
  },
  {
    id: "stripe",
    name: "Stripe",
    url: "https://mcp.stripe.com",
    summary: "Payments, customers, and docs",
  },
  {
    id: "cloudflare",
    name: "Cloudflare",
    url: "https://mcp.cloudflare.com/mcp",
    summary: "Workers, DNS, and account resources",
  },
  {
    id: "neon",
    name: "Neon",
    url: "https://mcp.neon.tech/mcp",
    summary: "Postgres projects and branches",
  },
  {
    id: "deepwiki",
    name: "DeepWiki",
    url: "https://mcp.deepwiki.com/mcp",
    summary: "Ask questions about any public repo",
  },
  {
    id: "exa",
    name: "Exa",
    url: "https://mcp.exa.ai/mcp",
    summary: "Web and code search",
  },
  {
    id: "posthog",
    name: "PostHog",
    url: "https://mcp.posthog.com/mcp",
    summary: "Product analytics and feature flags",
  },
  {
    id: "huggingface",
    name: "Hugging Face",
    url: "https://huggingface.co/mcp",
    summary: "Models, datasets, and Spaces",
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
  return serverUrls.some((url) => sameServer(url, template.url));
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
  /** Opens the form on the server's own fields, so the token can be pasted in. */
  askForToken(name: string, template: ServerTemplate): void;
}

export async function addServer(template: ServerTemplate, host: ServerHost): Promise<void> {
  const name = availableName(template.name, host.takenNames);
  if (template.tokenHint) {
    host.askForToken(name, template);
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
