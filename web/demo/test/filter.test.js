import assert from "node:assert/strict";
import test from "node:test";

import { dueBy, open, tagged } from "../src/filter.js";

const tasks = [
  { title: "Ship the docs", due: "2026-03-10", done: false, added: "2026-03-01", tags: ["docs"] },
  { title: "Fix the parser", due: "2026-03-14", done: false, added: "2026-03-04", tags: ["bug"] },
  { title: "Answer Priya", due: "2026-03-20", done: true, added: "2026-03-02", tags: ["inbox"] },
];

test("dueBy keeps everything up to the cutoff", () => {
  assert.deepEqual(
    dueBy(tasks, "2026-03-10").map((t) => t.title),
    ["Ship the docs"],
  );
});

test("open sorts newest first and drops what is done", () => {
  assert.deepEqual(
    open(tasks).map((t) => t.title),
    ["Fix the parser", "Ship the docs"],
  );
});

test("tagged wants all of them, not any", () => {
  assert.deepEqual(tagged(tasks, ["bug"]).length, 1);
  assert.deepEqual(tagged(tasks, ["bug", "docs"]).length, 0);
});
