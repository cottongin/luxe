// @ts-check
// The prompt waiting on you, above the composer: a permission to grant or a question to answer.
//
// Like the TUI's card, it holds back every answer but the refusal for a moment after it appears,
// since it can slide in under a thumb aimed at Send; a tap that lands then can't grant anything.

import { useEffect, useState } from "preact/hooks";
import { fieldOf, html } from "./html.js";
import { permissionReply, questionReply } from "./pending.js";

/** @import { Frame } from "./connection.js" */
/** @import { PermissionPrompt, Prompt, QuestionOption, QuestionPrompt } from "./pending.js" */
/** @typedef {(frame: Frame) => boolean} Send */

/** How long a new prompt ignores everything but a refusal, as the TUI's `GRACE`. */
export const GRACE_MS = 400;

/** @param {{ prompts: Prompt[], send: Send }} props */
export function PromptCard({ prompts, send }) {
  const prompt = prompts[0];
  const settled = useSettled(prompt?.id);
  if (!prompt) return null;
  const count = prompts.length > 1 ? html`<span class="dim"> 1 of ${prompts.length}</span>` : null;
  const card = prompt.kind === "permission"
    ? html`<${Permission} prompt=${prompt} send=${send} settled=${settled} />`
    : html`<${Question} key=${prompt.id} prompt=${prompt} send=${send} settled=${settled} />`;
  return html`<section class="prompt" role="dialog" aria-label=${prompt.kind}>${count}${card}</section>`;
}

/**
 * False for [`GRACE_MS`] after the prompt `id` appears. Keyed by id rather than a flag reset in an
 * effect, which would leave the next prompt one render with the last one's grace already spent.
 * @param {string | undefined} id
 */
function useSettled(id) {
  const [settledId, setSettledId] = useState(/** @type {string | undefined} */ (undefined));
  useEffect(() => {
    const timer = setTimeout(() => setSettledId(id), GRACE_MS);
    return () => clearTimeout(timer);
  }, [id]);
  return id != null && settledId === id;
}

/**
 * The TUI's answers, in its order; the two that remember only when the call has a pattern to.
 * @param {PermissionPrompt} prompt
 */
export function permissionChoices(prompt) {
  const patterns = prompt.always_patterns ?? [];
  const remembered = patterns.join(", ");
  return [
    { reply: "once", label: "Allow once", hint: "run just this call" },
    ...(patterns.length
      ? [
          { reply: "always", label: "Allow for this session", hint: `remember ${remembered}` },
          { reply: "always_persist", label: "Allow always", hint: `save ${remembered} to your permissions` },
        ]
      : []),
    { reply: "reject", label: "Reject", hint: "deny this call" },
  ];
}

/** @param {{ prompt: PermissionPrompt, send: Send, settled: boolean }} props */
function Permission({ prompt, send, settled }) {
  return html`
    <p class="prompt-title">${prompt.tool} wants permission</p>
    <p class="prompt-summary">${prompt.summary}</p>
    <div class="choices">
      ${permissionChoices(prompt).map((choice) => html`
        <button type="button" class=${`choice ${choice.reply}`} disabled=${!settled && choice.reply !== "reject"}
          onClick=${() => send(permissionReply(prompt.id, choice.reply))}>
          <span class="choice-label">${choice.label}</span>
          <span class="choice-hint">${choice.hint}</span>
        </button>`)}
    </div>`;
}

/** @param {{ prompt: QuestionPrompt, send: Send, settled: boolean }} props */
function Question({ prompt, send, settled }) {
  const [chosen, setChosen] = useState(/** @type {string[]} */ ([]));
  /** @param {string[] | null} answered */
  const answer = (answered) => send(questionReply(prompt.id, answered));
  /** @param {string} label */
  const pick = (label) => (prompt.multi ? setChosen(toggled(chosen, label)) : answer([label]));
  return html`
    <p class="prompt-title">${prompt.prompt}</p>
    <div class="choices">
      ${prompt.options.map((option) => html`<${QuestionChoice} option=${option} settled=${settled}
        picked=${chosen.includes(option.label)} multi=${prompt.multi} onPick=${() => pick(option.label)} />`)}
    </div>
    <${OwnAnswer} chosen=${chosen} settled=${settled} answer=${answer} />
    <button type="button" class="dismiss" onClick=${() => answer(null)}>Dismiss</button>`;
}

/**
 * Words of your own, sent with whatever options are ticked.
 * @param {{ chosen: string[], settled: boolean, answer: (answered: string[]) => void }} props
 */
function OwnAnswer({ chosen, settled, answer }) {
  const [own, setOwn] = useState("");
  const typed = own.trim();
  const ready = settled && (chosen.length > 0 || typed !== "");
  /** @param {SubmitEvent} event */
  const submit = (event) => {
    event.preventDefault();
    if (ready) answer([...chosen, ...(typed ? [typed] : [])]);
  };
  return html`<form class="own-answer" onSubmit=${submit}>
    <input type="text" placeholder="Type your own answer" value=${own} onInput=${(/** @type {Event} */ event) => setOwn(fieldOf(event).value)} />
    <button type="submit" disabled=${!ready}>Send</button>
  </form>`;
}

/**
 * @param {{ option: QuestionOption, settled: boolean, picked: boolean, multi: boolean,
 *   onPick: () => void }} props
 */
function QuestionChoice({ option, settled, picked, multi, onPick }) {
  const mark = multi ? html`<span class="check" aria-hidden="true">${picked ? "[x] " : "[ ] "}</span>` : null;
  return html`<button type="button" class=${`choice ${picked ? "picked" : ""}`} disabled=${!settled}
    aria-pressed=${multi ? picked : undefined} onClick=${onPick}>
    <span class="choice-label">${mark}${option.label}${option.recommended ? html`<span class="recommended">  recommended</span>` : null}</span>
    ${option.description ? html`<span class="choice-hint">${option.description}</span>` : null}
  </button>`;
}

/** `list` with `label` added, or taken out if it was there. @param {string[]} list @param {string} label */
export function toggled(list, label) {
  return list.includes(label) ? list.filter((item) => item !== label) : [...list, label];
}
