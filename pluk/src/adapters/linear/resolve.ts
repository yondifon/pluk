import { linearGraphQL } from "./client.js";

interface TeamNode {
  id: string;
  key: string;
  name: string;
}

interface UserNode {
  id: string;
  name: string;
  email: string | null;
}

function cap(items: string[]): string {
  const shown = items.slice(0, 10).join(", ");
  return items.length > 10 ? `${shown}, …` : shown;
}

export async function resolveTeam(apiKey: string, keyOrName: string): Promise<TeamNode> {
  const data = await linearGraphQL<{ teams: { nodes: TeamNode[] } }>(apiKey, `{ teams { nodes { id key name } } }`);
  const want = keyOrName.trim().toLowerCase();
  const exact = data.teams.nodes.filter((t) => t.key.toLowerCase() === want || t.name.toLowerCase() === want);
  if (exact.length === 1) return exact[0]!;
  if (exact.length > 1) {
    const list = exact.map((t) => `${t.key} (${t.name})`).join(", ");
    throw new Error(`Team "${keyOrName}" matches more than one team: ${list}. Pass the exact team key.`);
  }
  const known = data.teams.nodes.map((t) => `${t.key} (${t.name})`);
  throw new Error(`No team named "${keyOrName}". Known teams: ${cap(known)}.`);
}

export async function resolveUser(apiKey: string, emailOrName: string): Promise<UserNode> {
  const term = emailOrName.trim();
  const byEmail = term.includes("@");
  const filter = byEmail ? { email: { containsIgnoreCase: term } } : { name: { containsIgnoreCase: term } };
  const data = await linearGraphQL<{ users: { nodes: UserNode[] } }>(
    apiKey,
    `query($filter:UserFilter){ users(first: 50, filter: $filter){ nodes { id name email } } }`,
    { filter },
  );
  const want = term.toLowerCase();
  const exact = data.users.nodes.filter((u) =>
    byEmail ? (u.email ?? "").toLowerCase() === want : u.name.toLowerCase() === want,
  );
  if (exact.length === 1) return exact[0]!;
  if (exact.length > 1) {
    const list = exact.map((u) => (u.email ? `${u.name} <${u.email}>` : u.name)).join(", ");
    throw new Error(`Assignee "${emailOrName}" matches more than one user: ${list}. Pass a unique email or name.`);
  }
  const near = data.users.nodes.map((u) => (u.email ? `${u.name} <${u.email}>` : u.name));
  if (near.length) throw new Error(`No user matches "${emailOrName}". Near matches: ${cap(near)}.`);
  throw new Error(`No user matches "${emailOrName}".`);
}

export async function resolveState(apiKey: string, teamKey: string, name: string): Promise<{ id: string; name: string }> {
  const data = await linearGraphQL<{ workflowStates: { nodes: { id: string; name: string }[] } }>(
    apiKey,
    `query($filter:WorkflowStateFilter){ workflowStates(filter: $filter){ nodes { id name } } }`,
    { filter: { team: { key: { eq: teamKey } } } },
  );
  const want = name.trim().toLowerCase();
  const exact = data.workflowStates.nodes.filter((s) => s.name.toLowerCase() === want);
  if (exact.length === 1) return exact[0]!;
  if (exact.length > 1) {
    const list = exact.map((s) => s.name).join(", ");
    throw new Error(`State "${name}" matches more than one workflow state: ${list}. Pass the exact state name.`);
  }
  const names = data.workflowStates.nodes.map((s) => s.name);
  throw new Error(`No workflow state named "${name}" in team ${teamKey}. States: ${cap(names)}.`);
}

export async function resolveLabels(apiKey: string, names: string[]): Promise<string[]> {
  const data = await linearGraphQL<{ issueLabels: { nodes: { id: string; name: string }[] } }>(
    apiKey,
    `{ issueLabels(first: 250){ nodes { id name } } }`,
  );
  return names.map((raw) => {
    const want = raw.trim().toLowerCase();
    const exact = data.issueLabels.nodes.filter((l) => l.name.toLowerCase() === want);
    if (exact.length === 1) return exact[0]!.id;
    if (exact.length > 1) {
      const list = exact.map((l) => l.name).join(", ");
      throw new Error(`Label "${raw}" matches more than one label: ${list}. Pass the exact label name.`);
    }
    const known = data.issueLabels.nodes.map((l) => l.name);
    throw new Error(`No label named "${raw}". Existing labels: ${cap(known)}.`);
  });
}
