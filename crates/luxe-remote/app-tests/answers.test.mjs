// A question card's answers, read back out of the call's result. Each result text is a literal
// copied from a saved session or from `luxe_core::question`'s own tests, so a reworded ending on
// the host fails here rather than leaving the phone's card quietly blank.

import assert from "node:assert/strict";
import { test } from "node:test";
import { questionOutcome } from "../app/answers.js";

// The break this catches: a single answer, the usual case, left off the card
test("one answer reads back as that answer", () => {
  assert.deepEqual(questionOutcome("The user answered: Now, with my phone"), { answers: ["Now, with my phone"], note: "" });
});

// The break this catches: several answers run together as one, or a comma inside an answer
// splitting it in two
test("several answers read back one per line, commas and all", () => {
  const result = "The user answered:\n- Dark mode\n- also, vim keys";
  assert.deepEqual(questionOutcome(result), { answers: ["Dark mode", "also, vim keys"], note: "" });
});

// The break this catches: a dismissal drawn as though the reader chose something
test("a dismissal says so and credits no answer", () => {
  const outcome = questionOutcome("The user dismissed the question without answering.");
  assert.deepEqual(outcome, { answers: [], note: "dismissed without answering" });
});

// The break this catches: an empty submission mistaken for a dismissal, or for text the card can't read
test("an empty submission says it had no answer", () => {
  const outcome = questionOutcome("The user submitted the question with no selection.");
  assert.deepEqual(outcome, { answers: [], note: "submitted with no answer" });
});

// The break this catches: a default the call fell back to credited to a reader who was never asked
test("an answer nobody gave says it came from the default", () => {
  const outcome = questionOutcome("[Auto-answered, no interactive session] Which runtime?\nAnswer: tokio");
  assert.deepEqual(outcome, { answers: ["tokio"], note: "nobody to ask · answered from the default" });
});

// The break this catches: the card inventing an answer from text that isn't one of the host's endings
test("text that isn't a question's ending reads as nothing", () => {
  assert.equal(questionOutcome("Error: question needs a prompt"), null);
  assert.equal(questionOutcome("[Auto-answered, no interactive session] Which runtime?"), null);
});
