// The dock's state: what `workspace_updated` says, and the one diff open.

import assert from "node:assert/strict";
import { test } from "node:test";
import { createWorkspace, diffRequest, hasDock, planScroll } from "../app/workspace.js";

/** Ten todos a line (10px) each, in `statuses` order, then pending. @param {string[]} statuses */
const plan = (...statuses) => Array.from({ length: 10 }, (_, index) => ({ bottom: (index + 1) * 10, status: statuses[index] ?? "pending" }));

// The break this catches: a long plan scrolled to its end, or left at the top with the work in
// hand below the box, or the follow keeping no lookahead, so the next todo is always out of view
test("a long plan keeps the todo in hand and the next two in view", () => {
  const statuses = ["completed", "completed", "completed", "completed", "completed", "in-progress"];
  assert.equal(planScroll(plan(...statuses), 40), 40, "rows 6 to 8 end at 80, so 40 above a 40px box");
});

// The break this catches: a short plan or one barely begun scrolled away from its first rows
test("a plan whose work in hand fits stays at the top", () => {
  assert.equal(planScroll(plan("in-progress"), 40), 0);
  assert.equal(planScroll([], 40), 0);
});

// The break this catches: a plan with nothing in progress following nothing, or following a done
// todo, so the next thing to do isn't what shows
test("with nothing in progress, the plan follows the first todo not done, then the last", () => {
  assert.equal(planScroll(plan("completed", "completed", "completed", "completed"), 40), 30);
  assert.equal(planScroll(plan(...Array(10).fill("completed")), 40), 60);
});

// A `workspace_updated` frame's todos and changes, cut down from a live session
const TODOS = { current: null, items: [{ id: "1", subject: "Read the notes", status: "completed", blocked_by: [] }], pending: 0, in_progress: 0, completed: 1, blocked: 0, total: 1, truncated: false };
const CHANGES = { files: [{ path: "notes.txt", status: "added", additions: 1, deletions: 0, binary: false }], total_files: 1, additions: 1, deletions: 0, truncated: false };
const DIFF = { path: "notes.txt", file: null, untracked: false };

const frame = (kind, content) => ({ kind, content });

function opened(path = "notes.txt") {
  const workspace = createWorkspace();
  const request = diffRequest(path);
  workspace.opened(request);
  return { workspace, id: request.content.request_id };
}

// The break this catches: the dock left on the first snapshot, or todos and changes crossed
test("the latest snapshot is what the dock shows", () => {
  const workspace = createWorkspace();
  workspace.apply(frame("workspace_updated", { todos: { total: 0 }, changes: { total_files: 0 } }));
  workspace.apply(frame("workspace_updated", { todos: TODOS, changes: CHANGES }));
  assert.equal(workspace.todos.value, TODOS);
  assert.equal(workspace.changes.value, CHANGES);
});

// The break this catches: the project panel's branch left on the first snapshot, so a checkout or
// a commit never shows
test("the latest snapshot's project is the one the panel names", () => {
  const workspace = createWorkspace();
  const project = (branch, dirty) => ({ display_name: "luxe", git_branch: branch, git_dirty: dirty });
  workspace.apply(frame("workspace_updated", { todos: TODOS, changes: CHANGES, project: project("main", true) }));
  workspace.apply(frame("workspace_updated", { todos: TODOS, changes: CHANGES, project: project("remote-access", false) }));
  assert.deepEqual(workspace.project.value, project("remote-access", false));
});

// The break this catches: another request's answer (a TUI's, or one this page gave up on) shown
// as this file's diff
test("a diff is matched to its answer by request id", () => {
  const { workspace, id } = opened();
  workspace.apply(frame("file_diff_loaded", { request_id: "someone-else", diff: DIFF }));
  assert.equal(workspace.diff.value.waiting, true);
  workspace.apply(frame("file_diff_loaded", { request_id: id, diff: DIFF }));
  assert.deepEqual(workspace.diff.value, { path: "notes.txt", requestId: id, waiting: false, diff: DIFF });
});

// The break this catches: a failure left loading forever, or shown without core's reason
test("a failed diff keeps the reason", () => {
  const { workspace, id } = opened();
  workspace.apply(frame("file_diff_failed", { request_id: id, path: "notes.txt", error: "too_large" }));
  assert.equal(workspace.diff.value.waiting, false);
  assert.equal(workspace.diff.value.error, "too_large");
});

// The break this catches: a diff closed while loading reopened by its late answer
test("a closed diff stays closed", () => {
  const { workspace, id } = opened();
  workspace.close();
  workspace.apply(frame("file_diff_loaded", { request_id: id, diff: DIFF }));
  assert.equal(workspace.diff.value, null);
});

// The break this catches: a request lost with a dropped socket left loading forever, or a diff
// already shown thrown away by reconnecting
test("reconnecting gives up only on a diff still loading", () => {
  const { workspace, id } = opened();
  workspace.apply(frame("ready", {}));
  assert.equal(workspace.diff.value.lost, true);
  const shown = opened();
  shown.workspace.apply(frame("file_diff_loaded", { request_id: shown.id, diff: DIFF }));
  shown.workspace.apply(frame("ready", {}));
  assert.equal(shown.workspace.diff.value.lost, undefined);
  assert.notEqual(id, shown.id, "each request has its own id");
});

// The break this catches: the request's path or kind wrong, so the host answers a different file
test("a diff request names its file", () => {
  const request = diffRequest("src/a b.rs");
  assert.equal(request.kind, "load_file_diff");
  assert.equal(request.content.path, "src/a b.rs");
});

// The break this catches: an empty dock taking the wide screen's right side, or a dock with only
// a plan, or only changes, not shown
test("the dock shows when there's a plan or a change", () => {
  assert.equal(hasDock(null, null), false);
  assert.equal(hasDock({ total: 0 }, { total_files: 0 }), false);
  assert.equal(hasDock(TODOS, { total_files: 0 }), true);
  assert.equal(hasDock({ total: 0 }, CHANGES), true);
});
