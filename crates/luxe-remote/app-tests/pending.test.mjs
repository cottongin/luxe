// Prompts and steering, folded from frames, and the frames the phone sends back.

import assert from "node:assert/strict";
import { test } from "node:test";
import { createPending, messageFrame, permissionReply, questionReply } from "../app/pending.js";
import { permissionChoices, toggled } from "../app/prompt.js";

const frame = (kind, content) => ({ kind, content });
const asked = (id) => frame("permission_requested", {
  id, session_id: "s", tool: "bash", permission: "bash", summary: "Run: rm -rf build", always_patterns: ["rm *"],
});
const ids = (pending) => pending.prompts.value.map((prompt) => prompt.id);

function fold(frames) {
  const pending = createPending();
  frames.forEach(pending.apply);
  return pending;
}

// The break this catches: `prompt_closed` taking down every prompt, or the wrong one, so a
// question answered at the desk clears the approval still waiting on the phone
test("a prompt closes by its own id, whoever answered it", () => {
  const pending = fold([asked("perm:1"), asked("perm:2"), asked("perm:3"), frame("prompt_closed", { id: "perm:2" })]);
  assert.deepEqual(ids(pending), ["perm:1", "perm:3"]);
});

// The break this catches: a question shown as a permission card, or its options lost
test("a question opens as a question, with its options", () => {
  const options = [{ label: "postgres", description: "Relational", recommended: true }];
  const pending = fold([frame("question_asked", { id: "ask:1", session_id: "s", prompt: "Which?", options, multi: false })]);
  const [prompt] = pending.prompts.value;
  assert.equal(prompt.kind, "question");
  assert.deepEqual(prompt.options, options);
});

// The break this catches: the same prompt sent twice (a snapshot racing the live event) drawn as
// two, so answering one leaves a card behind that can't be answered
test("a prompt already open isn't opened twice", () => {
  assert.deepEqual(ids(fold([asked("perm:1"), asked("perm:1")])), ["perm:1"]);
});

// The break this catches: a reconnect keeping the last connection's prompts, which the snapshot
// sends again if they're still open, so one answered meanwhile lingers on the phone
test("a new connection starts with nothing waiting", () => {
  const pending = fold([asked("perm:1"), frame("ready", {})]);
  pending.steered();
  pending.apply(frame("ready", {}));
  assert.deepEqual(ids(pending), []);
  assert.equal(pending.steering.value, 0);
});

// The break this catches: a delivery clearing only some of the tray, or a turn ending clearing
// steering the engine holds for the next turn, as the TUI's tray keeps it
test("steering waits through a turn's end, and a checkpoint delivers all of it", () => {
  const pending = createPending();
  pending.steered();
  pending.steered();
  pending.apply(frame("turn_ended", { reason: "end_turn" }));
  assert.equal(pending.steering.value, 2);
  pending.apply(frame("steering_delivered", { count: 1 }));
  assert.equal(pending.steering.value, 0);
});

// The break this catches: Stop or withdraw leaving the tray saying messages wait that the engine
// has thrown away
test("dropping steering empties the tray", () => {
  const pending = createPending();
  pending.steered();
  pending.dropSteering();
  assert.equal(pending.steering.value, 0);
});

// The break this catches: a reply frame the host's decoder doesn't know, which it rejects, so a
// tap on the phone answers nothing; shapes as `rpc.md` documents them
test("replies are the protocol's frames", () => {
  assert.deepEqual(permissionReply("perm:1", "always"), { kind: "respond_permission", content: { id: "perm:1", reply: "always" } });
  assert.deepEqual(questionReply("ask:1", ["postgres", "my own"]), {
    kind: "answer_question", content: { id: "ask:1", reply: { answered: ["postgres", "my own"] } },
  });
  assert.deepEqual(questionReply("ask:1", null), { kind: "answer_question", content: { id: "ask:1", reply: "cancelled" } });
});

// The break this catches: writing while a turn runs sent as a new message, which waits for the
// turn to end, instead of steering the one running
test("a message steers while a turn runs, and starts one otherwise", () => {
  assert.deepEqual(messageFrame("hi", false), { kind: "submit_user_message", content: { text: "hi" } });
  assert.deepEqual(messageFrame("hi", true), { kind: "steer_turn", content: { text: "hi" } });
});

// The break this catches: "remember" offered for a call with no pattern to remember, which the
// engine can only treat as once; or the options out of the TUI's order
test("the remembering answers appear only when the call has a pattern", () => {
  /** @param {string[]} always_patterns */
  const asking = (always_patterns) => permissionChoices({ kind: "permission", id: "p", tool: "bash", summary: "", always_patterns });
  const replies = (patterns) => asking(patterns).map((choice) => choice.reply);
  assert.deepEqual(replies([]), ["once", "reject"]);
  assert.deepEqual(replies(["rm *"]), ["once", "always", "always_persist", "reject"]);
  assert.match(asking(["rm *"])[1].hint, /rm \*/);
});

// The break this catches: ticking an option twice leaving it ticked, so it can't be taken back
test("ticking an option again unticks it", () => {
  assert.deepEqual(toggled(["a"], "b"), ["a", "b"]);
  assert.deepEqual(toggled(["a", "b"], "a"), ["b"]);
});
