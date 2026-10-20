// @ts-check
// Line picks: what a tap on a line's number picks, what a selection covers, and what Ask writes.
import assert from "node:assert/strict";
import { test } from "node:test";
import { askLabel, lineWords, markPicked, selectedLines, tapLine, tapped } from "../app/lines.js";

const at = (/** @type {number} */ first, last = first, path = "src/a.rs") => ({ path, first, last });

// The break this catches: a second tap starting over instead of stretching, a stretch that runs
// backwards when the second line is above the first, or a lone pick that can't be let go
test("a tap picks, a second stretches, and the same line lets go", () => {
  assert.deepEqual(tapLine(null, "src/a.rs", 12), at(12));
  assert.deepEqual(tapLine(at(12), "src/a.rs", 18), at(12, 18));
  assert.deepEqual(tapLine(at(12), "src/a.rs", 4), at(4, 12));
  assert.equal(tapLine(at(12), "src/a.rs", 12), null);
});

// The break this catches: a third tap stretching a stretch, or a tap in another file stretching
// across files
test("after a stretch, or in another file, a tap starts over", () => {
  assert.deepEqual(tapLine(at(12, 18), "src/a.rs", 30), at(30));
  assert.deepEqual(tapLine(at(12), "src/b.rs", 18), at(18, 18, "src/b.rs"));
});

// The break this catches: Ask writing something other than what the TUI sends for the same lines
// (its mention expands to the path in backticks, then `line 12` or `lines 12–18` with an en dash)
test("ask writes the path and lines as the TUI does", () => {
  assert.equal(lineWords(at(12)), "`src/a.rs` line 12");
  assert.equal(lineWords(at(12, 18)), "`src/a.rs` lines 12–18");
  assert.equal(askLabel(at(12)), "Ask about line 12");
  assert.equal(askLabel(at(12, 18)), "Ask about lines 12–18");
});

/** A row of the host's HTML: line `line` in the block for `path`. */
function row(/** @type {number | null} */ line, path = "src/a.rs") {
  const block = { getAttribute: () => path };
  const classes = new Set();
  const element = {
    getAttribute: (/** @type {string} */ name) => (name === "data-line" ? String(line) : null),
    closest: (/** @type {string} */ selector) => (selector === "[data-path]" ? block : line === null ? null : element),
    classList: { toggle: (/** @type {string} */ name, /** @type {boolean} */ on) => (on ? classes.add(name) : classes.delete(name)), has: (/** @type {string} */ name) => classes.has(name) },
  };
  return element;
}

/** A tap that landed on `row`'s number, or on its code. */
const landing = (/** @type {any} */ target, onNumber = true) => ({ closest: (/** @type {string} */ selector) => (selector === ".ln" && onNumber ? target : null) });

// The break this catches: a tap on the code (where a selection starts) picking the line, or a tap
// on a number not reading its line and file from the host's HTML
test("only a tap on a line's number picks it", () => {
  assert.deepEqual(tapped(null, /** @type {any} */ (landing(row(7)))), at(7));
  assert.equal(tapped(at(7), /** @type {any} */ (landing(row(9), false))), null, "a tap elsewhere lets go");
  assert.equal(tapped(at(7), null), null);
});

/** A selection whose range touches `touched` rows of a block holding `rows`. */
function selecting(/** @type {any[]} */ rows, /** @type {any[]} */ touched, inside = true, path = "src/a.rs") {
  const block = { nodeType: 1, getAttribute: () => path, closest: () => block, querySelectorAll: () => rows };
  const range = { commonAncestorContainer: block, intersectsNode: (/** @type {any} */ node) => touched.includes(node) };
  const selection = { isCollapsed: false, rangeCount: 1, getRangeAt: () => range };
  const root = { contains: () => inside };
  return { selection: /** @type {any} */ (selection), root: /** @type {any} */ (root) };
}

// The break this catches: a selection that starts partway through a line not counting it, or the
// lines read from the first and last rows rather than every row touched
test("a selection offers every line it touches", () => {
  const rows = [row(3), row(4), row(5), row(6)];
  const { selection, root } = selecting(rows, [rows[1], rows[2]]);
  assert.deepEqual(selectedLines(selection, root), at(4, 5));
});

// The break this catches: Ask offered for nothing (a collapsed caret, a selection in another pane,
// or one across deleted lines alone, which no line in the file stands for)
test("a selection offers nothing when it covers no line of this pane", () => {
  const rows = [row(3), row(4)];
  assert.equal(selectedLines({ ...selecting(rows, rows).selection, isCollapsed: true }, selecting(rows, rows).root), null);
  const outside = selecting(rows, rows, false);
  assert.equal(selectedLines(outside.selection, outside.root), null);
  const none = selecting(rows, []);
  assert.equal(selectedLines(none.selection, none.root), null);
  assert.equal(selectedLines(null, none.root), null);
});

// The break this catches: rows outside the pick, or in another file with the same line numbers,
// marked as picked, or a let-go pick leaving its marks behind
test("only the picked rows of the picked file are marked", () => {
  const rows = [row(3), row(4), row(5), row(6), row(4, "src/b.rs")];
  const root = /** @type {any} */ ({ querySelectorAll: () => rows });
  markPicked(root, at(4, 5));
  assert.deepEqual(rows.map((it) => it.classList.has("picked")), [false, true, true, false, false]);
  markPicked(root, null);
  assert.deepEqual(rows.map((it) => it.classList.has("picked")), [false, false, false, false, false]);
});
