// @ts-check
// Which computer the app reaches, as the bridge names the page for it.
import assert from "node:assert/strict";
import { test } from "node:test";
import { machineName } from "../app/sessions.js";

/** A page whose `luxe-machine` meta says `name`, or that has none. */
const page = (/** @type {string | null} */ name) => ({
  querySelector: (/** @type {string} */ selector) =>
    selector === 'meta[name="luxe-machine"]' && name !== null ? { getAttribute: () => name } : null,
});

// The break this catches: the session list of two computers' apps looking the same because the
// meta goes unread, or a page without one (Tailscale couldn't say) showing a stray label
test("the app names the computer its page is named for, and nothing without a name", () => {
  assert.equal(machineName(/** @type {any} */ (page("desk"))), "desk");
  assert.equal(machineName(/** @type {any} */ (page(null))), "");
});
