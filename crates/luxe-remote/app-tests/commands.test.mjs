// The slash commands the phone carries out, and the frames they send.

import assert from "node:assert/strict";
import { test } from "node:test";
import { commandAction, matchCommands, runCommand } from "../app/commands.js";

const names = (commands) => commands.map((command) => command.name);

// The break this catches: the picker missing at the bare slash, which is where you'd browse
test("a bare slash offers every command", () => {
  assert.deepEqual(names(matchCommands("/")), ["compact", "cost", "dock", "mode", "model", "thinking"]);
});

// The break this catches: names matched anywhere but their start, or with case counted
test("what's typed narrows by the start of the name", () => {
  assert.deepEqual(names(matchCommands("/mo")), ["mode", "model"]);
  assert.deepEqual(names(matchCommands("/MOD")), ["mode", "model"]);
  assert.deepEqual(names(matchCommands("/st")), []);
});

// The break this catches: the picker staying up over the words a command takes, or over a
// message that happens to hold a slash
test("nothing is offered once there's a space, or no slash first", () => {
  assert.deepEqual(matchCommands("/compact the API"), []);
  assert.deepEqual(matchCommands("see a/b"), []);
});

// The break this catches: the focus dropped, so the summary keeps what it likes
test("compact sends its focus", () => {
  assert.deepEqual(commandAction("/compact the API decisions", "build"), {
    frame: { kind: "compact_now", content: { focus: "the API decisions" } },
  });
});

// The break this catches: an empty focus sent, which the summary would be told to keep
test("compact alone sends no focus", () => {
  assert.deepEqual(commandAction("/compact", "build"), { frame: { kind: "compact_now" } });
});

// The break this catches: a bare `/mode` sent as a mode named nothing, rather than the next one
test("mode alone moves to the next mode", () => {
  assert.deepEqual(commandAction("/mode", "build"), { frame: { kind: "set_mode", content: "plan" } });
});

// The break this catches: a named mode ignored for the next one, or any word sent as a mode
test("mode takes plan or build, and anything else goes to the agent", () => {
  assert.deepEqual(commandAction("/mode build", "build"), { frame: { kind: "set_mode", content: "build" } });
  assert.equal(commandAction("/mode fast", "build"), null);
});

// The break this catches: the pickers' commands sent to the agent as a message
test("model, thinking and cost open the details", () => {
  for (const name of ["model", "thinking", "cost"]) assert.deepEqual(commandAction(`/${name}`, "build"), { details: true });
});

// The break this catches: a bare `/dock` doing nothing, where the TUI's toggles
test("dock alone toggles", () => {
  assert.deepEqual(commandAction("/dock", "build"), { dock: "toggle" });
});

// The break this catches: one of the TUI's `/dock` verbs dropped, or any word taken for one
test("dock takes the TUI's verbs, and anything else goes to the agent", () => {
  for (const verb of ["show", "hide", "auto", "left", "right"]) assert.deepEqual(commandAction(`/dock ${verb}`, "build"), { dock: verb });
  assert.deepEqual(commandAction("/dock Left", "build"), { dock: "left" });
  assert.equal(commandAction("/dock is in the way", "build"), null);
});

/** Hooks that record what a command did. */
function recorded() {
  /** @type {unknown[]} */
  const did = [];
  const hooks = {
    send: (/** @type {unknown} */ frame) => (did.push(frame), true),
    details: () => void did.push("details"),
    dock: (/** @type {string} */ verb) => void did.push(`dock ${verb}`),
  };
  return { did, hooks };
}

// The break this catches: a command routed to the wrong place, the sheet's commands sent to the
// session or the dock's left undone, or a message run as a command
test("each command goes where it's carried out", () => {
  const { did, hooks } = recorded();
  assert.equal(runCommand({ details: true }, hooks), true);
  assert.equal(runCommand({ dock: "left" }, hooks), true);
  assert.equal(runCommand({ frame: { kind: "compact_now" } }, hooks), true);
  assert.equal(runCommand(null, hooks), null);
  assert.deepEqual(did, ["details", "dock left", { kind: "compact_now" }]);
});

// The break this catches: a frame that couldn't go out reported as sent, so the box clears and
// the command is lost
test("a command frame that couldn't be sent says so", () => {
  const hooks = { ...recorded().hooks, send: () => false };
  assert.equal(runCommand({ frame: { kind: "compact_now" } }, hooks), false);
});

// The break this catches: a command the keyboard capitalized sent to the agent as a message
test("a command counts whatever its case", () => {
  assert.deepEqual(commandAction("/Model", "build"), { details: true });
});

// The break this catches: a known command given words it doesn't take swallowed, so the message
// never reaches the agent
test("a command given words it doesn't take is a message", () => {
  assert.equal(commandAction("/model is slow today", "build"), null);
});

// The break this catches: a slash that isn't one of the phone's commands kept from the agent
test("anything else that starts with a slash is a message", () => {
  assert.equal(commandAction("/review the diff", "build"), null);
  assert.equal(commandAction("not /compact", "build"), null);
});
