// `@` mentions of project files: reading one back from the box, and the folders listed for it.

import assert from "node:assert/strict";
import { test } from "node:test";
import { createListings, matchEntries, mentionAt, pickEntry } from "../app/mentions.js";

/** @import { Entry } from "../app/mentions.js" */

/** @param {string} name @returns {Entry} */
const dir = (name) => ({ name, kind: "directory" });
/** @param {string} name @returns {Entry} */
const file = (name) => ({ name, kind: "file" });

// The break this catches: the mention missed at the start of the box, where most begin
test("an @ at the start is a mention", () => {
  assert.deepEqual(mentionAt("@ma", 3), { start: 0, folder: "", name: "ma" });
});

// The break this catches: the folder and name split anywhere but the last slash
test("what's typed splits into a folder and a name at the last slash", () => {
  const text = "look at @src/cli/ma";
  assert.deepEqual(mentionAt(text, text.length), { start: 8, folder: "src/cli", name: "ma" });
});

// The break this catches: an address read as a mention, which pops the picker mid-email
test("an @ inside a word isn't a mention", () => {
  assert.equal(mentionAt("me@example.com", 14), null);
});

// The break this catches: the picker kept up after the mention's done and a space typed
test("a space ends the mention", () => {
  assert.equal(mentionAt("@src ", 5), null);
});

// The break this catches: the caret ignored, so a mention typed mid-message reads the end instead
test("the mention is the one just before the caret", () => {
  assert.deepEqual(mentionAt("@do and more", 3), { start: 0, folder: "", name: "do" });
});

// The break this catches: the ranks lost, so a name holding the text comes before one starting
// with it, or one holding it nowhere stays in
test("names starting with what's typed lead, then names holding it", () => {
  const entries = [file("domain.rs"), file("main.rs"), file("Makefile"), file("lib.rs")];
  assert.deepEqual(matchEntries(entries, "ma").map((entry) => entry.name), ["main.rs", "Makefile", "domain.rs"]);
});

// The break this catches: case counted, so a capital the keyboard put in hides every file
test("names match whatever the case typed", () => {
  assert.deepEqual(matchEntries([file("main.rs")], "MA").map((entry) => entry.name), ["main.rs"]);
});

// The break this catches: a file put in bare or without the folder it's in, which isn't what the
// TUI sends the model, or the caret left before it
test("a picked file goes in as its path in backticks", () => {
  const text = "fix @src/ma please";
  const query = /** @type {any} */ (mentionAt(text, 11));
  assert.deepEqual(pickEntry(text, 11, query, file("main.rs")), { text: "fix `src/main.rs`  please", caret: 18 });
});

// The break this catches: a picked folder closing the mention, so its files can't be reached
test("a picked folder opens it for the next level", () => {
  const query = /** @type {any} */ (mentionAt("@sr", 3));
  assert.deepEqual(pickEntry("@sr", 3, query, dir("src")), { text: "@src/", caret: 5 });
});

/**
 * Listings that have asked for `folders`, the frames they sent, and the id of the nth question.
 * @param {string[]} folders
 */
function listingsAsked(...folders) {
  const listings = createListings();
  /** @type {import("../app/connection.js").Frame[]} */
  const frames = [];
  for (const folder of folders) listings.ask(folder, (frame) => (frames.push(frame), true));
  return { listings, frames, idOf: (/** @type {number} */ nth) => frames[nth].content.request_id };
}

// The break this catches: the question in a shape `list_directory` doesn't take, or asked twice
test("a folder is asked for once, as list_directory", () => {
  const { listings, frames, idOf } = listingsAsked("src", "src");
  assert.deepEqual(frames, [{ kind: "list_directory", content: { request_id: idOf(0), path: "src" } }]);
  assert.equal(listings.folders.value.src.state, "asking");
});

// The break this catches: ids counted from one on every phone, so two phones in one session file
// each other's answers, since every client hears every answer
test("no two phones ask with the same id", () => {
  const first = listingsAsked("src");
  const second = listingsAsked("src");
  assert.notEqual(first.idOf(0), second.idOf(0));
});

// The break this catches: the answer filed under the wrong folder, or its entries read from the
// wrong field
test("the answer is filed under the folder that was asked for", () => {
  const { listings, idOf } = listingsAsked("", "src");
  const listing = { path: "src", entries: [file("main.rs")], omitted: 0 };
  listings.apply({ kind: "directory_listed", content: { request_id: idOf(1), listing } });
  assert.deepEqual(listings.folders.value.src, { state: "listed", entries: [file("main.rs")] });
  assert.equal(listings.folders.value[""].state, "asking");
});

// The break this catches: a folder that can't be listed left asking forever
test("a folder that can't be listed says so", () => {
  const { listings, idOf } = listingsAsked("gone");
  listings.apply({ kind: "directory_list_failed", content: { request_id: idOf(0), path: "gone", error: "no such folder" } });
  assert.equal(listings.folders.value.gone.state, "failed");
});

// The break this catches: another client's answer, or a stale one, filed as if it were ours
test("an answer to a question not asked here changes nothing", () => {
  const { listings } = listingsAsked("src");
  const listing = { path: "lib", entries: [], omitted: 0 };
  listings.apply({ kind: "directory_listed", content: { request_id: "listing:elsewhere", listing } });
  assert.deepEqual(Object.keys(listings.folders.value), ["src"]);
});

// The break this catches: a question that never left counted as asked, so it's never asked again
test("a question that couldn't be sent is asked again next time", () => {
  const listings = createListings();
  listings.ask("src", () => false);
  assert.equal(listings.folders.value.src, undefined);
});

/** Listings with `src` listed as holding `main.rs`, and the frames sent so far. */
function srcListed() {
  const asked = listingsAsked("src");
  const listing = { path: "src", entries: [file("main.rs")], omitted: 0 };
  asked.listings.apply({ kind: "directory_listed", content: { request_id: asked.idOf(0), listing } });
  return asked;
}

// The break this catches: the Files tab shown a folder as it was when the picker first listed it,
// missing what the agent has written since
test("a fresh ask lists a known folder again, keeping its entries up meanwhile", () => {
  const { listings, frames } = srcListed();
  listings.ask("src", (frame) => (frames.push(frame), true), { fresh: true });
  assert.equal(frames.length, 2);
  assert.deepEqual(listings.folders.value.src, { state: "asking", entries: [file("main.rs")] });
});

// The break this catches: a fresh ask sent again while one is on its way, doubling every request
// each time the view draws
test("a fresh ask waits for the one on its way", () => {
  const { listings, frames } = listingsAsked("src");
  listings.ask("src", (frame) => (frames.push(frame), true), { fresh: true });
  assert.equal(frames.length, 1);
});

// The break this catches: a folder asked for just before the connection dropped left loading
// forever, since the new connection never answers the old question
test("a new connection forgets the folders still on their way, and only those", () => {
  const { listings, frames } = srcListed();
  listings.ask("lib", (frame) => (frames.push(frame), true));
  listings.apply({ kind: "ready", content: {} });
  assert.deepEqual(Object.keys(listings.folders.value), ["src"]);
});
