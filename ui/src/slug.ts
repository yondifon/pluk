/**
 * Tool-prefix derivation — must match the server exactly.
 * Mirrors `pluk/src/mcp/namespace.ts#slug` and `crates/pluk-server/src/mcp/namespace.rs#slug`
 * and `swift/Sources/GroupDetailView.swift#NamespaceFormat.slug`.
 *
 * A mismatch would mislead the user about what to call the tool.
 */

export function slug(name: string): string {
  const s = name.toLowerCase().replace(/[^a-z0-9]+/g, "_").replace(/^_+|_+$/g, "");
  return s === "" ? "member" : s;
}

/**
 * Assign stable, collision-free slugs for a list of member names, in order.
 * Mirrors `pluk/src/mcp/group.ts#buildGroupServer` collision logic:
 *   first "foo" -> "foo", second "Foo!" -> "foo_2", third -> "foo_3"
 */
export function slugsWithCollision(names: string[]): string[] {
  const used = new Map<string, number>();
  const out: string[] = [];
  for (const name of names) {
    let ns = slug(name);
    const seen = used.get(ns) ?? 0;
    used.set(ns, seen + 1);
    if (seen > 0) ns = `${ns}_${seen + 1}`;
    out.push(ns);
  }
  return out;
}

export function toolPrefix(name: string): string {
  return `${slug(name)}__*`;
}

export function toolPrefixWithCollision(name: string, occurrence: number): string {
  // occurrence is 0-indexed: 0 -> no suffix, 1 -> _2
  let ns = slug(name);
  if (occurrence > 0) ns = `${ns}_${occurrence + 1}`;
  return `${ns}__*`;
}
