import { expect, test } from "bun:test";
import {
  canonicalizeTargetUrl,
  DRIVER_CONTRACTS,
  MAX_SCREENSHOT_BYTES,
  makeReadyEnvelope,
  parseCommandEnvelope,
  parseCreateJobRequest,
  parseExtensionMessage,
  parseServerMessage,
} from "./protocol";
import { decodePngDataUrl, isSameAllowedOrigin } from "./browser-executor";
import {
  candidateServerUrls,
  parseConnectionSettings,
  parseServerUrl,
} from "./state";

test("accepts the bounded ready envelope and rejects oversized contracts", () => {
  const ready = makeReadyEnvelope("connection-1", Date.now());
  const parsed = parseServerMessage(ready);
  expect(parsed.ok).toBe(true);

  const oversized = {
    ...ready,
    contracts: [
      {
        platform: "x",
        hostnames: ["x".repeat(254)],
        capabilities: DRIVER_CONTRACTS.x.capabilities,
      },
    ],
  };
  const rejected = parseServerMessage(oversized);
  expect(rejected).toEqual({
    ok: false,
    error: {
      code: "invalid_schema",
      message: "Ready contracts contain an invalid hostname.",
    },
  });
});

test("probes the default port first and stays on loopback", () => {
  const candidates = candidateServerUrls("http://127.0.0.1:4242");
  expect(candidates[0]).toBe("http://127.0.0.1:4242");
  expect(new Set(candidates).size).toBe(candidates.length);
  for (const candidate of candidates) {
    expect(parseServerUrl(candidate)).toBe(candidate);
  }

  // A saved address outside the default range is still tried first.
  expect(candidateServerUrls("http://127.0.0.1:9100")[0]).toBe(
    "http://127.0.0.1:9100",
  );
});

test("restricts pairing endpoints to the local control plane", () => {
  expect(parseServerUrl("http://127.0.0.1:4242")).toBe("http://127.0.0.1:4242");
  expect(parseServerUrl("https://127.0.0.1:3210")).toBeNull();
  expect(parseServerUrl("http://localhost:3210")).toBeNull();
  expect(parseServerUrl("http://127.0.0.1:4242/path")).toBeNull();
  expect(
    parseConnectionSettings({
      serverUrl: "http://127.0.0.1:4242",
      token: "",
      enabled: true,
    }),
  ).toBeNull();
  expect(
    parseConnectionSettings({
      serverUrl: "http://127.0.0.1:4242",
      token: "short-token",
      enabled: true,
    }),
  ).toBeNull();
  expect(
    parseConnectionSettings({
      serverUrl: "http://127.0.0.1:4242",
      token: "x".repeat(257),
      enabled: true,
    }),
  ).toBeNull();
});

test("accepts only exact X HTTPS targets", () => {
  expect(canonicalizeTargetUrl("https://x.com:443/status/42", "x")).toMatchObject({
    ok: true,
    value: "https://x.com/status/42",
  });
  for (const targetUrl of [
    "https://x.com.evil/status/42",
    "http://x.com/status/42",
    "https://x.com/home#top",
    "https://user@x.com/home",
  ]) {
    expect(canonicalizeTargetUrl(targetUrl, "x")).toMatchObject({ ok: false });
  }
});

test("accepts same-host navigation and rejects cross-host redirects", () => {
  expect(
    isSameAllowedOrigin("https://x.com/status/42", "https://x.com/home", "x"),
  ).toBe(true);
  expect(
    isSameAllowedOrigin(
      "https://x.com/status/42",
      "https://twitter.com/status/42",
      "x",
    ),
  ).toBe(false);
});

test("accepts immediate X post submission payloads without a schedule", () => {
  const now = Date.now();
  const envelope = {
    version: 1,
    type: "command",
    jobId: "job-1",
    commandId: "command-1",
    platform: "x",
    action: "submit_post",
    targetUrl: "https://x.com/compose/post",
    issuedAt: now,
    expiresAt: now + 60_000,
    payload: {
      kind: "post_submission",
      draftId: "draft-1",
      text: "Hello from Wande",
    },
  };
  expect(parseCommandEnvelope(envelope)).toMatchObject({
    ok: true,
    value: {
      action: "submit_post",
      payload: { kind: "post_submission" },
    },
  });
  expect(
    parseCommandEnvelope({
      ...envelope,
      payload: { ...envelope.payload, scheduledAt: null },
    }),
  ).toMatchObject({ ok: false, error: { code: "invalid_schema" } });
});

test("rejects the removed native scheduling result", () => {
  expect(
    parseExtensionMessage({
      version: 1,
      type: "result",
      jobId: "job-1",
      commandId: "command-1",
      issuedAt: 100,
      expiresAt: 200,
      outcome: "succeeded",
      data: {
        kind: "scheduled_submission",
        platform: "x",
        accountIdentity: "@owner",
        scheduledAt: 1_000,
        scheduledNotification: "Your post was scheduled.",
      },
    }),
  ).toMatchObject({ ok: false, error: { code: "invalid_schema" } });
});

test("a zero-input feed request resolves to the fixed feed target", () => {
  const request = parseCreateJobRequest({
    platform: "x",
    action: "read_feed",
    payload: {},
  });
  expect(request).toMatchObject({
    ok: true,
    value: { targetUrl: "https://x.com/home", payload: { kind: "empty" } },
  });

  const trends = parseCreateJobRequest({
    platform: "x",
    action: "read_trends",
    payload: {},
  });
  expect(trends).toMatchObject({
    ok: true,
    value: { targetUrl: "https://x.com/explore" },
  });

  // An explicit override is still accepted and validated normally.
  const explicit = parseCreateJobRequest({
    platform: "x",
    action: "read_feed",
    targetUrl: "https://x.com/i/timeline",
    payload: {},
  });
  expect(explicit).toMatchObject({
    ok: true,
    value: { targetUrl: "https://x.com/i/timeline" },
  });
});

test("a profile read resolves a username to the canonical profile URL", () => {
  const byHandle = parseCreateJobRequest({
    platform: "x",
    action: "read_profile",
    payload: { username: "@jack" },
  });
  expect(byHandle).toMatchObject({
    ok: true,
    value: { targetUrl: "https://x.com/jack", payload: { kind: "empty" } },
  });

  // Legacy compatibility: a full profile URL still works without a username.
  const legacyUrl = parseCreateJobRequest({
    platform: "x",
    action: "read_profile",
    targetUrl: "https://x.com/jack",
    payload: {},
  });
  expect(legacyUrl).toMatchObject({
    ok: true,
    value: { targetUrl: "https://x.com/jack" },
  });
});

test("rejects invalid profile usernames, mixed inputs, and other platforms", () => {
  expect(
    parseCreateJobRequest({
      platform: "x",
      action: "read_profile",
      payload: { username: "not a handle" },
    }),
  ).toMatchObject({ ok: false, error: { code: "invalid_schema" } });

  expect(
    parseCreateJobRequest({
      platform: "x",
      action: "read_profile",
      payload: {},
    }),
  ).toMatchObject({ ok: false, error: { code: "invalid_target" } });

  expect(
    parseCreateJobRequest({
      platform: "x",
      action: "read_profile",
      payload: { username: "jack", extra: "field" },
    }),
  ).toMatchObject({ ok: false, error: { code: "invalid_schema" } });

  expect(
    parseCreateJobRequest({
      platform: "x",
      action: "read_profile",
      targetUrl: "https://x.com/explore",
      payload: {},
    }),
  ).toMatchObject({ ok: false, error: { code: "invalid_schema" } });

  for (const platform of ["linkedin", "instagram", "gmail", "tiktok"]) {
    expect(
      parseCreateJobRequest({
        platform,
        action: "read_profile",
        payload: { username: "someone" },
      }),
    ).toMatchObject({ ok: false, error: { code: "invalid_schema" } });
  }
});

test("decodes only bounded PNG screenshots", () => {
  expect(Array.from(decodePngDataUrl("data:image/png;base64,AA=="))).toEqual([
    0,
  ]);
  expect(() => decodePngDataUrl("data:image/jpeg;base64,AA==")).toThrow(
    "unsupported screenshot format",
  );
  const oversized = `data:image/png;base64,${"A".repeat(
    Math.ceil((MAX_SCREENSHOT_BYTES * 4) / 3) + 5,
  )}`;
  expect(() => decodePngDataUrl(oversized)).toThrow(
    "exceeded the local size limit",
  );
});
