// @ts-check
// The Review tab's state: joining the open review, loading one when none is, keeping up when any
// client changes it, and the pieces it asks the host to draw.
import assert from "node:assert/strict";
import { test } from "node:test";
import { applyReview, chapterPiece, createReview, guidePiece, nextChapter, targetName, withoutLines } from "../app/review.js";
import { tapChapter } from "../app/review-view.js";

/** @param {string} id @param {string} text */
const hunk = (id, text) => ({ id, lines: [{ kind: "addition", text }] });

/** Two chapters: the first names one hunk of `a.rs` and all of `b.png`; the second `c.rs`'s hunk. */
function review() {
  return {
    id: "review:1",
    target: { kind: "working_tree" },
    chapters: [
      { id: "chapter:1", items: [{ kind: "hunk", id: "hunk:a1" }, { kind: "file", id: "file:b" }] },
      { id: "chapter:2", items: [{ kind: "hunk", id: "hunk:c1" }] },
    ],
    files: [
      { id: "file:a", hunks: [hunk("hunk:a1", "one"), hunk("hunk:a2", "two")] },
      { id: "file:b", hunks: [] },
      { id: "file:c", hunks: [hunk("hunk:c1", "three")] },
    ],
  };
}

/** A review tab and everything it sent. */
function tab() {
  /** @type {any[]} */
  const sent = [];
  const state = createReview();
  const send = (/** @type {any} */ frame) => sent.push(frame) > 0;
  return { state, sent, send, last: () => sent[sent.length - 1] };
}

/** The answer to the last `show_open_review` sent. @param {any[]} sent @param {any} content */
function answer(sent, content) {
  const asked = [...sent].reverse().find((frame) => frame.kind === "show_open_review");
  return { kind: "open_review_shown", content: { request_id: asked.content.request_id, ...content } };
}

// The break this catches: taking another phone's answer for this one's, or ignoring this one's
test("an answer shows only for the request that asked", () => {
  const { state, sent, send } = tab();
  state.ask(send);
  assert.equal(state.shown.value.state, "asking");
  state.apply({ kind: "open_review_shown", content: { request_id: "phone-review:show:other", review: review() } });
  assert.equal(state.shown.value.review, null);
  state.apply(answer(sent, { review: review(), reading: [{ chapter_id: "chapter:1", needs_review: 1 }] }));
  assert.equal(state.shown.value.state, "shown");
  assert.equal(state.shown.value.review?.id, "review:1");
  assert.equal(state.shown.value.reading.length, 1);
});

// The break this catches: two phones asking with the same id, each taking the other's answer
test("each ask carries an id of its own", () => {
  const { state, sent, send } = tab();
  state.ask(send);
  state.ask(send);
  assert.notEqual(sent[0].content.request_id, sent[1].content.request_id);
});

// The break this catches: a review with nothing open read as still loading, so the button to
// review the changes never shows
test("nothing open is shown as nothing open", () => {
  const { state, sent, send } = tab();
  state.ask(send);
  state.apply(answer(sent, { review: null, session: null, reading: [] }));
  assert.deepEqual([state.shown.value.state, state.shown.value.review], ["shown", null]);
});

// The break this catches: the guide's piece sent with every line of the review, or the lines
// stripped from the review itself, leaving a chapter nothing to show
test("the guide goes without lines and the review keeps them", () => {
  const full = review();
  const guide = withoutLines(full);
  assert.deepEqual(guide.files[0].hunks.map((/** @type {any} */ it) => [it.id, it.lines]), [["hunk:a1", []], ["hunk:a2", []]]);
  assert.equal(full.files[0].hunks[0].lines.length, 1);
});

// The break this catches: progress walked on the terminal not showing here, or the guide piece
// leaving out what it marks
test("walked chapters come from the session and reach the guide", () => {
  const { state, sent, send } = tab();
  state.ask(send);
  state.apply(answer(sent, { review: review(), session: { progress: { walked_chapter_ids: ["chapter:2"] } } }));
  const piece = guidePiece(state.shown.value);
  assert.deepEqual([piece.kind, piece.walked, piece.review.files[2].hunks[0].lines], ["review_guide", ["chapter:2"], []]);
});

// The break this catches: a review another client loaded or re-guided left stale here, or the
// tab asking before anyone opened it
test("a review loaded or re-guided anywhere is asked for again", () => {
  const { state, sent, send } = tab();
  state.apply({ kind: "guided_review_loaded", content: { request_id: "elsewhere" } });
  assert.equal(sent.length, 0, "not before the tab has asked");
  state.ask(send);
  state.apply(answer(sent, { review: review() }));
  state.apply({ kind: "guided_review_loaded", content: { request_id: "elsewhere" } });
  assert.equal(sent.length, 2);
  state.apply({ kind: "guided_review_guide_updated", content: {} });
  assert.equal(sent.length, 3);
  assert.equal(sent[2].kind, "show_open_review");
});

// The break this catches: loading something other than the working changes, or a failure of
// another client's load shown as this one's
test("loading reviews the working changes and says when it fails", () => {
  const { state, sent, send, last } = tab();
  state.load(send);
  assert.deepEqual([last().kind, last().content.target], ["load_guided_review", { kind: "working_tree" }]);
  assert.equal(state.shown.value.state, "loading");
  state.apply({ kind: "guided_review_failed", content: { request_id: "elsewhere", error: { message: "no" } } });
  assert.equal(state.shown.value.state, "loading");
  state.apply({ kind: "guided_review_failed", content: { request_id: sent[0].content.request_id, error: { message: "Not a repository" } } });
  assert.deepEqual([state.shown.value.state, state.shown.value.error], ["failed", "Not a repository"]);
});

// The break this catches: a lost connection leaving "Loading…" up forever, or a shown review
// thrown away when the connection comes back
test("a new connection loses only what was waiting", () => {
  const asking = tab();
  asking.state.ask(asking.send);
  asking.state.apply({ kind: "ready", content: {} });
  assert.equal(asking.state.shown.value.state, "lost");
  const shown = tab();
  shown.state.ask(shown.send);
  shown.state.apply(answer(shown.sent, { review: review() }));
  shown.state.apply({ kind: "ready", content: {} });
  assert.equal(shown.state.shown.value.state, "shown");
});

// The break this catches: a send that didn't go read as asking, so the tab waits on nothing
test("an ask that couldn't be sent is lost", () => {
  const state = createReview();
  state.ask(() => false);
  assert.equal(state.shown.value.state, "lost");
});

// The break this catches: a chapter from a replaced review left open, or the open one closed by a
// fresh answer for the same review
test("a chapter stays open only while the review has it", () => {
  const { state, sent, send } = tab();
  state.ask(send);
  state.apply(answer(sent, { review: review() }));
  state.pick("chapter:2");
  state.apply(answer(sent, { review: review() }));
  assert.equal(state.open.value, "chapter:2");
  const other = { ...review(), chapters: [{ id: "chapter:9", items: [] }] };
  state.apply(answer(sent, { review: other }));
  assert.equal(state.open.value, null);
});

// The break this catches: marking a chapter sending only that one (the session keeps the list it's
// sent, so the others would be unwalked), or unmarking doing nothing
test("marking a chapter reviewed sends every chapter walked, and unmarks", () => {
  const { state, sent, send, last } = tab();
  state.ask(send);
  state.apply(answer(sent, { review: review(), session: { progress: { walked_chapter_ids: ["chapter:2"] } } }));
  state.walk("chapter:1", send);
  assert.deepEqual(last(), { kind: "record_review_progress", content: { review_id: "review:1", walked_chapter_ids: ["chapter:2", "chapter:1"] } });
  state.walk("chapter:2", send);
  assert.deepEqual(last().content.walked_chapter_ids, ["chapter:1"]);
  assert.deepEqual(state.shown.value.walked, ["chapter:1"]);
});

// The break this catches: a chapter's piece carrying the whole review, a hunk from another
// chapter, or the wrong place in the list
test("a chapter's piece holds only its files and hunks", () => {
  const shown = /** @type {any} */ ({ review: review(), reading: [{ chapter_id: "chapter:1", needs_review: 2 }], walked: ["chapter:1"] });
  const piece = chapterPiece(shown, "chapter:1");
  assert.deepEqual(piece.files.map((/** @type {any} */ file) => [file.id, file.hunks.map((/** @type {any} */ it) => it.id)]), [["file:a", ["hunk:a1"]], ["file:b", []]]);
  assert.deepEqual(piece.place, { number: 1, total: 2, walked: true });
  assert.equal(piece.reading?.needs_review, 2);
  assert.equal(chapterPiece(shown, "chapter:2").reading, null);
});

// The break this catches: Next skipping a chapter, or offered past the last one
test("next is the chapter after, and nothing after the last", () => {
  assert.equal(nextChapter(review(), "chapter:1"), "chapter:2");
  assert.equal(nextChapter(review(), "chapter:2"), null);
});

// The break this catches: every review called the same thing, so the tab can't say what it's of
test("a review says what it's of", () => {
  assert.equal(targetName({ kind: "working_tree" }), "Working changes");
  assert.equal(targetName({ kind: "pull_request", number: 230 }), "Pull request #230");
  assert.equal(targetName({ kind: "commit", oid: "842ffb60c0ffee" }), "Commit 842ffb60");
  assert.equal(targetName({ kind: "base", reference: "main" }), "Changes since main");
});

// The break this catches: a chapter tap leaving a file or pull request in the pane, or a tap
// between chapters opening something
test("tapping a chapter opens it in place of the other panes", () => {
  const closed = /** @type {string[]} */ ([]);
  const state = createReview();
  const hooks = /** @type {any} */ ({ review: state, files: { close: () => closed.push("file") }, github: { close: () => closed.push("item") } });
  const event = (/** @type {any} */ found) => /** @type {any} */ ({ target: { closest: () => found } });
  tapChapter(event(null), hooks);
  assert.deepEqual([state.open.value, closed], [null, []]);
  tapChapter(event({ dataset: { chapter: "chapter:2" } }), hooks);
  assert.deepEqual([state.open.value, closed], ["chapter:2", ["file", "item"]]);
});

// The break this catches: a frame for nothing this tab asked changing it
test("other frames leave the tab alone", () => {
  const shown = /** @type {any} */ ({ state: "shown", review: null, guide: null, reading: [], walked: [] });
  assert.deepEqual(applyReview(shown, { kind: "turn_started", content: {} }, { id: "a", load: "b" }), { next: shown, again: false });
});
