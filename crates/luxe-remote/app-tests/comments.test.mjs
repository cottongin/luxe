// @ts-check
// Comments from the phone: where one goes, the edits the buttons send, and the session they answer.
import assert from "node:assert/strict";
import { test } from "node:test";
import { actEdit, anchorFor, anchorWords, commentEdit, labelOf, unsent } from "../app/comments.js";
import { composedEdit, tapCard } from "../app/comments-view.js";
import { applyReview, chapterPiece, createReview } from "../app/review.js";

const line = (/** @type {number | null} */ n) => ({ new_line: n });
/** `src/a.rs` with two hunks (lines 1–3, then 10–11), and a chapter naming both. */
const REVIEW = {
  id: "review:1",
  chapters: [{ id: "chapter:1", items: [{ kind: "hunk", id: "hunk:1" }, { kind: "hunk", id: "hunk:2" }] }],
  files: [{ id: "file:a", new_path: "src/a.rs", hunks: [
    { id: "hunk:1", lines: [line(1), line(2), line(null), line(3)] },
    { id: "hunk:2", lines: [line(10), line(11)] },
  ] }],
};
const pick = (/** @type {number} */ first, last = first, path = "src/a.rs") => ({ path, first, last });

// The break this catches: a one-line pick anchored anywhere but its line (the comment lands on the
// wrong code), or on the old side, which a pick can't name
test("one picked line is a line comment on the new side", () => {
  assert.deepEqual(anchorFor(REVIEW, "chapter:1", pick(2)), { kind: "line", file_id: "file:a", hunk_id: "hunk:1", side: "new", line: 2 });
  assert.deepEqual(anchorFor(REVIEW, "chapter:1", pick(11)).hunk_id, "hunk:2");
});

// The break this catches: a run inside one hunk anchored to its first line only, or a run across
// hunks pinned to one of them
test("a run is its hunk's comment, and a wider one the chapter's", () => {
  assert.deepEqual(anchorFor(REVIEW, "chapter:1", pick(1, 3)), { kind: "hunk", file_id: "file:a", hunk_id: "hunk:1" });
  assert.deepEqual(anchorFor(REVIEW, "chapter:1", pick(2, 10)), { kind: "chapter", chapter_id: "chapter:1" });
});

// The break this catches: a comment with no pick, or on a line the review doesn't hold, sent with
// an anchor the desk refuses
test("no pick, or a line the review doesn't hold, comments on the chapter", () => {
  const chapter = { kind: "chapter", chapter_id: "chapter:1" };
  assert.deepEqual(anchorFor(REVIEW, "chapter:1", null), chapter);
  assert.deepEqual(anchorFor(REVIEW, "chapter:1", pick(2, 2, "src/b.rs")), chapter);
  assert.deepEqual(anchorFor(REVIEW, "chapter:1", pick(7)), chapter);
});

// The break this catches: the composer saying "line 12" over a comment going on the chapter
test("the composer says where the comment goes", () => {
  assert.equal(anchorWords({ kind: "line", line: 12 }, pick(12)), "Comment on line 12");
  assert.equal(anchorWords({ kind: "hunk" }, pick(1, 3)), "Comment on lines 1–3");
  assert.equal(anchorWords({ kind: "chapter" }, null), "Comment on this chapter");
});

// The break this catches: a question sent as an issue (the agent fixes instead of answering), or
// Save sending what was meant to wait
test("a comment carries its kind and whether it goes now", () => {
  const anchor = { kind: "chapter", chapter_id: "chapter:1" };
  assert.deepEqual(commentEdit(anchor, "Why?", true, false), { edit: "comment", anchor, body: "Why?", severity: "question", send: false });
  assert.equal(commentEdit(anchor, "Fix it", false, true).severity, "issue");
  assert.equal(commentEdit(anchor, "Fix it", false, true).send, true);
});

// The break this catches: a card's button sending another edit than it says, or a reply left
// unsent (the TUI's go at once)
test("each card button sends its own edit", () => {
  assert.deepEqual(actEdit("send", "f:1"), { edit: "send", finding_ids: ["f:1"] });
  assert.deepEqual(actEdit("delete", "f:1"), { edit: "delete_comment", finding_id: "f:1" });
  assert.deepEqual(actEdit("resolve", "f:1"), { edit: "resolve", finding_id: "f:1" });
  assert.deepEqual(actEdit("reopen", "f:1"), { edit: "reopen", finding_id: "f:1" });
  assert.deepEqual(actEdit("reply", "f:1", "Thanks"), { edit: "reply", finding_id: "f:1", body: "Thanks", send: true });
  assert.equal(actEdit("explode", "f:1"), null);
});

// The break this catches: Send all re-sending sent comments, or posting one already on GitHub
test("only drafts not on GitHub are unsent", () => {
  const session = { findings: [
    { id: "f:1", state: "draft", label: "R1" }, { id: "f:2", state: "open", label: "R2" },
    { id: "f:3", state: "draft", posted: { url: "x" }, label: "R3" },
  ] };
  assert.deepEqual(unsent(session), ["f:1"]);
  assert.deepEqual(unsent(null), []);
  assert.equal(labelOf(session, "f:2"), "R2");
});

/** A tap on a card's `act` button, and what it did. */
function tapAct(/** @type {string | null} */ act) {
  const edits = /** @type {any[]} */ ([]);
  const composed = /** @type {any[]} */ ([]);
  const card = { getAttribute: () => "f:1" };
  const button = act && { getAttribute: () => act, closest: () => card };
  const event = /** @type {any} */ ({ target: { closest: () => button } });
  const review = { shown: { value: { session: { findings: [{ id: "f:1", label: "R1" }] } } }, edit: (/** @type {any} */ _send, /** @type {any} */ edit) => edits.push(edit) };
  tapCard(event, /** @type {any} */ ({ review, send: () => true }), (composing) => composed.push(composing));
  return { edits, composed };
}

// The break this catches: Reply sending an empty reply instead of opening the composer, or a tap
// between buttons sending something
test("Reply opens the composer and other buttons send", () => {
  assert.deepEqual(tapAct("reply"), { edits: [], composed: [{ heading: "Reply to R1", reply: "f:1" }] });
  assert.deepEqual(tapAct("resolve").edits, [{ edit: "resolve", finding_id: "f:1" }]);
  assert.deepEqual(tapAct(null), { edits: [], composed: [] });
});

// The break this catches: the composer's words sent as a new comment when they reply to one
test("the composer replies or comments as it was opened", () => {
  assert.equal(composedEdit({ heading: "", reply: "f:1" }, "Yes", false, false).edit, "reply");
  assert.equal(composedEdit({ heading: "", anchor: { kind: "review" } }, "No", true, true).severity, "question");
});

/** A review tab showing REVIEW, and everything it sent. */
function shownTab() {
  const sent = /** @type {any[]} */ ([]);
  const state = createReview();
  const send = (/** @type {any} */ frame) => sent.push(frame) > 0;
  state.ask(send);
  state.apply({ kind: "open_review_shown", content: { request_id: sent[0].content.request_id, review: REVIEW, session: { id: "review:1", findings: [] } } });
  return { state, sent, send };
}

// The break this catches: comments made in the TUI, or the desk's answer to the phone's own edit,
// not reaching the phone, or another review's session taken for this one
test("a saved session for the open review is the one shown", () => {
  const { state } = shownTab();
  const session = { id: "review:1", findings: [{ id: "f:1" }], progress: { walked_chapter_ids: ["chapter:1"] } };
  state.apply({ kind: "review_session_saved", content: { request_id: "elsewhere", session } });
  assert.deepEqual([state.shown.value.session, state.shown.value.walked], [session, ["chapter:1"]]);
  state.apply({ kind: "review_session_saved", content: { request_id: "x", session: { id: "review:9", findings: [] } } });
  assert.equal(state.shown.value.session, session);
});

// The break this catches: an edit sent without the open review's id, a refusal of it unsaid, or
// another client's refusal shown as this one's
test("an edit names the open review and its refusal is said", () => {
  const { state, sent, send } = shownTab();
  state.edit(send, { edit: "resolve", finding_id: "f:1" });
  const edit = sent[sent.length - 1];
  assert.deepEqual([edit.kind, edit.content.review_id, edit.content.edit], ["edit_review", "review:1", "resolve"]);
  state.apply({ kind: "review_session_save_failed", content: { request_id: "elsewhere", message: "no" } });
  assert.equal(state.shown.value.problem, undefined);
  state.apply({ kind: "review_session_save_failed", content: { request_id: edit.content.request_id, message: "That comment was sent already." } });
  assert.equal(state.shown.value.problem, "That comment was sent already.");
});

// The break this catches: an edit that couldn't be sent read as sent, or an edit with no review
// open sent anyway
test("an edit that can't go says so, and none goes without a review", () => {
  const { state } = shownTab();
  state.edit(() => false, { edit: "resolve", finding_id: "f:1" });
  assert.match(state.shown.value.problem ?? "", /Connection lost/);
  const none = createReview();
  const sent = /** @type {any[]} */ ([]);
  none.edit((frame) => sent.push(frame) > 0, { edit: "resolve", finding_id: "f:1" });
  assert.equal(sent.length, 0);
});

// The break this catches: a chapter drawn without the session's comments, so none ever show
test("a chapter's piece carries the session's comments", () => {
  const findings = [{ id: "f:1" }];
  const piece = chapterPiece(/** @type {any} */ ({ review: REVIEW, reading: [], walked: [], session: { findings } }), "chapter:1");
  assert.equal(piece.findings, findings);
  assert.deepEqual(chapterPiece(/** @type {any} */ ({ review: REVIEW, reading: [], walked: [] }), "chapter:1").findings, []);
  assert.deepEqual(applyReview(/** @type {any} */ ({ review: null }), { kind: "review_session_saved", content: { session: { id: "review:1" } } }, { id: "", load: "" }).next, { review: null });
});
