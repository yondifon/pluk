import { expect, test } from "bun:test";
import { applyOnly, pickPaths, type FieldMap } from "./onlyProjection.js";

const MAP: FieldMap = {
  fields: ["id", "title", "state", "assignee", "priority", "labels"],
  default: ["id", "title", "state.name"],
  presets: {
    priority: ["priority"],
    ids: ["id"],
    flags: (item) => ({ hasLabels: Array.isArray(item.labels) && item.labels.length > 0 }),
  },
};

test("nested dot path preserves nesting", () => {
  const item = { id: "1", title: "T", state: { name: "Open", type: "unstarted" }, assignee: { name: "Ada" } };
  expect(applyOnly(item, ["assignee.name"], MAP)).toEqual({ assignee: { name: "Ada" } });
});

test("path crossing an array maps over elements", () => {
  const data = { issue: "ENG-1", comments: [{ user: { name: "Ada" }, body: "hi" }, { user: { name: "Bob" }, body: "yo" }] };
  const map: FieldMap = { fields: ["issue", "comments"], default: ["issue", "comments.user.name"] };
  expect(pickPaths(data, ["comments.user.name"])).toEqual({
    comments: [{ user: { name: "Ada" } }, { user: { name: "Bob" } }],
  });
  expect(applyOnly(data, undefined, map)).toEqual({
    issue: "ENG-1",
    comments: [{ user: { name: "Ada" } }, { user: { name: "Bob" } }],
  });
});

test("path into a missing key resolves to undefined, not a throw", () => {
  const item = { id: "1", title: "T" };
  expect(applyOnly(item, ["assignee.name"], MAP)).toEqual({ assignee: undefined });
});

test("preset expands to its dot paths", () => {
  const item = { id: "1", title: "T", priority: 2 };
  expect(applyOnly(item, ["priority"], MAP)).toEqual({ priority: 2 });
});

test("preset and literal path compose in one array", () => {
  const item = { id: "1", title: "T", priority: 2 };
  expect(applyOnly(item, ["title", "priority"], MAP)).toEqual({ title: "T", priority: 2 });
});

test("function preset computes its own slice", () => {
  const item = { id: "1", title: "T", labels: ["bug"] };
  expect(applyOnly(item, ["flags"], MAP)).toEqual({ hasLabels: true });
});

test("only omitted returns the default set", () => {
  const item = { id: "1", title: "T", state: { name: "Open", type: "unstarted" }, priority: 3 };
  expect(applyOnly(item, undefined, MAP)).toEqual({ id: "1", title: "T", state: { name: "Open" } });
});

test("only: ['*'] bypasses filtering entirely", () => {
  const item = { id: "1", title: "T", extra: { deep: true } };
  expect(applyOnly(item, ["*"], MAP)).toBe(item);
});

test("only applies per element on a list", () => {
  const list = [{ id: "1", title: "A", priority: 1 }, { id: "2", title: "B", priority: 2 }];
  expect(applyOnly(list, ["priority"], MAP)).toEqual([{ priority: 1 }, { priority: 2 }]);
});

test("an empty only array falls back to the default set", () => {
  const item = { id: "1", title: "T", state: { name: "Open" } };
  expect(applyOnly(item, [], MAP)).toEqual({ id: "1", title: "T", state: { name: "Open" } });
});

test("an unrecognised entry throws and lists valid fields and presets", () => {
  expect(() => applyOnly({ id: "1" }, ["bogus"], MAP)).toThrow(
    /Unknown "only" field "bogus"\. Valid fields: id, title, state, assignee, priority, labels\. Presets: priority, ids, flags\./,
  );
});
