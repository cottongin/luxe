// The wordmark, read from the file the terminal's art generates.

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { driftPhase, loadWordmark, readDrawing } from "../app/wordmark.js";

const GENERATED = readFileSync(new URL("../app/generated/wordmark.svg", import.meta.url), "utf8");

// The break this catches: a letter, the box or the drift's depth lost between the generated file
// and the page, which draws a mark missing a letter or swinging by nothing
test("the generated wordmark gives its box, its depth and every letter", () => {
  const drawing = readDrawing(GENERATED);
  assert.equal(drawing.viewBox, "0 0 440 154");
  assert.equal(drawing.depth, "0.55");
  assert.equal(drawing.paths.length, 5);
  assert.ok(drawing.paths.every((d) => d.startsWith("M")), "each path is geometry, not a fill");
});

// The break this catches: a file without the depth drawn anyway, its letters never moving
test("a file without the drift's depth isn't drawn", () => {
  assert.equal(readDrawing(GENERATED.replace(/ data-drift-depth="[^"]+"/, "")), null);
});

// The break this catches: a file with no letters drawn as an empty box where the name was
test("a file without letters isn't drawn", () => {
  assert.equal(readDrawing(GENERATED.replace(/<path[^>]*>/g, "")), null);
});

// The break this catches: the letters in step, which pulses the whole mark, or spread over a whole
// swing, which makes it symmetrical about its middle the way the terminal's never is
test("half a swing is spread from the first letter to the last", () => {
  assert.equal(driftPhase(0, 5), -1);
  assert.equal(driftPhase(4, 5), -0.5);
  assert.equal(driftPhase(2, 5), -0.75);
});

// The break this catches: a one-letter mark dividing by nothing and losing its animation to NaN
test("a single letter still has a phase", () => {
  assert.equal(driftPhase(0, 1), -1);
});

// The break this catches: the drawing asked for somewhere the bridge doesn't serve it, or a
// refusal taken for a drawing
test("the wordmark is read from the app's generated folder", async () => {
  const asked = [];
  const get = async (url) => {
    asked.push(url);
    return new Response("missing", { status: 404 });
  };
  await assert.rejects(loadWordmark(get), { message: "the wordmark answered 404" });
  assert.deepEqual(asked, ["/app/generated/wordmark.svg"]);
});
