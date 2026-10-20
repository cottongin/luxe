// @ts-check
// Comments on the open review from the phone: where a new one goes, and the `edit_review` edit each
// action sends. The review desk answers every edit with the whole session, so nothing here keeps
// comments of its own.

/** @import { LinePick } from "./lines.js" */

/**
 * Where a comment on `pick` goes: a line for one picked line, its hunk for a run inside one hunk,
 * and the chapter for anything wider or for no pick at all. A line is the new side's, the only
 * side a pick can name.
 * @param {any} review @param {string} chapterId @param {LinePick | null} pick
 */
export function anchorFor(review, chapterId, pick) {
  const chapter = { kind: "chapter", chapter_id: chapterId };
  const file = pick && review.files.find((/** @type {any} */ it) => it.new_path === pick.path);
  const holds = (/** @type {any} */ hunk, /** @type {number} */ line) => hunk.lines.some((/** @type {any} */ it) => it.new_line === line);
  const hunk = pick && file?.hunks.find((/** @type {any} */ it) => holds(it, pick.first));
  if (!pick || !hunk) return chapter;
  if (pick.first === pick.last) return { kind: "line", file_id: file.id, hunk_id: hunk.id, side: "new", line: pick.first };
  return holds(hunk, pick.last) ? { kind: "hunk", file_id: file.id, hunk_id: hunk.id } : chapter;
}

/** The composer's heading for a comment at `anchor`. @param {any} anchor @param {LinePick | null} pick */
export function anchorWords(anchor, pick) {
  if (anchor.kind === "line") return `Comment on line ${anchor.line}`;
  if (anchor.kind === "hunk" && pick) return `Comment on lines ${pick.first}–${pick.last}`;
  return "Comment on this chapter";
}

/**
 * A new comment, unsent unless `send`. A question asks for an answer; anything else is an issue,
 * the review desk's default for a comment.
 * @param {any} anchor @param {string} body @param {boolean} question @param {boolean} send
 */
export function commentEdit(anchor, body, question, send) {
  return { edit: "comment", anchor, body, severity: question ? "question" : "issue", send };
}

/**
 * The edit a card's button asks for. A reply goes to the agent at once, as a thread reply does in
 * the TUI.
 * @param {string} act @param {string} findingId @param {string} [body]
 */
export function actEdit(act, findingId, body = "") {
  switch (act) {
    case "send": return { edit: "send", finding_ids: [findingId] };
    case "delete": return { edit: "delete_comment", finding_id: findingId };
    case "resolve": return { edit: "resolve", finding_id: findingId };
    case "reopen": return { edit: "reopen", finding_id: findingId };
    case "reply": return { edit: "reply", finding_id: findingId, body, send: true };
    default: return null;
  }
}

/** The comments not sent yet, which Send all sends. A posted one is GitHub's. @param {any} session @returns {string[]} */
export function unsent(session) {
  const drafts = (session?.findings ?? []).filter((/** @type {any} */ it) => it.state === "draft" && !it.posted);
  return drafts.map((/** @type {any} */ it) => it.id);
}

/** A card's label, for the reply composer's heading. @param {any} session @param {string} findingId */
export function labelOf(session, findingId) {
  return session?.findings?.find((/** @type {any} */ it) => it.id === findingId)?.label ?? "";
}
