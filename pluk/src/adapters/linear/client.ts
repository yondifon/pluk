// Minimal Linear GraphQL client. Personal API keys go directly in the
// Authorization header (no "Bearer" prefix). Endpoint + auth confirmed against
// https://linear.app/developers/graphql.

const ENDPOINT = "https://api.linear.app/graphql";
const TIMEOUT_MS = 20_000;

interface GraphQLResponse<T> {
  data?: T;
  errors?: { message: string }[];
}

export async function linearGraphQL<T>(
  apiKey: string,
  query: string,
  variables?: Record<string, unknown>,
): Promise<T> {
  if (!apiKey) throw new Error("Linear API key is missing. Set it in the integration config.");

  let res: Response;
  try {
    res = await fetch(ENDPOINT, {
      method: "POST",
      headers: { "Content-Type": "application/json", Authorization: apiKey },
      body: JSON.stringify({ query, variables }),
      signal: AbortSignal.timeout(TIMEOUT_MS),
    });
  } catch (err) {
    if ((err as Error).name === "TimeoutError") throw new Error(`Linear API timed out after ${TIMEOUT_MS / 1000}s`);
    throw new Error(`Linear API request failed: ${(err as Error).message}`);
  }

  let json: GraphQLResponse<T>;
  try {
    json = (await res.json()) as GraphQLResponse<T>;
  } catch {
    throw new Error(`Linear API ${res.status}: non-JSON response`);
  }

  // Linear returns a structured errors[] even on 200; surface it first.
  if (json.errors?.length) throw new Error(`Linear: ${json.errors.map((e) => e.message).join("; ")}`);
  if (!res.ok) throw new Error(`Linear API ${res.status}`);
  if (!json.data) throw new Error("Linear API returned no data");
  return json.data;
}
