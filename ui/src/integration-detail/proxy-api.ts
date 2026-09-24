import { humanizeHealthError } from "../health";
import { integrationApi } from "../host";

export type Answer<T> = { ok: true; value: T } | { ok: false; error: string };

/**
 * One call to a route the server's adapter serves. A refusal the route itself
 * chose comes back with its own wording; only a host that could not carry the
 * request at all throws.
 */
export async function callProxyApi<T>(
  integrationId: string,
  method: string,
  subpath: string,
  body?: unknown,
): Promise<Answer<T>> {
  try {
    const answer = await integrationApi<T & { error?: string }>({
      integrationId,
      method,
      subpath,
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    if (answer.status >= 200 && answer.status < 300) return { ok: true, value: answer.body };
    return { ok: false, error: humanizeHealthError(answer.body?.error) };
  } catch (e) {
    return { ok: false, error: humanizeHealthError(e instanceof Error ? e.message : String(e)) };
  }
}
