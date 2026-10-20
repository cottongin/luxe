// @ts-check
// Tagged templates instead of JSX, so the browser runs the files as they are written.
import { h } from "preact";
import htm from "htm";

export const html = htm.bind(h);

/**
 * The field an event came from. Templates don't type their handlers' events, so the one cast the
 * app's inputs need lives here.
 * @param {Event} event @returns {HTMLInputElement}
 */
export function fieldOf(event) {
  return /** @type {HTMLInputElement} */ (event.target);
}

/**
 * The element matching `selector` that `event` landed in, for a tap on HTML the host rendered,
 * which carries no handlers of its own.
 * @param {Event} event @param {string} selector @returns {HTMLElement | null}
 */
export function landedIn(event, selector) {
  const target = /** @type {Element | null} */ (event.target);
  return target?.closest?.(selector) ?? null;
}
