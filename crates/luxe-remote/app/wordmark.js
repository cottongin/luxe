// @ts-check
// The logotype, drawn from the file `scripts/site-assets.sh` derives from the terminal's art and
// lit by the terminal's slow drift. Drawn inline rather than as an image, because a theme's colors
// can't reach inside an <img>.

import { signal } from "@preact/signals";
import { html } from "./html.js";

/** @typedef {{ viewBox: string, depth: string, paths: string[] }} Drawing */

/** @type {import("@preact/signals").Signal<Drawing | null>} */
const drawing = signal(null);

/**
 * What the app needs from the generated file; null when any of it is missing, so the header falls
 * back to the name. @param {string} svg @returns {Drawing | null}
 */
export function readDrawing(svg) {
  const viewBox = /viewBox="([^"]+)"/.exec(svg)?.[1];
  const depth = /data-drift-depth="([^"]+)"/.exec(svg)?.[1];
  const paths = [...svg.matchAll(/<path[^>]*\sd="([^"]+)"/g)].map((match) => match[1]);
  if (!viewBox || !depth || !paths.length) return null;
  return { viewBox, depth, paths };
}

/**
 * Where letter `index` of `count` starts in the drift, as a negative share of one swing. Half a
 * swing is spread across the mark, the terminal's `DRIFT_SPREAD`, so one wave runs from the first
 * letter to the last and never folds back. @param {number} index @param {number} count
 */
export function driftPhase(index, count) {
  return -1 + (index / Math.max(count - 1, 1)) * 0.5;
}

/** Reads the drawing, once per page. @param {typeof fetch} get */
export async function loadWordmark(get = fetch) {
  if (drawing.value) return;
  const response = await get("/app/generated/wordmark.svg");
  if (!response.ok) throw new Error(`the wordmark answered ${response.status}`);
  drawing.value = readDrawing(await response.text());
}

export function Wordmark() {
  const art = drawing.value;
  if (!art) return html`<span class="title">Luxe</span>`;
  // a custom property set through the DOM, which the page's style-src allows
  return html`<svg class="wordmark" viewBox=${art.viewBox} role="img" aria-label="Luxe"
    style=${{ "--drift-depth": art.depth }}>
    ${art.paths.map((d, index) => html`<path d=${d} style=${{ "--phase": driftPhase(index, art.paths.length) }} />`)}
  </svg>`;
}
