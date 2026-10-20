// @ts-check
// The Review tab's state: the review open in the session, joined rather than replaced, and the one
// chapter open in the peek.
//
// A session holds one review, and loading one makes a new one with no comments or progress, so the
// tab asks for whatever is open (`show_open_review`) and offers to review the working changes only
// when nothing is. A review loaded or re-guided afterwards, by any client, is the open one now, so
// the tab asks again. The guide goes to the host without its hunks' lines, which it doesn't draw;
// what needs reading comes with the answer, since it's decided from those lines. Comments change
// through `edit_review`, and every change, whoever made it, comes back as the whole session.

import { signal } from "@preact/signals";

/** @import { Frame } from "./connection.js" */
/**
 * @typedef {"idle" | "asking" | "loading" | "shown" | "failed" | "lost"} ReviewState
 * @typedef {{ state: ReviewState, review: any | null, guide: any | null, reading: any[], walked: string[],
 *   session?: any, error?: string, problem?: string }} ReviewShown
 * @typedef {(frame: Frame) => boolean} Send
 */

/** @type {ReviewShown} */
const NOTHING = { state: "idle", review: null, guide: null, reading: [], walked: [] };

/** Nothing asked and no chapter open. */
export function createReview() {
  const shown = signal(NOTHING);
  const open = signal(/** @type {string | null} */ (null));
  /** @type {Asked} */
  const asked = { id: "", load: "", send: null, edits: new Set() };
  /** @param {Send} send */
  const ask = (send) => (shown.value = { ...shown.value, state: askOpen(asked, send) ? "asking" : "lost" });
  /** @param {Send} send */
  const load = (send) => (shown.value = { ...shown.value, state: loadChanges(asked, send) ? "loading" : "lost", error: undefined });
  /** @param {Frame} frame */
  const apply = (frame) => {
    const { next, again } = applyReview(shown.value, frame, asked);
    shown.value = next;
    if (!next.review?.chapters?.some((/** @type {any} */ it) => it.id === open.value)) open.value = null;
    if (again && asked.send) ask(asked.send);
  };
  const walk = (/** @type {string} */ chapter, /** @type {Send} */ send) => (shown.value = walked(shown.value, chapter, send));
  const edit = (/** @type {Send} */ send, /** @type {object | null} */ change) => (shown.value = edited(shown.value, asked, send, change));
  return { shown, open, ask, load, apply, walk, edit, pick: (/** @type {string} */ id) => (open.value = id), close: () => (open.value = null) };
}

/**
 * Sends `change` to the open review, remembering the request so a refusal of it is said. The
 * session the desk answers with arrives through `apply`, as everyone's changes do.
 * @param {ReviewShown} shown @param {Asked} asked @param {Send} send @param {object | null} change @returns {ReviewShown}
 */
function edited(shown, asked, send, change) {
  if (!shown.review || !change) return shown;
  const request_id = requestId("edit");
  asked.edits.add(request_id);
  const sent = send({ kind: "edit_review", content: { request_id, review_id: shown.review.id, ...change } });
  return { ...shown, problem: sent ? undefined : "Connection lost. Try again once it's back." };
}

/**
 * The requests the next answers must carry, and the send to ask again with when the open review
 * changes. @typedef {{ id: string, load: string, send: Send | null, edits: Set<string> }} Asked
 */

/** Asks for the open review. @param {Asked} asked @param {Send} send */
function askOpen(asked, send) {
  Object.assign(asked, { id: requestId("show"), send });
  return send({ kind: "show_open_review", content: { request_id: asked.id } });
}

/** Loads a review of the working changes, which becomes the open one. @param {Asked} asked @param {Send} send */
function loadChanges(asked, send) {
  Object.assign(asked, { load: requestId("load"), send });
  return send({ kind: "load_guided_review", content: { request_id: asked.load, target: { kind: "working_tree" } } });
}

/** An id no other phone will send, so each answers only its asker. @param {string} what */
function requestId(what) {
  return `phone-review:${what}:${Date.now().toString(36)}${Math.random().toString(36).slice(2, 8)}`;
}

/**
 * What `frame` does to the tab, and whether to ask for the open review again because it changed.
 * @param {ReviewShown} shown @param {Frame} frame @param {{ id: string, load: string, edits?: Set<string> }} asked
 * @returns {{ next: ReviewShown, again: boolean }}
 */
export function applyReview(shown, { kind, content }, asked) {
  const waiting = shown.state === "asking" || shown.state === "loading";
  if (kind === "ready") return { next: waiting ? { ...shown, state: "lost" } : shown, again: false };
  if (kind === "open_review_shown" && content?.request_id === asked.id) return { next: answered(content), again: false };
  if (kind === "review_session_saved" && shown.review && content?.session?.id === shown.review.id) {
    return { next: withSession(shown, content.session), again: false };
  }
  if (kind === "review_session_save_failed" && asked.edits?.has(content?.request_id)) {
    return { next: { ...shown, problem: content.message }, again: false };
  }
  if (kind === "guided_review_failed" && content?.request_id === asked.load) {
    return { next: { ...shown, state: "failed", error: content.error?.message ?? "The review couldn't be loaded." }, again: false };
  }
  return { next: shown, again: kind === "guided_review_loaded" || kind === "guided_review_guide_updated" };
}

/** The tab after an answer: the review, its guide without lines, and the reading it came with. @param {any} content @returns {ReviewShown} */
function answered({ review, session, reading }) {
  if (!review) return { ...NOTHING, state: "shown" };
  return withSession({ state: "shown", review, guide: withoutLines(review), reading: reading ?? [], walked: [] }, session);
}

/** The tab holding `session`, its comments and the chapters walked in it. @param {ReviewShown} shown @param {any} session @returns {ReviewShown} */
function withSession(shown, session) {
  return { ...shown, session, walked: session?.progress?.walked_chapter_ids ?? shown.walked };
}

/** `review` with every hunk's lines left out, which is all the guide's piece needs. @param {any} review */
export function withoutLines(review) {
  const files = review.files.map((/** @type {any} */ file) => ({ ...file, hunks: file.hunks.map((/** @type {any} */ hunk) => ({ ...hunk, lines: [] })) }));
  return { ...review, files };
}

/**
 * `chapter` marked walked, or unmarked if it was, with the session told every chapter walked now.
 * @param {ReviewShown} shown @param {string} chapter @param {Send} send @returns {ReviewShown}
 */
function walked(shown, chapter, send) {
  if (!shown.review) return shown;
  const was = shown.walked.includes(chapter);
  const walked = was ? shown.walked.filter((id) => id !== chapter) : [...shown.walked, chapter];
  send({ kind: "record_review_progress", content: { review_id: shown.review.id, walked_chapter_ids: walked } });
  return { ...shown, walked };
}

/** The guide's piece. @param {ReviewShown} shown */
export function guidePiece({ guide, reading, walked }) {
  return { kind: /** @type {const} */ ("review_guide"), review: guide, reading, walked };
}

/**
 * Chapter `id`'s piece: the chapter, and only the files it names holding only its hunks, so the
 * request stays the size of the chapter rather than the review. @param {ReviewShown} shown @param {string} id
 */
export function chapterPiece({ review, reading, walked, session }, id) {
  const index = review.chapters.findIndex((/** @type {any} */ chapter) => chapter.id === id);
  const chapter = review.chapters[index];
  const named = new Set(chapter.items.map((/** @type {any} */ item) => item.id));
  const files = review.files
    .map((/** @type {any} */ file) => ({ ...file, hunks: file.hunks.filter((/** @type {any} */ hunk) => named.has(hunk.id)) }))
    .filter((/** @type {any} */ file) => named.has(file.id) || file.hunks.length);
  const place = { number: index + 1, total: review.chapters.length, walked: walked.includes(id) };
  const read = reading.find((/** @type {any} */ it) => it.chapter_id === id) ?? null;
  const findings = session?.findings ?? [];
  return { kind: /** @type {const} */ ("review_chapter"), chapter, files, place, reading: read, findings };
}

/** The chapter after `id`, if there is one. @param {any} review @param {string} id @returns {string | null} */
export function nextChapter(review, id) {
  const index = review.chapters.findIndex((/** @type {any} */ chapter) => chapter.id === id);
  return review.chapters[index + 1]?.id ?? null;
}

/** What the review is of, in a few words. @param {any} target */
export function targetName(target) {
  switch (target?.kind) {
    case "working_tree": return "Working changes";
    case "pull_request": return `Pull request #${target.number}`;
    case "commit": return `Commit ${String(target.oid).slice(0, 8)}`;
    case "base": return `Changes since ${target.reference}`;
    default: return "Review";
  }
}
