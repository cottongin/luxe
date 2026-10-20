// @ts-check
// The status line's details: the mode, the thinking level and the model, each a tap to change,
// and the readings the compact line leaves out. Changes go to the session as they're picked; the
// sheet shows what the session reports back, so a refused change never looks made.

import { html } from "./html.js";
import { Icon } from "./icons.js";
import { useEscape } from "./dock.js";
import { isCurrent, selectModelFrame, thinkingFrame, thinkingLevels } from "./models.js";
import { useRendered } from "./render.js";

/** @import { Frame } from "./connection.js" */
/** @import { Listing, ModelChoice } from "./models.js" */

/** The modes `set_mode` takes. */
export const MODES = ["build", "plan"];

/**
 * @param {{ facts: Record<string, any>, mode: string, listing: Listing,
 *   send: (frame: Frame) => boolean, close: () => void }} props
 */
export function DetailsSheet({ facts, mode, listing, send, close }) {
  useEscape(close);
  const levels = thinkingLevels(listing.models, facts.model);
  return html`<div class="sheet" role="dialog" aria-modal="true" aria-label="Session details">
    <header class="bar">
      <span class="title">Details</span>
      <button type="button" class="close" aria-label="Close the details" onClick=${close}><${Icon} name="close" /></button>
    </header>
    <div class="sheet-body details">
      <section><h2 class="section-head"><span class="badge">Model</span></h2>
        <${Choices} title="Mode" options=${MODES} current=${mode} pick=${(/** @type {string} */ next) => send({ kind: "set_mode", content: next })} />
        <${Choices} title="Thinking" options=${levels} current=${facts.thinking} pick=${(/** @type {string} */ level) => send(thinkingFrame(level))} />
        <${ModelPicker} listing=${listing} current=${facts.model} pick=${(/** @type {ModelChoice} */ choice) => send(selectModelFrame(choice))} />
      </section>
      <${Readings} facts=${facts} />
    </div>
  </div>`;
}

/**
 * A row of choices with the current one marked; nothing when there's nothing to choose.
 * @param {{ title: string, options: string[], current: string, pick: (option: string) => void }} props
 */
function Choices({ title, options, current, pick }) {
  if (!options.length) return null;
  return html`<h3>${title}</h3><div class="options" role="group" aria-label=${title}>
    ${options.map((option) => html`<button type="button" key=${option} aria-pressed=${option === current}
      onClick=${() => pick(option)}>${option}</button>`)}
  </div>`;
}

/**
 * @param {{ listing: Listing, current: import("./models.js").CurrentModel | undefined,
 *   pick: (choice: ModelChoice) => void }} props
 */
function ModelPicker({ listing, current, pick }) {
  return html`<h3>Model</h3>
    <${ListingNote} state=${listing.state} empty=${!listing.models.length} />
    <ul class="models">${listing.models.map((choice) => html`<li key=${`${choice.provider}/${choice.model}`}>
      <button type="button" aria-pressed=${isCurrent(choice, current)} onClick=${() => pick(choice)}>
        <span class="name">${choice.name}</span> <span class="dim">${choice.provider}</span>
      </button></li>`)}
    </ul>`;
}

/** What the model list is waiting on, when it's not just a list. @param {{ state: string, empty: boolean }} props */
function ListingNote({ state, empty }) {
  if (state === "unsupported") return html`<p class="dim">Update Luxe on your computer to switch models from here.</p>`;
  if (state === "lost") return html`<p class="dim">Not connected.</p>`;
  if (state === "asking" && empty) return html`<p class="dim">Loading models…</p>`;
  if (state === "listed" && empty) return html`<p class="dim">No other models configured.</p>`;
  return null;
}

/**
 * Whether the rendered line holds a reading: a session that has spent nothing yet reports a total
 * the host words as no segment at all. @param {string | null | undefined} rendered
 */
export function hasReadings(rendered) {
  return /\bseg-(context|cost)\b/.test(rendered ?? "");
}

/**
 * The context and cost, worded as the line words them: the line's own render, the same request,
 * with the model and thinking left to the pickers above. @param {{ facts: Record<string, any> }} props
 */
function Readings({ facts }) {
  const rendered = useRendered({ kind: "status", ...facts }, true);
  if (!hasReadings(rendered)) return null;
  return html`<section><h2 class="section-head"><span class="badge recessed">Session</span></h2>
    <p class="readings" dangerouslySetInnerHTML=${{ __html: rendered }}></p></section>`;
}
