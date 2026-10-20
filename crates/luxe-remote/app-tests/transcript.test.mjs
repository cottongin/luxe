// The transcript fold, against a real recorded session and against one frame at a time.

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { WINDOW, elapsed, windowOf, windowStart } from "../app/items.js";
import { createTranscript, formatTokens } from "../app/transcript.js";

/** A session joined through the bridge: the snapshot, then a live turn with two tool calls. */
const recorded = readFileSync(new URL("fixtures/turn.jsonl", import.meta.url), "utf8")
  .trim()
  .split("\n")
  .map((line) => JSON.parse(line));

function fold(frames) {
  const transcript = createTranscript();
  frames.forEach(transcript.apply);
  return transcript;
}

function shape(transcript) {
  return transcript.items.value.map((item) => {
    if (item.card) return `tool ${item.card.value.name} ${item.card.value.state}`;
    return `${item.kind}${item.provisional ? " (streaming)" : ""}`;
  });
}

/** @import { Item, TextItem, ToolItem } from "../app/transcript.js" */

/** `item`, which the test expects to be text. @param {Item} item @returns {TextItem} */
function asText(item) {
  assert.ok(item.kind !== "tool", `${item.key} is a tool card`);
  return item;
}

/** `item`, which the test expects to be a tool card. @param {Item} item @returns {ToolItem} */
function asTool(item) {
  assert.ok(item.kind === "tool", `${item.key} isn't a tool card`);
  return item;
}

const frame = (kind, content) => ({ kind, content });
const reply = (...content) => frame("message_committed", { message: { role: "assistant", content } });
const card = (revision, state) =>
  frame("tool_activity_updated", { call_id: "c1", name: "bash", label: "ls", state, revision });

// The break this catches: a photo's bytes kept for the render request, sent back to the host on
// every redraw; or the blocks that aren't text dropped, which moves hook context to block 0, where
// the host reads it as the reader's own words
test("a user message keeps its blocks for rendering, an image as just its type", () => {
  const image = { type: "image", content: { media_type: "image/png", data: "iVBORw0KGgo=" } };
  const words = { type: "text", content: "what's this?" };
  const message = { role: "user", content: [image, words] };
  const transcript = fold([frame("message_committed", { message })]);
  const item = asText(transcript.items.value[0]);
  assert.equal(item.text.value, "what's this?");
  assert.deepEqual(item.content, [{ type: "image" }, words]);
});

// The break this catches: a commit that adds the saved reply beside the streamed one, so every
// answer shows twice; or a turn whose question never arrives, so the phone shows answers alone.
test("a recorded turn folds into what the TUI shows", () => {
  const items = shape(fold(recorded));
  const lastTurn = items.slice(items.lastIndexOf("user"));
  assert.deepEqual(lastTurn, ["user", "thinking", "tool bash succeeded", "tool read succeeded", "assistant"]);
  assert.equal(items.filter((item) => item === "user").length, 4);
});

// The break this catches: lining blocks up by position whatever their kind, so the saved
// reasoning lands in the reply's place and the reply goes missing.
test("streamed blocks that don't match the saved ones are replaced by them", () => {
  const transcript = fold([
    frame("token_delta", { block: 0, delta: "Hel" }),
    reply({ type: "thinking", content: { text: "why", signature: "" } }, { type: "text", content: "Hello" }),
  ]);
  assert.deepEqual(shape(transcript), ["thinking", "assistant"]);
  assert.deepEqual(transcript.items.value.map((item) => asText(item).text.value), ["why", "Hello"]);
});

// The break this catches: swapping a streamed reply for a new item on commit, which redraws it,
// so a long answer flickers and the page jumps as it finishes.
test("streamed blocks that match keep their place and take the saved text", () => {
  const transcript = fold([
    frame("token_delta", { block: 0, delta: "Hello wor" }),
  ]);
  const streamed = asText(transcript.items.value[0]);
  transcript.apply(reply({ type: "text", content: "Hello world\n\n" }));
  assert.equal(transcript.items.value[0], streamed);
  assert.equal(streamed.text.value, "Hello world");
  assert.deepEqual(shape(transcript), ["assistant"]);
});

// The break this catches: a rejoin, which replays the snapshot, adding the whole history again
// under what is already there.
test("joining again starts the transcript over", () => {
  const once = shape(fold(recorded)).length;
  assert.equal(shape(fold([...recorded, ...recorded])).length, once);
});

// The break this catches: taking whatever card update arrives last, so a late, older update puts a
// finished call back to running.
test("an older card update doesn't undo a newer one", () => {
  assert.deepEqual(shape(fold([card(3, "succeeded"), card(2, "running")])), ["tool bash succeeded"]);
});

// The break this catches: a call the session never finished showing as running forever.
test("a call without a result when the session loads shows as interrupted", () => {
  const call = { type: "tool_call", content: { id: "c1", name: "bash", arguments: { command: "make" } } };
  const loaded = frame("session_loaded", { messages: [{ message: { role: "assistant", content: [call] } }] });
  assert.deepEqual(shape(fold([loaded])), ["tool bash interrupted"]);
});

// The break this catches: reading every saved result as a success, so a failed call looks fine.
test("a saved result settles its card, failures as failures", () => {
  const call = { type: "tool_call", content: { id: "c1", name: "bash", arguments: { command: "make" } } };
  const result = {
    type: "tool_result",
    content: { call_id: "c1", content: [{ type: "text", content: "no rule" }], is_error: true },
  };
  const loaded = frame("session_loaded", {
    messages: [{ message: { role: "assistant", content: [call] } }, { message: { role: "tool", content: [result] } }],
  });
  const transcript = fold([loaded]);
  assert.deepEqual(shape(transcript), ["tool bash failed"]);
  assert.equal(asTool(transcript.items.value[0]).card.value.output_preview, "no rule");
});

// The break this catches: a frame the host refused vanishing, so a reply or a message sent from
// the phone seems to go nowhere
test("a refused action shows as an error, with the host's reason", () => {
  const transcript = fold([frame("action_rejected", { kind: "respond_permission", reason: "no such prompt" })]);
  assert.deepEqual(shape(transcript), ["error"]);
  assert.match(asText(transcript.items.value[0]).text.value, /no such prompt/);
});

// The break this catches: the composer never offering to steer, since the engine announces no
// turn's start; or a turn kept running after it ended, so a new message is sent as steering
test("a turn runs from its first sign until it ends", () => {
  const transcript = createTranscript();
  const user = frame("message_committed", { message: { role: "user", content: [{ type: "text", content: "hi" }] } });
  /** @type {[import("../app/connection.js").Frame, boolean][]} */
  const steps = [
    [user, true],
    [frame("turn_ended", { reason: "end_turn" }), false],
    [frame("thinking_delta", { block: 0, delta: "hm" }), true],
    [frame("error", "provider down"), false],
    [frame("tool_call_composing", { call_id: "c", name: "bash" }), true],
    [frame("state_changed", { streaming: false }), false],
    [frame("token_delta", { block: 0, delta: "a" }), true],
    [frame("turn_ended", { reason: "end_turn" }), false],
    [frame("turn_started"), true],
  ];
  for (const [step, busy] of steps) {
    transcript.apply(step);
    assert.equal(transcript.busy.value, busy, step.kind);
  }
});

// The break this catches: a photo sent with no words dropped from the conversation, or its bytes
// kept in what's sent to the host to render
test("a photo sent on its own is still a message", () => {
  const photo = { role: "user", content: [{ type: "image", content: { data: "QUJD", mime: "image/jpeg" } }] };
  const transcript = fold([frame("message_committed", { message: photo })]);
  assert.deepEqual(shape(transcript), ["user"]);
  assert.deepEqual(asText(transcript.items.value[0]).content, [{ type: "image" }]);
});

// The break this catches: a hook's note, committed between turns, taken for a turn starting
test("a system message doesn't start a turn", () => {
  const note = { role: "system", content: [{ type: "text", content: "hook says hi" }] };
  assert.equal(fold([frame("message_committed", { message: note })]).busy.value, false);
});

/** The text of each of `transcript`'s lines. */
const texts = (transcript) => transcript.items.value.map((item) => asText(item).text.value);

// The break this catches: a minute of summary with nothing on the phone to say so, or a second
// line added beside the first when it's done
test("a compaction shows as it starts, and its line becomes the result", () => {
  const transcript = fold([frame("compaction_started", { tokens_before: 579_400 })]);
  assert.deepEqual(texts(transcript), ["Compacting the conversation — 579.4k of context…"]);
  transcript.apply(frame("compaction_done", { tokens_before: 579_400, tokens_after: 28_700 }));
  assert.deepEqual(texts(transcript), ["Compacted the conversation — 579.4k → 28.7k of context."]);
});

// The break this catches: "Compacting…" left standing after a cancel or failure, or the end
// written onto whatever line came last rather than the one that announced it
test("a stopped compaction settles the line that announced it", () => {
  const started = frame("compaction_started", { tokens_before: 1000 });
  const transcript = fold([started, frame("notice", "between"), frame("compaction_stopped")]);
  assert.deepEqual(texts(transcript), ["Compaction stopped.", "between"]);
});

// The break this catches: the phone writing a count differently from the TUI beside it.
test("token counts read as the TUI writes them", () => {
  assert.deepEqual([812, 1000, 579_400, 1_240_000].map(formatTokens), ["812", "1.0k", "579.4k", "1.2M"]);
});

// The break this catches: a long session drawing every item it ever had, which in WebKit slows
// each keystroke past 100ms, or the window dropping the newest items rather than the oldest
test("a long transcript draws its latest items and says how many wait above", () => {
  const items = Array.from({ length: WINDOW + 5 }, (_, index) => index);
  const { hidden, drawn } = windowOf(items, WINDOW);
  assert.equal(hidden, 5);
  assert.equal(drawn.length, WINDOW);
  assert.deepEqual([drawn[0], drawn.at(-1)], [5, WINDOW + 4]);
});

// The break this catches: a short session losing its first items to the window, or told there's
// history above that isn't there
test("a transcript that fits is drawn whole", () => {
  const items = [1, 2, 3];
  assert.deepEqual(windowOf(items, WINDOW), { hidden: 0, drawn: items });
  assert.deepEqual(windowOf(items, 3), { hidden: 0, drawn: items });
});

// The break this catches: a reader at the bottom left with an ever-growing page, or a first draw
// that doesn't start from the latest items
test("the window slides to the latest items while the reader follows the bottom", () => {
  assert.equal(windowStart(null, WINDOW + 5, WINDOW, false), 5);
  assert.equal(windowStart(5, WINDOW + 6, WINDOW, true), 6);
});

// The break this catches: each new item taking the oldest from the top while the reader is scrolled
// up, which in WebKit moves everything they were reading up the screen
test("the window holds its start while the reader reads above the bottom", () => {
  assert.equal(windowStart(5, WINDOW + 6, WINDOW, false), 5);
  assert.equal(windowStart(5, WINDOW + 40, WINDOW, false), 5);
});

// The break this catches: "Show earlier" doing nothing while the reader is scrolled up to tap it
test("asking for earlier items moves the start back while reading above the bottom", () => {
  assert.equal(windowStart(500, 900, 900 - 200, false), 200);
});

// The break this catches: a reader back at the bottom keeping every item that came while they read
test("back at the bottom, the window trims to the latest items again", () => {
  assert.equal(windowStart(5, WINDOW + 40, WINDOW, true), 40);
});

// The break this catches: a duration written differently from the TUI's card beside it.
test("durations read as the TUI writes them", () => {
  assert.deepEqual([0.4, 19, 1500, 63000].map(elapsed), ["<1ms", "19ms", "1.5s", "1m 3s"]);
});

// The break this catches: reasoning clipped to a few rows with nothing to say there's more, which
// reads as the agent's thought cut off mid-sentence where the TUI shows all of it
test("reasoning is drawn whole, never clipped", () => {
  const css = readFileSync(new URL("../app/style.css", import.meta.url), "utf8");
  const rules = [...css.matchAll(/([^{}]*\.thinking[^{}]*)\{([^}]*)\}/g)];
  assert.ok(rules.length > 0, "the reasoning rule is where this test looks");
  for (const [, selector, body] of rules) {
    assert.doesNotMatch(body, /max-height|overflow\s*:\s*hidden|line-clamp/, `${selector.trim()} clips`);
  }
});
