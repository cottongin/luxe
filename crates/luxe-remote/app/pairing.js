// @ts-check
// Pairing from inside the app. A scanned code opens Safari, and an app kept on the home screen has
// cookies of its own, copied from Safari only when it was added, so one that lost its pairing
// can't get it back by scanning. Pasting the invitation link here pairs the app itself.

import { useState } from "preact/hooks";
import { fieldOf, html } from "./html.js";

/**
 * The invitation in `text`: the link `luxe remote pair` prints, or the part after its `#`.
 * @param {string} text @returns {string | null}
 */
export function invitationFrom(text) {
  const token = text.trim().split("#").pop() ?? "";
  return /^[A-Za-z0-9_-]+$/.test(token) ? token : null;
}

/** What an unpaired device is told, and a field for the link. */
export function PairWithLink() {
  const [text, setText] = useState("");
  const token = invitationFrom(text);
  /** @param {SubmitEvent} event */
  const submit = (event) => {
    event.preventDefault();
    if (token) location.assign(`/pair#${token}`);
  };
  return html`<section class="pairing">
    <p>Run <code>luxe remote pair</code> on your computer, then scan its QR code or paste its link
      here.</p>
    <form class="pair-link" onSubmit=${submit}>
      <input type="text" inputmode="url" placeholder="https://…/pair#…" value=${text} autocapitalize="off"
        autocorrect="off" spellcheck="false" aria-label="Invitation link"
        onInput=${(/** @type {Event} */ event) => setText(fieldOf(event).value)} />
      <button type="submit" disabled=${!token}>Pair</button>
    </form>
  </section>`;
}
