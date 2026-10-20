// @ts-check
// A question card's answers, read back out of the call's result the way the TUI's card reads them
// (`luxe_core::question::QuestionOutcome`). The prompt above the composer is gone once answered, so
// without these rows the card is the question alone and the decision leaves no trace.

import { html } from "./html.js";

// The four endings as the host writes them, copied from `QuestionOutcome::result_text`
const ANSWERED = "The user answered:";
const EMPTY = "The user submitted the question with no selection.";
const DISMISSED = "The user dismissed the question without answering.";
const UNATTENDED = "[Auto-answered, no interactive session]";

/**
 * @typedef {{ answers: string[], note: string }} Outcome
 */

/**
 * What a question's result says was decided, or null when the text isn't one of the host's: a card
 * that shows its raw output beats one inventing an answer nobody gave.
 * @param {string} result @returns {Outcome | null}
 */
export function questionOutcome(result) {
  const [first = "", ...rest] = result.split("\n");
  const head = first.trimEnd();
  if (head.startsWith(ANSWERED)) return answered(head.slice(ANSWERED.length).trim(), rest);
  if (head.startsWith(UNATTENDED)) return unattended(rest);
  if (head === EMPTY) return { answers: [], note: "submitted with no answer" };
  if (head === DISMISSED) return { answers: [], note: "dismissed without answering" };
  return null;
}

/** @param {string} inline @param {string[]} lines @returns {Outcome} */
function answered(inline, lines) {
  if (inline) return { answers: [inline], note: "" };
  const listed = lines.map((line) => line.trim()).filter((line) => line.startsWith("- ")).map((line) => line.slice(2));
  const answers = listed.filter(Boolean);
  return { answers, note: answers.length ? "" : "submitted with no answer" };
}

/** Nobody was there to ask, so the card mustn't credit the reader. @param {string[]} lines */
function unattended(lines) {
  const answer = lines.map((line) => line.trim()).find((line) => line.startsWith("Answer:"));
  if (answer === undefined) return null;
  return { answers: [answer.slice("Answer:".length).trim()], note: "nobody to ask · answered from the default" };
}

/** The decision under its question, labeled so it doesn't read as the options offered. @param {{ outcome: Outcome }} props */
export function Answers({ outcome }) {
  return html`<div class="answers">
    ${outcome.note ? html`<p class="answer-note">${outcome.note}</p>` : null}
    ${outcome.answers.map((answer) => html`<p class="answer"><span class="answer-label">answer</span>${answer}</p>`)}
  </div>`;
}
