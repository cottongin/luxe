// The Files tab: the tree's rows and taps, reading a file, its answer, and mentioning it.

import assert from "node:assert/strict";
import { test } from "node:test";
import { applyRead, childPath, createFiles, entriesNote, fileMention, toggled, treeRows, withMention } from "../app/files.js";
import { createGithub } from "../app/github.js";
import { createReview } from "../app/review.js";
import { tapRow } from "../app/files-view.js";

/** @import { OpenFile } from "../app/files.js" */

/** @type {OpenFile} */
const ASKING = { path: "src/a.rs", requestId: "file:1", state: "asking" };
/** A `project_file_read` frame's file, copied from `luxe --rpc`. */
const FILE = { path: "a.rs", text: "fn main() {}\n", bytes: 13, truncated: false, binary: false };

// The break this catches: the question in a shape `read_project_file` doesn't take, or its id
// counted rather than random, so two phones in one session open each other's files
test("reading a file asks read_project_file with an id of its own", () => {
  /** @type {any[]} */
  const frames = [];
  const files = createFiles();
  files.read("src/a.rs", (frame) => (frames.push(frame), true));
  files.read("src/a.rs", (frame) => (frames.push(frame), true));
  assert.deepEqual(frames[0], { kind: "read_project_file", content: { request_id: frames[0].content.request_id, path: "src/a.rs" } });
  assert.match(frames[0].content.request_id, /^file:/);
  assert.notEqual(frames[0].content.request_id, frames[1].content.request_id);
  assert.equal(files.open.value?.state, "asking");
});

// The break this catches: a read that never left shown as loading forever
test("a read that couldn't be sent shows as lost", () => {
  const files = createFiles();
  files.read("a.rs", () => false);
  assert.equal(files.open.value?.state, "lost");
});

// The break this catches: the answer's file read from the wrong field
test("the answer to the open file shows it", () => {
  const read = applyRead(ASKING, { kind: "project_file_read", content: { request_id: "file:1", file: FILE } });
  assert.deepEqual(read, { ...ASKING, state: "read", file: FILE });
});

// The break this catches: a failure left loading, or its reason dropped
test("a failed read says why", () => {
  const failed = applyRead(ASKING, { kind: "project_file_read_failed", content: { request_id: "file:1", path: "src/a.rs", error: "not_found" } });
  assert.deepEqual(failed, { ...ASKING, state: "failed", error: "not_found" });
});

// The break this catches: another phone's answer, or one for a file since closed, shown here
test("an answer to another question changes nothing", () => {
  const frame = { kind: "project_file_read", content: { request_id: "file:2", file: FILE } };
  assert.equal(applyRead(ASKING, frame), ASKING);
  assert.equal(applyRead(null, frame), null);
});

// The break this catches: a read asked for before the connection dropped left loading forever
test("a new connection marks a read still on its way as lost", () => {
  assert.equal(applyRead(ASKING, { kind: "ready", content: {} })?.state, "lost");
});

// The break this catches: a file already shown thrown back to lost by a reconnect
test("a new connection leaves a file already read alone", () => {
  /** @type {OpenFile} */
  const read = { ...ASKING, state: "read", file: FILE };
  assert.equal(applyRead(read, { kind: "ready", content: {} }), read);
});

// The break this catches: a root entry asked for as `/name`, which isn't a project path
test("a path inside the root has no leading slash", () => {
  assert.equal(childPath("", "src"), "src");
  assert.equal(childPath("src/app", "a.rs"), "src/app/a.rs");
});

/** Listings as `directory_listed` leaves them: the root, `src` and `src/app`. */
const FOLDERS = /** @type {Record<string, import("../app/mentions.js").Listing>} */ ({
  "": { state: "listed", entries: [{ name: "src", kind: "directory" }, { name: "README.md", kind: "file" }] },
  src: { state: "listed", entries: [{ name: "app", kind: "directory" }, { name: "lib.rs", kind: "file" }] },
  "src/app": { state: "listed", entries: [{ name: "a.rs", kind: "file" }] },
});

// The break this catches: an open folder's entries left out, placed after its siblings rather than
// under it, at the wrong depth, or by name alone rather than their whole path
test("an open folder's entries sit under it, a level in", () => {
  assert.deepEqual(treeRows(FOLDERS, new Set(["src"])), [
    { kind: "folder", path: "src", name: "src", depth: 0, open: true },
    { kind: "folder", path: "src/app", name: "app", depth: 1, open: false },
    { kind: "file", path: "src/lib.rs", name: "lib.rs", depth: 1 },
    { kind: "file", path: "README.md", name: "README.md", depth: 0 },
  ]);
});

// The break this catches: a folder open inside a closed one shown anyway, which the TUI hides until
// the closed one opens again
test("a closed folder hides what's open inside it", () => {
  const rows = treeRows(FOLDERS, new Set(["src/app"]));
  assert.deepEqual(rows.map((row) => row.kind === "note" ? row.text : row.path), ["src", "README.md"]);
});

// The break this catches: an open folder not listed yet shown as empty, or its note at the
// folder's own depth rather than where its entries will be
test("an open folder still being listed says so where its entries will be", () => {
  const rows = treeRows({ "": FOLDERS[""] }, new Set(["src"]));
  assert.deepEqual(rows[1], { kind: "note", depth: 1, text: "Loading…" });
  assert.deepEqual(treeRows({}, new Set()), [{ kind: "note", depth: 0, text: "Loading…" }]);
});

// The break this catches: a tap that only ever opens, so a folder can't be closed again, or one
// that closes every other folder too
test("toggling opens a closed folder and closes an open one", () => {
  const opened = toggled(new Set(["docs"]), "src");
  assert.deepEqual([...opened], ["docs", "src"]);
  assert.deepEqual([...toggled(opened, "src")], ["docs"]);
});

/**
 * A tap on a tree row with `dataset`, and what it asked for.
 * @param {Record<string, string>} dataset @param {Iterable<string>} [unfolded]
 */
function tap(dataset, unfolded = []) {
  /** @type {any[]} */
  const asked = [];
  const files = createFiles();
  files.unfolded.value = new Set(unfolded);
  const listings = /** @type {any} */ ({ ask: (/** @type {any[]} */ ...args) => asked.push(args) });
  const event = /** @type {any} */ ({ target: { closest: () => ({ dataset }) } });
  const send = () => true;
  const github = createGithub();
  github.open.value = /** @type {any} */ ({ subject: { number: 1 } });
  const review = createReview();
  review.open.value = "chapter:1";
  tapRow(event, /** @type {any} */ ({ files, github, review, listings, send }));
  return { files, github, review, asked, send };
}

// The break this catches: a folder opened without being listed afresh, so it shows what was there
// before the agent changed it, or listed again on closing, which asks for nothing anyone sees
test("opening a folder lists it afresh and closing it doesn't", () => {
  const opened = tap({ folder: "src" });
  assert.deepEqual([...opened.files.unfolded.value], ["src"]);
  assert.deepEqual(opened.asked, [["src", opened.send, { fresh: true }]]);
  const closed = tap({ folder: "src" }, ["src"]);
  assert.deepEqual([...closed.files.unfolded.value], []);
  assert.deepEqual(closed.asked, []);
});

// The break this catches: a file tap that unfolds nothing but doesn't open the file either, opens
// the wrong path, or leaves an open pull request in the pane over it
test("tapping a file opens it", () => {
  const { files, github, review } = tap({ file: "src/lib.rs" });
  assert.equal(files.open.value?.path, "src/lib.rs");
  assert.deepEqual([...files.unfolded.value], []);
  assert.equal(github.open.value, null);
  assert.equal(review.open.value, null, "one pane at a time");
});

// The break this catches: a folder tap that closes the open pull request, which only a file
// taking the pane should
test("tapping a folder leaves the pane alone", () => {
  const { github, review } = tap({ folder: "src" });
  assert.notEqual(github.open.value, null);
  assert.equal(review.open.value, "chapter:1");
});

// The break this catches: a mention run into the word before it, or a file not in the backticks
// the `@` picker writes, which is what the TUI sends the model for a file
test("a mention goes at the end of the draft, spaced from what's there", () => {
  assert.deepEqual(withMention("look at", fileMention("a.rs")), { text: "look at `a.rs` ", caret: 15, placed: true });
  assert.deepEqual(withMention("look at ", "#7").text, "look at #7 ");
  assert.deepEqual(withMention("", "#7").text, "#7 ");
});

// The break this catches: an empty folder shown as loading, a failure as empty, or a folder being
// listed again hidden behind "Loading…" when its last entries are there to show
test("a folder with nothing to list says why", () => {
  assert.equal(entriesNote(undefined), "Loading…");
  assert.equal(entriesNote({ state: "asking", entries: [] }), "Loading…");
  assert.equal(entriesNote({ state: "listed", entries: [] }), "Empty folder");
  assert.equal(entriesNote({ state: "failed", entries: [] }), "Couldn't list this folder");
  assert.equal(entriesNote({ state: "asking", entries: [{ name: "a", kind: "file" }] }), "");
});
