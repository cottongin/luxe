// Asking the host for rendered text: batching, keeping, and failing.

import assert from "node:assert/strict";
import { test } from "node:test";
import { createRenderer } from "../app/render.js";

/** A host that answers each piece with its own JSON, and remembers every batch it was sent. */
function fakeHost() {
  const batches = [];
  const post = async (pieces) => {
    batches.push(pieces);
    return pieces.map((piece) => `<p>${JSON.stringify(piece)}</p>`);
  };
  return { batches, post };
}

/** @param {string} text @returns {import("../app/render.js").Piece} */
const answer = (text) => ({ kind: "answer", text });

// The break this catches: a request per piece, so a transcript of a hundred messages costs a
// hundred round trips; or answers handed back by position wrongly, so one message shows another's
test("pieces asked for together go in one request and come back to their askers", async () => {
  const host = fakeHost();
  const renderer = createRenderer(host.post);
  const [first, second] = await Promise.all([
    renderer.render(answer("one"), { keep: false }),
    renderer.render(answer("two"), { keep: false }),
  ]);
  assert.equal(host.batches.length, 1);
  assert.equal(first, '<p>{"kind":"answer","text":"one"}</p>');
  assert.equal(second, '<p>{"kind":"answer","text":"two"}</p>');
});

// The break this catches: finished messages asked for again on every redraw and reconnect
test("a finished piece is kept and not asked for again", async () => {
  const host = fakeHost();
  const renderer = createRenderer(host.post);
  await renderer.render(answer("done"), { keep: true });
  const again = await renderer.render(answer("done"), { keep: true });
  assert.equal(host.batches.length, 1);
  assert.equal(again, renderer.peek(answer("done")));
});

// The break this catches: every version of a streaming reply kept, which fills the cache with
// prefixes nobody will see again
test("a growing piece isn't kept", async () => {
  const host = fakeHost();
  const renderer = createRenderer(host.post);
  await renderer.render(answer("grow"), { keep: false });
  assert.equal(renderer.peek(answer("grow")), undefined);
  await renderer.render(answer("grow"), { keep: false });
  assert.equal(host.batches.length, 2);
});

// The break this catches: a failed request leaving its askers waiting forever, so those messages
// never fall back from plain text and never ask again
test("a failed request fails every piece in it", async () => {
  const renderer = createRenderer(async () => {
    throw new Error("rendering failed: 502");
  });
  const asked = [renderer.render(answer("a"), { keep: true }), renderer.render(answer("b"), { keep: true })];
  for (const one of asked) await assert.rejects(one, /502/);
  assert.equal(renderer.peek(answer("a")), undefined);
});
