// @ts-check
// The dock's state: the plan, the changed files and the project's branch from `workspace_updated`,
// and the one diff open, matched to its answer by request id so a late answer for a closed diff is
// dropped.

import { batch, signal } from "@preact/signals";

/**
 * @typedef {{ path: string, requestId: string, waiting: boolean, lost?: boolean,
 *   diff?: any, error?: any }} OpenDiff
 * @typedef {{ total: number, [field: string]: any }} Todos
 * @typedef {{ total_files: number, [field: string]: any }} Changes
 * @typedef {{ git_branch?: string | null, git_dirty?: boolean, [field: string]: any }} Project
 * @typedef {{ todos: Signal<Todos | null>, changes: Signal<Changes | null>, project: Signal<Project | null>,
 *   diff: Signal<OpenDiff | null> }} State
 */
/** @import { Signal } from "@preact/signals" */
/** @import { Frame } from "./connection.js" */

let asked = 0;

/** Nothing known yet, and the function that folds frames into what is. */
export function createWorkspace() {
  /** @type {State} */
  const state = { todos: signal(null), changes: signal(null), project: signal(null), diff: signal(null) };
  return {
    ...state,
    /** @param {Frame} frame */
    apply: (frame) => batch(() => apply(state, frame)),
    /** @param {{ content: { request_id: string, path: string } }} request */
    opened: ({ content }) => (state.diff.value = { path: content.path, requestId: content.request_id, waiting: true }),
    close: () => (state.diff.value = null),
    /** The request for the open diff went nowhere. */
    lost: () => dropped(state),
  };
}

/** @param {State} state @param {Frame} frame */
function apply(state, { kind, content }) {
  switch (kind) {
    case "workspace_updated":
      state.todos.value = content.todos;
      state.changes.value = content.changes;
      state.project.value = content.project;
      return;
    case "file_diff_loaded": return answer(state, content.request_id, { diff: content.diff });
    case "file_diff_failed": return answer(state, content.request_id, { error: content.error });
    case "ready": return dropped(state);
  }
}

/** @param {State} state @param {string} requestId @param {{ diff?: any, error?: any }} outcome */
function answer(state, requestId, outcome) {
  const open = state.diff.value;
  if (!open || open.requestId !== requestId) return;
  state.diff.value = { ...open, ...outcome, waiting: false };
}

/**
 * A request sent before the socket dropped is never answered, so the new connection says so.
 * @param {State} state
 */
function dropped(state) {
  const open = state.diff.value;
  if (open?.waiting) state.diff.value = { ...open, waiting: false, lost: true };
}

/**
 * The frame asking for `path`'s diff, with an id no other request from this page shares.
 * @param {string} path
 */
export function diffRequest(path) {
  asked += 1;
  return { kind: "load_file_diff", content: { request_id: `phone-${asked}`, path } };
}

/** Whether there's anything for the dock to show. @param {Todos | null} todos @param {Changes | null} changes */
export function hasDock(todos, changes) {
  return Boolean(todos?.total || changes?.total_files);
}

/** Todos the plan keeps in view below the active one, as the TUI's PLAN_LOOKAHEAD. */
const PLAN_LOOKAHEAD = 2;

/**
 * Where a plan that scrolls in its own box sits while nobody has moved it, as the TUI's
 * `plan_follow` has it: at the top until the todo in hand (else the first not done, else the last)
 * would fall below the box, then only far enough to keep it and the next couple in view.
 * @param {{ bottom: number, status: string }[]} rows Each todo's bottom edge in the list
 * @param {number} height The box's height @returns {number} The list's scroll offset
 */
export function planScroll(rows, height) {
  if (!rows.length) return 0;
  const active = rows.findIndex((row) => row.status === "in-progress");
  const open = rows.findIndex((row) => row.status !== "completed");
  const focus = active >= 0 ? active : open >= 0 ? open : rows.length - 1;
  const shown = rows[Math.min(focus + PLAN_LOOKAHEAD, rows.length - 1)];
  return Math.max(0, shown.bottom - height);
}
