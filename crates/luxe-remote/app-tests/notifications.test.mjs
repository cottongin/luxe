import { test } from "node:test";
import assert from "node:assert/strict";
import { keyBytes, support } from "../app/notifications.js";

// The break this catches: the switch offered where subscribing can only fail, or a Safari tab on
// iOS told notifications are impossible when the Home Screen is the way to them.
test("what the switch offers follows what the browser can do", () => {
  assert.equal(support({ worker: true, push: true, standalone: false }), "ready");
  assert.equal(support({ worker: true, push: true, standalone: true }), "ready");
  assert.equal(support({ worker: true, push: false, standalone: false }), "home-screen");
  assert.equal(support({ worker: false, push: true, standalone: false }), "home-screen");
  assert.equal(support({ worker: true, push: false, standalone: true }), "unsupported");
});

// The break this catches: the key passed as base64url text or decoded as plain base64, which
// PushManager refuses, so turning notifications on fails on every device.
test("the bridge's key reaches the browser as bytes", () => {
  assert.deepEqual([...keyBytes("AQID_-8")], [1, 2, 3, 255, 239]);
  const key = "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4";
  const bytes = keyBytes(key);
  assert.equal(bytes.length, 65);
  assert.equal(bytes[0], 4, "an uncompressed point");
});
