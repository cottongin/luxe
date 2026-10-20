// @ts-check
// The few drawn icons the app uses, as inline SVG in the text's color. Drawn rather than taken from
// a font, so there's no glyph a phone can lack; each button carrying one says its words to a
// screen reader.

import { html } from "./html.js";

/** @typedef {"send" | "stop" | "attach" | "close" | "dock" | "menu"} IconName */

const STROKE = { fill: "none", stroke: "currentColor", "stroke-width": "2.2", "stroke-linecap": "round", "stroke-linejoin": "round" };

/** @type {Record<IconName, () => unknown>} */
const DRAWINGS = {
  send: () => html`<path d="M12 19V5M5.5 11.5 12 5l6.5 6.5" ...${STROKE} />`,
  stop: () => html`<rect x="7" y="7" width="10" height="10" rx="1.5" fill="currentColor" />`,
  // a paperclip, tilted as one lies on a desk: attaching reads quicker than a picture of a photo
  attach: () => html`<path d="M17 8v8a5 5 0 0 1-10 0V6a3.25 3.25 0 0 1 6.5 0v9.5a1.5 1.5 0 0 1-3 0V8"
    transform="rotate(40 12 12)" ...${STROKE} />`,
  close: () => html`<path d="M6.5 6.5l11 11M17.5 6.5l-11 11" ...${STROKE} />`,
  dock: () => html`<rect x="3.5" y="4.5" width="17" height="15" rx="2" ...${STROKE} /><path d="M9.5 4.5v15" ...${STROKE} />`,
  menu: () => html`<path d="M4.5 7h15M4.5 12h15M4.5 17h15" ...${STROKE} />`,
};

/** One icon, sized to the line it sits in. @param {{ name: IconName }} props */
export function Icon({ name }) {
  return html`<svg class="icon" viewBox="0 0 24 24" aria-hidden="true">${DRAWINGS[name]()}</svg>`;
}
