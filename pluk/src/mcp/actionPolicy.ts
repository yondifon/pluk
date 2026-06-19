/**
 * Action policy for API adapters (Linear, Sentry, …). The SQL policy engine
 * classifies statements; this is its coarse-grained sibling for services whose
 * operations are tool calls, not SQL. Each adapter tool declares a category and
 * is gated against the integration's allowed set.
 */

export type ActionCategory = "read" | "write" | "delete" | "admin";

const ALL: ActionCategory[] = ["read", "write", "delete", "admin"];

export interface ActionPolicy {
  allowed: ActionCategory[];
}

/**
 * Derive an action policy from the integration's stored `query_policy` + the
 * `read_only` flag. Forward-compatible: an explicit `{ "actions": [...] }` blob
 * wins. Otherwise we fall back to the read_only flag — read-only ⇒ read only,
 * else read+write. `delete`/`admin` are never granted implicitly.
 */
export function parseActionPolicy(raw: string | null | undefined, readOnly: number): ActionPolicy {
  if (raw) {
    try {
      const parsed = JSON.parse(raw) as { actions?: unknown };
      if (Array.isArray(parsed.actions)) {
        const allowed = parsed.actions.filter((a): a is ActionCategory => ALL.includes(a as ActionCategory));
        if (allowed.length > 0) return { allowed };
      }
    } catch {
      // fall through to flag-based default
    }
  }
  return { allowed: readOnly ? ["read"] : ["read", "write"] };
}

export function actionAllowed(policy: ActionPolicy, category: ActionCategory): boolean {
  return policy.allowed.includes(category);
}

export function actionPolicyDescription(policy: ActionPolicy): string {
  return `Allowed actions: ${policy.allowed.join(", ")}.`;
}
