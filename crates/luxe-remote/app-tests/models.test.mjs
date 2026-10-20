// The model and thinking pickers: asking the session for its models, and the frames a pick sends.

import assert from "node:assert/strict";
import { test } from "node:test";
import { createModels, isCurrent, selectModelFrame, thinkingFrame, thinkingLevels } from "../app/models.js";

const SONNET = { provider: "anthropic", model: "claude-sonnet-5", name: "Claude Sonnet 5", thinking_levels: ["off", "low", "high"] };
const MINI = { provider: "openai", model: "gpt-mini", name: "GPT Mini", thinking_levels: ["off"] };

/** A models state that has asked once, and the frames it sent. */
function asked(sent = true) {
  const models = createModels();
  const frames = [];
  models.ask((frame) => (frames.push(frame), sent));
  return { models, frames };
}

// The break this catches: the question sent in a shape `list_models` doesn't take, or the sheet
// not showing that it's waiting
test("asking sends list_models with an id, and waits", () => {
  const { models, frames } = asked();
  assert.deepEqual(frames, [{ kind: "list_models", content: { request_id: "models-1" } }]);
  assert.equal(models.listing.value.state, "asking");
});

// The break this catches: a question that never left counted as asked, so the sheet waits forever
test("a question that couldn't be sent says so", () => {
  assert.equal(asked(false).models.listing.value.state, "lost");
});

// The break this catches: the answer not taken, or taken from the wrong field
test("the answer to the question is listed", () => {
  const { models } = asked();
  models.apply({ kind: "models_listed", content: { request_id: "models-1", models: [SONNET, MINI] } });
  assert.deepEqual(models.listing.value, { state: "listed", models: [SONNET, MINI] });
});

// The break this catches: an answer to an earlier question, arriving late, replacing the latest
test("an answer to an earlier question is ignored", () => {
  const { models } = asked();
  models.ask(() => true);
  models.apply({ kind: "models_listed", content: { request_id: "models-1", models: [MINI] } });
  assert.equal(models.listing.value.state, "asking");
});

// The break this catches: an older luxe's refusal ignored, so the sheet says it's listing forever
test("a luxe that doesn't know the question is told apart", () => {
  const { models } = asked();
  models.apply({ kind: "action_rejected", content: { kind: "list_models", reason: "unknown kind" } });
  assert.equal(models.listing.value.state, "unsupported");
});

// The break this catches: any refusal, of something else entirely, taken for the question's
test("a refusal of something else leaves the question waiting", () => {
  const { models } = asked();
  models.apply({ kind: "action_rejected", content: { kind: "steer_turn", reason: "missing text" } });
  assert.equal(models.listing.value.state, "asking");
});

// The break this catches: the running model matched by provider or by id alone, which marks a
// second provider's model of the same name as running
test("the running model is matched by provider and id together", () => {
  assert.equal(isCurrent(SONNET, { provider: "anthropic", model: "claude-sonnet-5" }), true);
  assert.equal(isCurrent(SONNET, { provider: "bedrock", model: "claude-sonnet-5" }), false);
  assert.equal(isCurrent(SONNET, { provider: "anthropic", model: "claude-opus-5" }), false);
});

// The break this catches: the levels offered from another model's list, or every level offered
// before the running model's are known, which the session would clamp without a word
test("thinking offers the running model's levels, and none before it's listed", () => {
  const current = { provider: "openai", model: "gpt-mini" };
  assert.deepEqual(thinkingLevels([SONNET, MINI], current), ["off"]);
  assert.deepEqual(thinkingLevels([], current), []);
});

// The break this catches: a pick sent with the display name or levels, which `select_model`
// doesn't take, or under another key
test("a picked model is sent as provider and id", () => {
  assert.deepEqual(selectModelFrame(SONNET), {
    kind: "select_model",
    content: { provider: "anthropic", model: "claude-sonnet-5" },
  });
});

// The break this catches: the level wrapped in an object, when `set_thinking_level` takes a string
test("a picked level is sent bare", () => {
  assert.deepEqual(thinkingFrame("high"), { kind: "set_thinking_level", content: "high" });
});
