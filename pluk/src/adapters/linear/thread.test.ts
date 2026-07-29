import { test, expect } from "bun:test";
import { threadComments } from "./index.js";

// Linear hands back a thread as a flat list in creation order, so the shape the
// agent reads is entirely ours to rebuild. These pin the two things that would
// silently mislead an agent: a reply attributed to the wrong comment, and a reply
// quietly dropped when its parent is outside the fetched page.

test("replies nest under the comment they answer, not under the thread root", () => {
  const [root] = threadComments([
    { id: "a", body: "question", parentId: null },
    { id: "b", body: "answer", parentId: "a" },
    { id: "c", body: "follow-up", parentId: "a" },
  ]);

  expect(root?.id).toBe("a");
  expect((root?.replies as Record<string, unknown>[]).map((r) => r.id)).toEqual(["b", "c"]);
  // parentId is redundant once nested — dropping it keeps the payload readable.
  expect(root).not.toHaveProperty("parentId");
});

test("a reply whose parent fell outside the page surfaces as a root, never disappears", () => {
  const roots = threadComments([
    { id: "b", body: "answer", parentId: "a" },
    { id: "c", body: "unrelated", parentId: null },
  ]);

  expect(roots.map((r) => r.id).sort()).toEqual(["b", "c"]);
});
