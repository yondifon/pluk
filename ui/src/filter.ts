/**
 * Filter + search reduction — pure logic, tested without DOM.
 * Mirrors swift/Sources/ContentView.swift filtering behaviour.
 */

import type { Environment, Group, Integration, AdapterManifest } from "./types";
import { typeLabel } from "./types";

export type HealthMap = Record<string, { status: "ok" | "error" }>;

export function matchesSearch(
  conn: Integration,
  query: string,
  adapters: AdapterManifest[],
): boolean {
  const q = query.trim().toLowerCase();
  if (!q) return true;
  const label = typeLabel(conn.type, adapters).toLowerCase();
  return (
    conn.name.toLowerCase().includes(q) ||
    conn.type.toLowerCase().includes(q) ||
    label.toLowerCase().includes(q) ||
    conn.environment.toLowerCase().includes(q)
  );
}

export function filteredGroups(
  groups: Group[],
  query: string,
  typeFilter: Set<string>,
  envFilter: Set<Environment>,
): Group[] {
  if (typeFilter.size > 0) return [];
  const q = query.trim().toLowerCase();
  return groups.filter((g) => {
    const matchSearch = !q || g.name.toLowerCase().includes(q);
    const matchEnv =
      envFilter.size === 0 || (g.environment ? envFilter.has(g.environment) : true);
    return matchSearch && matchEnv;
  });
}

export function filteredIntegrations(
  integrations: Integration[],
  query: string,
  typeFilter: Set<string>,
  envFilter: Set<Environment>,
  adapters: AdapterManifest[],
): Integration[] {
  return integrations.filter(
    (c) =>
      matchesSearch(c, query, adapters) &&
      (typeFilter.size === 0 || typeFilter.has(c.type)) &&
      (envFilter.size === 0 || envFilter.has(c.environment)),
  );
}

export function availableTypes(integrations: Integration[]): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const c of integrations) {
    if (!seen.has(c.type)) {
      seen.add(c.type);
      out.push(c.type);
    }
  }
  // sorted by label — but without adapters it is type alphabetical; caller can re-sort with adapters
  return out.sort((a, b) => a.localeCompare(b));
}

export function availableTypesSorted(
  integrations: Integration[],
  adapters: AdapterManifest[],
): string[] {
  const types = availableTypes(integrations);
  return types.sort((a, b) =>
    typeLabel(a, adapters).localeCompare(typeLabel(b, adapters), undefined, {
      sensitivity: "base",
    }),
  );
}

export function availableEnvs(
  integrations: Integration[],
  groups: Group[],
): Environment[] {
  const present = new Set<Environment>();
  for (const c of integrations) present.add(c.environment);
  for (const g of groups) if (g.environment) present.add(g.environment);
  const order: Environment[] = ["production", "staging", "development", "local"];
  return order.filter((e) => present.has(e));
}
