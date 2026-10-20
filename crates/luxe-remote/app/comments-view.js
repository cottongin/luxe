// @ts-check
// Commenting on a review chapter from the phone: the composer that writes a comment or a reply,
// the buttons on the host's comment cards, and the guide's line offering to send what's unsent.

import { useState } from "preact/hooks";
import { html, landedIn } from "./html.js";
import { Icon } from "./icons.js";
import { actEdit, anchorFor, anchorWords, commentEdit, labelOf, unsent } from "./comments.js";

/** @import { PanelHooks } from "./panel.js" */
/** @import { LinePick } from "./lines.js" */
/** @import { ReviewShown } from "./review.js" */
/**
 * What the composer is writing: a new comment at `anchor`, or a reply on comment `reply`.
 * @typedef {{ heading: string, anchor?: any, reply?: string }} Composing
 */

/** A new comment on `pick`, or on the chapter with none. @param {any} review @param {string} chapter @param {LinePick | null} pick @returns {Composing} */
export function composeAt(review, chapter, pick) {
  const anchor = anchorFor(review, chapter, pick);
  return { heading: anchorWords(anchor, pick), anchor };
}

/**
 * A tap on a comment card's button: a reply opens the composer, anything else is sent as it is.
 * @param {MouseEvent} event @param {PanelHooks} hooks @param {(composing: Composing) => void} compose
 */
export function tapCard(event, { review, send }, compose) {
  const button = landedIn(event, "[data-act]");
  const finding = button?.closest("[data-finding]")?.getAttribute("data-finding");
  const act = button?.getAttribute("data-act");
  if (!finding || !act) return;
  if (act === "reply") return compose({ heading: `Reply to ${labelOf(review.shown.value.session, finding)}`, reply: finding });
  const edit = actEdit(act, finding);
  if (edit) review.edit(send, edit);
}

/** The edit the composer's words make. @param {Composing} composing @param {string} body @param {boolean} question @param {boolean} now */
export function composedEdit(composing, body, question, now) {
  if (composing.reply) return actEdit("reply", composing.reply, body);
  return commentEdit(composing.anchor, body, question, now);
}

/**
 * The composer, at the foot of the chapter. A comment can be saved to send later or sent now; a
 * reply goes at once, as the TUI's does.
 * @param {{ composing: Composing, submit: (body: string, question: boolean, now: boolean) => void, cancel: () => void }} props
 */
export function CommentSheet({ composing, submit, cancel }) {
  const [body, setBody] = useState("");
  const [question, setQuestion] = useState(false);
  const words = body.trim();
  const kind = (/** @type {boolean} */ asks, /** @type {string} */ label) =>
    html`<button type="button" class="mention" aria-pressed=${question === asks} onClick=${() => setQuestion(asks)}>${label}</button>`;
  return html`<div class="rv-compose" role="group" aria-label=${composing.heading}>
    <header class="bar"><span class="title">${composing.heading}</span>
      <button type="button" class="close" aria-label="Cancel" onClick=${cancel}><${Icon} name="close" /></button></header>
    ${composing.reply ? null : html`<p class="rv-kinds">${kind(false, "Comment")}${kind(true, "Question")}</p>`}
    <textarea rows="4" aria-label=${composing.heading} value=${body} onInput=${(/** @type {any} */ event) => setBody(event.target.value)}></textarea>
    <p class="rv-compose-acts">
      ${composing.reply ? null : html`<button type="button" class="mention" disabled=${!words} onClick=${() => submit(words, question, false)}>Save</button>`}
      <button type="button" class="mention" disabled=${!words} onClick=${() => submit(words, question, true)}>Send</button>
    </p>
  </div>`;
}

/** The guide's line for comments not sent yet, and the button that sends them all. @param {{ shown: ReviewShown, hooks: PanelHooks }} props */
export function Unsent({ shown, hooks }) {
  const ids = unsent(shown.session);
  if (!ids.length) return null;
  const words = ids.length === 1 ? "1 unsent comment" : `${ids.length} unsent comments`;
  return html`<p class="rv-unsent">${words}
    <button type="button" class="mention" onClick=${() => hooks.review.edit(hooks.send, { edit: "send", finding_ids: ids })}>Send</button></p>`;
}
