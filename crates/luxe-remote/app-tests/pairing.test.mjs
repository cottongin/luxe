// Reading an invitation out of what someone pasted.

import assert from "node:assert/strict";
import { test } from "node:test";
import { invitationFrom } from "../app/pairing.js";

/** A token as `luxe remote pair` makes one: 32 random bytes, URL-safe base64 without padding. */
const TOKEN = "q3J0-Zx_8fLmN2pQrStUvWxYz0123456789AbCdEfGh";

// The break this catches: the whole link sent as the invitation, which the bridge can't match
test("the token is read out of the printed link", () => {
  assert.equal(invitationFrom(`https://desk.tail1234.ts.net/pair#${TOKEN}`), TOKEN);
});

// The break this catches: insisting on a whole link, when a notes app or a message may carry only
// the part after the #, or a copy picks up the line's spaces
test("the token alone is taken, spaces and all", () => {
  assert.equal(invitationFrom(TOKEN), TOKEN);
  assert.equal(invitationFrom(`  ${TOKEN}\n`), TOKEN);
});

// The break this catches: a link without its invitation, or text that isn't one, enabling Pair
// for a request that can only fail
test("nothing that can't be an invitation is taken for one", () => {
  for (const text of ["", "https://desk.tail1234.ts.net/pair", "https://desk.tail1234.ts.net/pair#", "two words"]) {
    assert.equal(invitationFrom(text), null, JSON.stringify(text));
  }
});
