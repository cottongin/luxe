// The status line's facts, folded from frames, and the mode switch.

import assert from "node:assert/strict";
import { test } from "node:test";
import { createStatus, nextModeFrame } from "../app/status.js";

const frame = (kind, content) => ({ kind, content });
const model = { provider: "github-copilot", model: "claude-haiku-4.5", display_name: "Claude Haiku 4.5 (latest)" };

// The break this catches: each frame replacing the facts rather than adding to them, so a
// thinking change wipes the context reading until the next call reports it again
test("each frame updates its own facts and keeps the rest", () => {
  const status = createStatus();
  status.apply(frame("context_usage", { used_tokens: 9613, context_window: 200000 }));
  status.apply(frame("state_changed", { model, streaming: false, mode: "plan", thinking: "high" }));
  assert.deepEqual(status.facts.value, {
    context: { used_tokens: 9613, context_window: 200000 },
    model,
    thinking: "high",
  });
  assert.equal(status.mode.value, "plan");
});

// The break this catches: the plan's allowance or the session's spend never reaching the host,
// so the cost slot stays empty; the spend goes as its total, which is all the line shows
test("the plan and the spend are kept for the cost slot", () => {
  const status = createStatus();
  const plan = { plan: "individual", primary: { used_percent: 22.3, window_minutes: 43200 } };
  const total = { calls: 14, cost_usd: 0, unpriced_calls: 0, plan_usd: 0.045, plan_calls: 14 };
  status.apply(frame("plan_usage", plan));
  status.apply(frame("session_usage", { total, by_source: [{ source: "agent", totals: total }] }));
  assert.deepEqual(status.facts.value, { plan, session: { total } });
});

// The break this catches: the mode switch sending the mode already on, so a tap does nothing
test("a tap on the mode switches to the other one", () => {
  assert.deepEqual(nextModeFrame("build"), { kind: "set_mode", content: "plan" });
  assert.deepEqual(nextModeFrame("plan"), { kind: "set_mode", content: "build" });
});
