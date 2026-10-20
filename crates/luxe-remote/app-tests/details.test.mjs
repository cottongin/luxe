// The details sheet's readings, from the markup the bridge renders for the status line.

import assert from "node:assert/strict";
import { test } from "node:test";
import { hasReadings } from "../app/details.js";

const MODEL = '<span class="seg seg-model">Scripted</span>';
const THINKING = '<span class="seg seg-thinking"><span class="seg-glyph" aria-hidden="true">x</span> off</span>';

// The break this catches: a SESSION heading over nothing, when the line holds only the model and
// thinking the sheet already has pickers for
test("a line with only the model and thinking has no readings", () => {
  assert.equal(hasReadings(THINKING + MODEL), false);
});

// The break this catches: the context reading missed, so a filling window goes unshown
test("the context is a reading", () => {
  assert.equal(hasReadings(`<span class="seg seg-context tone-error">95%/200k</span>${MODEL}`), true);
});

// The break this catches: the cost missed, which the phone's line leaves to the sheet alone
test("the cost is a reading", () => {
  assert.equal(hasReadings(`<span class="seg seg-cost">$1.20</span>${MODEL}`), true);
});

// The break this catches: nothing rendered yet read as something, or a crash before the first render
test("nothing rendered has no readings", () => {
  assert.equal(hasReadings(null), false);
});
