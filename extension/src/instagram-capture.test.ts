import { expect, test } from "bun:test";
import { installInstagramCapture } from "./instagram-capture";

interface FixtureElement {
  id: string;
  type: string;
  hidden: boolean;
  textContent: string;
}

function makeDocument(): {
  readonly getElementById: (id: string) => FixtureElement | null;
  readonly createElement: (tagName: string) => FixtureElement;
  readonly head: { appendChild(node: FixtureElement): void };
  readonly documentElement: { appendChild(node: FixtureElement): void };
  readonly containerText: () => string | null;
} {
  let container: FixtureElement | null = null;
  return {
    getElementById: (id) => (id === container?.id ? container : null),
    createElement: () => ({ id: "", type: "", hidden: false, textContent: "" }),
    head: {
      appendChild: (node) => {
        container = node;
      },
    },
    documentElement: {
      appendChild: (node) => {
        container = node;
      },
    },
    containerText: () => container?.textContent ?? null,
  };
}

type FakeFetch = (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>;

function makeWindow(fetchImpl: FakeFetch): {
  fetch: FakeFetch;
  XMLHttpRequest: typeof XMLHttpRequest;
} {
  return { fetch: fetchImpl, XMLHttpRequest: class {} as unknown as typeof XMLHttpRequest };
}

function jsonResponse(body: unknown): Response {
  return new Response(JSON.stringify(body), {
    headers: { "content-type": "application/json" },
  });
}

test("records a fetch whose JSON body Instagram's own app receives", async () => {
  const doc = makeDocument();
  const win = makeWindow(async () => jsonResponse({ follower_count: 57008460 }));
  installInstagramCapture(win, doc, "https://www.instagram.com/janedoe/");

  const response = await win.fetch(
    "https://www.instagram.com/api/v1/users/web_profile_info/?username=janedoe",
  );
  expect(await response.clone().json()).toEqual({ follower_count: 57008460 });
  await Promise.resolve();
  await Promise.resolve();

  const published = JSON.parse(doc.containerText() ?? "[]");
  expect(published).toHaveLength(1);
  expect(published[0]).toMatchObject({
    url: "https://www.instagram.com/api/v1/users/web_profile_info/?username=janedoe",
    method: "GET",
    body: { follower_count: 57008460 },
  });
  expect(typeof published[0].receivedAt).toBe("number");
});

test("ignores a non-JSON response", async () => {
  const doc = makeDocument();
  const win = makeWindow(
    async () =>
      new Response("<html></html>", { headers: { "content-type": "text/html" } }),
  );
  const handle = installInstagramCapture(
    win,
    doc,
    "https://www.instagram.com/janedoe/",
  );

  await win.fetch("https://www.instagram.com/api/v1/something/");
  await Promise.resolve();
  await Promise.resolve();

  expect(handle.entries()).toHaveLength(0);
  expect(doc.containerText()).toBeNull();
});

test("ignores a response from another host", async () => {
  const doc = makeDocument();
  const win = makeWindow(async () => jsonResponse({ ok: true }));
  const handle = installInstagramCapture(
    win,
    doc,
    "https://www.instagram.com/janedoe/",
  );

  await win.fetch("https://evil.example.com/api/v1/something/");
  await Promise.resolve();
  await Promise.resolve();

  expect(handle.entries()).toHaveLength(0);
  expect(doc.containerText()).toBeNull();
});

test("a rejecting fetch still rejects the same way for the caller", async () => {
  const doc = makeDocument();
  const failure = new Error("network down");
  const win = makeWindow(async () => {
    throw failure;
  });
  installInstagramCapture(win, doc, "https://www.instagram.com/janedoe/");

  await expect(
    win.fetch("https://www.instagram.com/api/v1/something/"),
  ).rejects.toBe(failure);
});

test("drops the oldest entry once the buffer cap is hit", async () => {
  const doc = makeDocument();
  const win = makeWindow(async () => jsonResponse({ n: 0 }));
  const handle = installInstagramCapture(
    win,
    doc,
    "https://www.instagram.com/janedoe/",
  );

  for (let index = 0; index < 41; index += 1) {
    await win.fetch(`https://www.instagram.com/api/v1/item/${index}/`);
    await Promise.resolve();
    await Promise.resolve();
  }

  const entries = handle.entries();
  expect(entries).toHaveLength(40);
  expect(entries[0]?.url).toBe("https://www.instagram.com/api/v1/item/1/");
  expect(entries[entries.length - 1]?.url).toBe(
    "https://www.instagram.com/api/v1/item/40/",
  );
});
