// What the composer says while a turn runs, since its buttons are icons.

import assert from "node:assert/strict";
import { test } from "node:test";
import { sendLabel, steeringLine } from "../app/composer.js";

// The break this catches: a line shown between turns, which says sending steers when it starts one
test("idle, with nothing waiting, there's no line", () => {
  assert.equal(steeringLine(false, 0, false), "");
});

// The break this catches: the running turn left unsaid, so the icon gives no hint a message steers
test("while a turn runs, the line says sending steers it", () => {
  assert.equal(steeringLine(true, 0, false), "Send steers the turn");
});

// The break this catches: a message with a photo promised to steer, when it waits for the turn
test("with a photo attached, the line says it waits for the turn", () => {
  assert.equal(steeringLine(true, 0, true), "Photos wait for the turn");
});

// The break this catches: waiting steering hidden behind the running-turn hint, or once the turn
// ends, though the engine holds it for the next one and it can still be withdrawn
test("steering that waits is counted, running or not", () => {
  assert.equal(steeringLine(true, 1, false), "1 message waiting to steer");
  assert.equal(steeringLine(false, 3, false), "3 messages waiting to steer");
});

// The break this catches: the send icon's spoken name left at Send while it steers or queues
test("the send button's name follows what it does", () => {
  assert.equal(sendLabel(false, true), "Send");
  assert.equal(sendLabel(true, false), "Steer");
  assert.equal(sendLabel(true, true), "Queue");
});
