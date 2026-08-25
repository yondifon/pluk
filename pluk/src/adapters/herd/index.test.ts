import { test, expect } from "bun:test";
import { mkdirSync, mkdtempSync, symlinkSync, writeFileSync } from "fs";
import { tmpdir } from "os";
import { join } from "path";
import { assertFeature, discoverSites, herdConfig, parseLinkPaths, parseWorktrees, setAppUrl, siteUrl } from "./client.js";
import { herdAdapter } from "./index.js";
import type { Integration } from "../../store/integrations.js";

function conn(config: Record<string, unknown>): Integration {
  return { id: "h", name: "Herd", type: "herd", config, read_only: 0, query_policy: null, token: "t", created_at: "" };
}

/** A machine with no Herd config: discovery finds nothing, defaults apply. */
const NO_HERD = join(tmpdir(), "pluk-herd-absent");

/** Build a Herd config folder: `parked` folders become parked apps, `linked`
 *  names become `herd link` symlinks, and `repos` get a .git to look like apps. */
function herdFixture(spec: { tld?: string; parked?: string[]; linked?: Record<string, string>; repos?: string[] }): string {
  const tmp = mkdtempSync(join(tmpdir(), "pluk-herd-"));
  const root = join(tmp, "config", "valet");
  const sites = join(root, "Sites");
  mkdirSync(sites, { recursive: true });

  for (const app of [...(spec.parked ?? []), ...Object.values(spec.linked ?? {})]) mkdirSync(join(tmp, app), { recursive: true });
  for (const app of spec.repos ?? []) mkdirSync(join(tmp, app, ".git"), { recursive: true });
  for (const [name, app] of Object.entries(spec.linked ?? {})) symlinkSync(join(tmp, app), join(sites, name));

  const parkedRoots = [...new Set((spec.parked ?? []).map((p) => join(tmp, p, "..")))];
  writeFileSync(join(root, "config.json"), JSON.stringify({ tld: spec.tld ?? "test", paths: [sites, ...parkedRoots] }));
  return root;
}

test("herdConfig derives the site and worktree root from the app folder", () => {
  const cfg = herdConfig(conn({ app_path: "/Users/me/Herd/app" }), NO_HERD);
  expect(cfg).toMatchObject({ site: "app", tld: "test", secure: true, worktreeRoot: "/Users/me/Herd/app-worktrees" });
});

test("herdConfig honours explicit site, tld, root and https toggle", () => {
  const cfg = herdConfig(conn({ app_path: "/Users/me/Herd/app/", site: "shop", tld: "localhost", secure: false, worktree_root: "~/wt/" }), NO_HERD);
  expect(cfg).toMatchObject({ site: "shop", tld: "localhost", secure: false });
  expect(cfg.worktreeRoot.endsWith("/wt")).toBe(true);
});

test("discoverSites lists linked sites first, then the parked folders", () => {
  const root = herdFixture({ linked: { shop: "apps/shop" }, parked: ["parked/blog", "parked/docs"], repos: ["apps/shop", "parked/blog"] });
  expect(discoverSites(root).map((s) => s.site)).toEqual(["shop", "blog", "docs"]);
});

test("herdConfig finds the app from Herd when only one site is a repository", () => {
  const root = herdFixture({ tld: "dev", linked: { shop: "apps/shop" }, parked: ["parked/notes"], repos: ["apps/shop"] });
  const cfg = herdConfig(conn({}), root);
  expect(cfg.site).toBe("shop");
  expect(cfg.tld).toBe("dev");
  expect(cfg.appPath.endsWith("/apps/shop")).toBe(true);
});

test("herdConfig resolves the app path from the named Herd site", () => {
  const root = herdFixture({ linked: { shop: "apps/shop" }, parked: ["parked/blog"], repos: ["apps/shop", "parked/blog"] });
  expect(herdConfig(conn({ site: "Blog" }), root).appPath.endsWith("/parked/blog")).toBe(true);
  expect(() => herdConfig(conn({ site: "nope" }), root)).toThrow(/no site called "nope".*blog, shop/s);
});

test("herdConfig asks which app when Herd serves several and none is named", () => {
  const root = herdFixture({ linked: { shop: "apps/shop" }, parked: ["parked/blog"], repos: ["apps/shop", "parked/blog"] });
  expect(() => herdConfig(conn({}), root)).toThrow(/serves 2 apps/);
  expect(() => herdConfig(conn({}), NO_HERD)).toThrow(/serves no sites/);
});

test("an explicit app path overrides discovery", () => {
  const root = herdFixture({ linked: { shop: "apps/shop" }, repos: ["apps/shop"] });
  expect(herdConfig(conn({ app_path: "/Users/me/Herd/other" }), root)).toMatchObject({ site: "other", appPath: "/Users/me/Herd/other" });
});

test("siteUrl nests the feature under the base site", () => {
  expect(siteUrl(herdConfig(conn({ app_path: "/x/app" }), NO_HERD), "checkout-fix")).toBe("https://checkout-fix.app.test");
  expect(siteUrl(herdConfig(conn({ app_path: "/x/app", secure: false }), NO_HERD), "feature")).toBe("http://feature.app.test");
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

test("testConnection reports an unresolvable app before shelling out", async () => {
  await expect(herdAdapter.testConnection(conn({ site: "definitely-not-a-herd-site-xyz" }))).rejects.toThrow(/Herd serves no/);
});
