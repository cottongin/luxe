// Keeping iOS from zooming into a focused field, and nothing else from pinching.

import assert from "node:assert/strict";
import { test } from "node:test";
import { holdFieldZoom, isAppleTouch } from "../app/viewport.js";

const IPHONE = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_5 like Mac OS X) AppleWebKit/605.1.15";
const MAC = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15";
const ANDROID = "Mozilla/5.0 (Linux; Android 15; Pixel 9) AppleWebKit/537.36 Chrome/139.0";
const START = "width=device-width, initial-scale=1, viewport-fit=cover";

/** A page with one viewport tag, and what it says after `holdFieldZoom`. */
function zoomHeldOn(device, content = START) {
  const viewport = { content };
  const page = { querySelector: (selector) => (selector === 'meta[name="viewport"]' ? viewport : null) };
  holdFieldZoom(/** @type {any} */ (page), /** @type {any} */ (device));
  return viewport.content;
}

// The break this catches: the iPhone's own name missed, so every field zooms the page on focus
test("an iPhone is an Apple touch device", () => {
  assert.equal(isAppleTouch({ userAgent: IPHONE, platform: "iPhone", maxTouchPoints: 5 }), true);
});

// The break this catches: an iPad, which names itself a Mac, taken for one, so its fields zoom
test("an iPad is told from a Mac by its touch screen", () => {
  assert.equal(isAppleTouch({ userAgent: MAC, platform: "MacIntel", maxTouchPoints: 5 }), true);
  assert.equal(isAppleTouch({ userAgent: MAC, platform: "MacIntel", maxTouchPoints: 0 }), false);
});

// The break this catches: the hold applied everywhere, which stops Android from pinching at all
test("Android is left able to pinch", () => {
  const device = { userAgent: ANDROID, platform: "Linux armv8l", maxTouchPoints: 5 };
  assert.equal(zoomHeldOn(device), START);
});

// The break this catches: the hold left out of the viewport tag on iOS, so a field still zooms
test("iOS gets maximum-scale added to the viewport", () => {
  const device = { userAgent: IPHONE, platform: "iPhone", maxTouchPoints: 5 };
  assert.equal(zoomHeldOn(device), `${START}, maximum-scale=1`);
});

// The break this catches: a second run adding the setting again, which a reload or a test would do
test("the hold is added once", () => {
  const device = { userAgent: IPHONE, platform: "iPhone", maxTouchPoints: 5 };
  assert.equal(zoomHeldOn(device, `${START}, maximum-scale=1`), `${START}, maximum-scale=1`);
});
