// What the box offers as you type, and what a pick does to the draft.

import assert from "node:assert/strict";
import { test } from "node:test";
import { COMMANDS } from "../app/commands.js";
import { afterPick, pickerNote, suggestions } from "../app/picker.js";

/** @import { Entry, Listing } from "../app/mentions.js" */

/** @param {string} text */
const at = (text) => ({ text, caret: text.length });
/** @type {Entry} */
const MAIN = { name: "main.rs", kind: "file" };
/** @type {Listing} */
const SRC = { state: "listed", entries: [{ name: "cli", kind: "directory" }, MAIN] };
/** @type {Listing} */
const ASKING = { state: "asking", entries: [] };
/** @type {Listing} */
const FAILED = { state: "failed", entries: [] };
/** @param {string} name */
const command = (name) => COMMANDS.find((entry) => entry.name === name);

// The break this catches: commands labeled without their slash, or without the TUI's title
test("a slash offers commands, labeled as the TUI's", () => {
  const offered = suggestions(at("/co"), {});
  assert.deepEqual(offered.map((item) => [item.label, item.hint]), [["/compact", "Compact the conversation"], ["/cost", "Session cost"]]);
});

// The break this catches: a folder shown like a file, so it's not clear a tap opens it
test("a mention offers the folder's entries, folders marked with a slash", () => {
  const offered = suggestions(at("@src/"), { src: SRC });
  assert.deepEqual(offered.map((item) => item.label), ["cli/", "main.rs"]);
});

// The break this catches: suggestions for a folder that hasn't been listed taken from another
test("nothing is offered from a folder not listed yet", () => {
  assert.deepEqual(suggestions(at("@lib/"), { src: SRC }), []);
});

// The break this catches: a plain message offering anything
test("a plain message offers nothing", () => {
  assert.deepEqual(suggestions(at("hello"), { src: SRC }), []);
});

// The break this catches: a command that takes words run before they're written
test("a command that takes words goes into the box to finish", () => {
  assert.deepEqual(afterPick(at("/co"), { key: "compact", label: "", hint: "", command: command("compact") }), {
    text: "/compact ",
    caret: 9,
    placed: true,
  });
});

// The break this catches: a command that takes nothing put in the box, a second tap to run it
test("a command that takes nothing runs", () => {
  assert.equal(afterPick(at("/mo"), { key: "model", label: "", hint: "", command: command("model") }), null);
});

// The break this catches: the caret not placed after the pick, so typing goes in before it
test("a picked file replaces the mention and places the caret after it", () => {
  assert.deepEqual(afterPick(at("@src/ma"), { key: "main.rs", label: "", hint: "", entry: MAIN }), {
    text: "`src/main.rs` ",
    caret: 14,
    placed: true,
  });
});

// The break this catches: a folder on its way shown as empty, which reads as no files
test("a folder being listed says so", () => {
  assert.equal(pickerNote(at("@src/"), { src: ASKING }), "Listing files…");
});

// The break this catches: an empty folder showing no picker at all, which reads as broken, or a
// folder with files said to be empty
test("an empty folder says so", () => {
  /** @type {Listing} */
  const empty = { state: "listed", entries: [] };
  assert.equal(pickerNote(at("@.luxe/"), { ".luxe": empty }), "No files in .luxe");
  assert.equal(pickerNote(at("@src/zz"), { src: SRC }), "");
});

// The break this catches: a folder that can't be listed shown as empty, or the project root named
// as nothing
test("a folder that can't be listed is named", () => {
  assert.equal(pickerNote(at("@gone/"), { gone: FAILED }), "Couldn't list gone");
  assert.equal(pickerNote(at("@"), { "": FAILED }), "Couldn't list the project");
});
