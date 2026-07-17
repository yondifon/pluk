import { test, expect } from "bun:test";
import { assertFeature, herdConfig, parseLinkPaths, parseWorktrees, setAppUrl, siteUrl } from "./client.js";
import { herdAdapter } from "./index.js";
import type { Integration } from "../../store/integrations.js";

function conn(config: Record<string, unknown>): Integration {
  return { id: "h", name: "Herd", type: "herd", config, read_only: 0, query_policy: null, token: "t", created_at: "" };
}

test("herdConfig derives the site and worktree root from the app folder", () => {
  const cfg = herdConfig(conn({ app_path: "/Users/me/Herd/app" }));
  expect(cfg).toMatchObject({ site: "app", tld: "test", secure: true, worktreeRoot: "/Users/me/Herd/app-worktrees" });
});

test("herdConfig honours explicit site, tld, root and https toggle", () => {
  const cfg = herdConfig(conn({ app_path: "/Users/me/Herd/app/", site: "shop", tld: "localhost", secure: false, worktree_root: "~/wt/" }));
  expect(cfg).toMatchObject({ site: "shop", tld: "localhost", secure: false });
  expect(cfg.worktreeRoot.endsWith("/wt")).toBe(true);
});

test("herdConfig rejects a missing app path", () => {
  expect(() => herdConfig(conn({}))).toThrow(/App path is missing/);
});

test("siteUrl nests the feature under the base site", () => {
  expect(siteUrl(herdConfig(conn({ app_path: "/x/app" })), "checkout-fix")).toBe("https://checkout-fix.app.test");
  expect(siteUrl(herdConfig(conn({ app_path: "/x/app", secure: false })), "feature")).toBe("http://feature.app.test");
});

test("assertFeature lowercases a valid name and rejects anything else", () => {
  expect(assertFeature(" Checkout-Fix ")).toBe("checkout-fix");
  // A feature name reaches a hostname and a path — reject separators outright.
  for (const bad of ["", "-lead", "a b", "../etc", "a/b", "feature;rm -rf", "x".repeat(41)]) {
    expect(() => assertFeature(bad)).toThrow(/Invalid feature name/);
  }
});

test("parseLinkPaths splits on commas and newlines, rejecting escapes", () => {
  expect(parseLinkPaths("vendor, node_modules\npublic/build ")).toEqual(["vendor", "node_modules", "public/build"]);
  expect(() => parseLinkPaths("vendor, ../../etc")).toThrow(/Invalid linked path/);
  expect(() => parseLinkPaths("/etc/passwd")).toThrow(/Invalid linked path/);
});

test("setAppUrl repoints an existing APP_URL and leaves the rest alone", () => {
  const env = "APP_NAME=App\nAPP_URL=https://app.test\nDB_DATABASE=app\n";
  expect(setAppUrl(env, "https://f.app.test")).toBe("APP_NAME=App\nAPP_URL=https://f.app.test\nDB_DATABASE=app\n");
});

test("setAppUrl appends APP_URL when the env file has none", () => {
  expect(setAppUrl("APP_NAME=App", "https://f.app.test")).toBe("APP_NAME=App\nAPP_URL=https://f.app.test\n");
  expect(setAppUrl("", "https://f.app.test")).toBe("APP_URL=https://f.app.test\n");
});

test("parseWorktrees reads path + branch, marking detached checkouts", () => {
  const out = parseWorktrees(
    "worktree /Users/me/Herd/app\nHEAD abc\nbranch refs/heads/main\n\nworktree /Users/me/Herd/app-worktrees/f\nHEAD def\nbranch refs/heads/f\n\nworktree /tmp/d\nHEAD 000\ndetached\n",
  );
  expect(out).toEqual([
    { path: "/Users/me/Herd/app", branch: "main" },
    { path: "/Users/me/Herd/app-worktrees/f", branch: "f" },
    { path: "/tmp/d", branch: "detached" },
  ]);
});

test("the adapter exposes list/create/destroy with create and destroy off by default", () => {
  const specs = Object.fromEntries(herdAdapter.toolSpecs.map((t) => [t.name, t]));
  expect(Object.keys(specs).sort()).toEqual(["create_site", "destroy_site", "list_sites"]);
  expect(specs.list_sites!.defaultEnabled).toBe(true);
  expect(specs.create_site!.defaultEnabled).toBe(false);
  expect(specs.destroy_site!.defaultEnabled).toBe(false);
});

test("testConnection rejects a blank app path before shelling out", async () => {
  await expect(herdAdapter.testConnection(conn({}))).rejects.toThrow(/App path is missing/);
});
