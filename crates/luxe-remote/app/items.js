// @ts-check
// The transcript's items, drawn the way the TUI draws them: your messages on a band, replies as
// text, reasoning muted, and a line for each tool call that opens to show what it returned.
//
// Text arrives as the host renders it (`render.js`). Until it does, an item shows its plain text,
// so nothing waits on the round trip to be readable.

import { useLayoutEffect, useRef, useState } from "preact/hooks";
import { Answers, questionOutcome } from "./answers.js";
import { html, landedIn } from "./html.js";
import { useRendered } from "./render.js";

/** @import { Signal } from "@preact/signals" */
/** @import { Card, Item as TranscriptItem, TextItem } from "./transcript.js" */

/** How long a copy control says it copied. */
const COPIED_MS = 1500;

/**
 * How many items are drawn at first, and how many more each tap on "Show earlier" adds. WebKit
 * slows every keystroke as a page passes a few thousand items (a 9,000-item session took 100ms a
 * key), so a long session's history waits above until it's asked for.
 */
export const WINDOW = 300;

/**
 * The items to draw when the latest `shown` are wanted, and how many earlier ones wait above.
 * @template T @param {T[]} items @param {number} shown
 */
export function windowOf(items, shown) {
  const hidden = Math.max(0, items.length - shown);
  return { hidden, drawn: hidden ? items.slice(hidden) : items };
}

/**
 * Where the drawn items start, of `length`, when at least the latest `shown` are wanted. A reader
 * following the bottom gets the latest `shown`. One reading above it keeps the start where it was,
 * so new items grow the list below them rather than taking items from over them: WebKit doesn't
 * anchor the scroll, and every item dropped from the top moved what they were reading.
 * @param {number | null} start where the drawn items started last time, if they have
 * @param {number} length @param {number} shown @param {boolean} following
 */
export function windowStart(start, length, shown, following) {
  const latest = Math.max(0, length - shown);
  return following || start === null ? latest : Math.min(start, latest);
}

/** @param {{ items: TranscriptItem[], following: boolean }} props */
export function Transcript({ items, following }) {
  const [shown, setShown] = useState(WINDOW);
  const list = useRef(/** @type {HTMLDivElement | null} */ (null));
  const start = useRef(/** @type {number | null} */ (null));
  const keepPlace = useKeptPlace(list);
  start.current = windowStart(start.current, items.length, shown, following);
  const { hidden, drawn } = windowOf(items, items.length - start.current);
  const more = () => keepPlace(() => setShown(items.length - Math.max(0, hidden - WINDOW)));
  const count = html`<span class="dim">${hidden.toLocaleString()} more</span>`;
  return html`<div class="transcript" ref=${list} onClick=${copyCode}>
    ${hidden ? html`<button type="button" class="earlier" onClick=${more}>Show earlier ${count}</button>` : null}
    ${drawn.map((item) => html`<${Item} key=${item.key} item=${item} />`)}
  </div>`;
}

/**
 * Runs a change that adds items above, then scrolls the item that was first back to where it was.
 * @param {{ current: HTMLElement | null }} list @returns {(change: () => void) => void}
 */
function useKeptPlace(list) {
  const held = useRef(/** @type {{ item: Element, top: number } | null} */ (null));
  useLayoutEffect(() => {
    const kept = held.current;
    held.current = null;
    if (kept) holdInPlace(kept.item, kept.top);
  });
  return (change) => {
    const item = list.current?.querySelector(".item");
    if (item) held.current = { item, top: item.getBoundingClientRect().top };
    change();
  };
}

/** How many frames an item is held in place while the items drawn above it settle. */
const HOLD_FRAMES = 45;

/**
 * Keeps `item` at `top` while what was added above settles: items off screen are drawn at a guessed
 * height until they near it, and WebKit doesn't anchor the scroll as they do. A touch or a wheel
 * hands the scroll back.
 * @param {Element} item @param {number} top
 */
function holdInPlace(item, top) {
  let frames = HOLD_FRAMES;
  const release = () => (frames = 0);
  const kinds = ["touchstart", "wheel"];
  for (const kind of kinds) addEventListener(kind, release, { once: true, passive: true });
  const step = () => {
    if (frames-- <= 0) return kinds.forEach((kind) => removeEventListener(kind, release));
    scrollBy(0, item.getBoundingClientRect().top - top);
    requestAnimationFrame(step);
  };
  step();
}

/** @param {{ item: TranscriptItem }} props */
function Item({ item }) {
  switch (item.kind) {
    case "user": return html`<${UserMessage} item=${item} />`;
    case "assistant": return html`<${Reply} item=${item} />`;
    case "thinking": return html`<${Thinking} item=${item} />`;
    case "tool": return html`<${ToolCard} card=${item.card} />`;
    case "error": return html`<div class="item problem">${item.text}</div>`;
    default: return html`<div class="item notice">${item.text}</div>`;
  }
}

/**
 * Host HTML when it has come, the plain text until then.
 * @param {{ className: string, rendered: string | undefined, text: string }} props
 */
function Rendered({ className, rendered, text }) {
  if (rendered === undefined) return html`<div class=${`${className} plain`}>${text}</div>`;
  return html`<div class=${className} dangerouslySetInnerHTML=${{ __html: rendered }} />`;
}

/** @param {{ item: TextItem }} props */
function UserMessage({ item }) {
  const rendered = useRendered({ kind: "user", content: item.content ?? [] }, false);
  return html`<${Rendered} className="item user-message" rendered=${rendered} text=${item.text.value} />`;
}

/** @param {{ item: TextItem }} props */
function Reply({ item }) {
  const rendered = useRendered({ kind: "answer", text: item.text.value }, !!item.provisional);
  return html`<${Rendered} className="item reply md" rendered=${rendered} text=${item.text.value} />`;
}

/** Reasoning in full, as the TUI shows it by default. @param {{ item: TextItem }} props */
function Thinking({ item }) {
  const rendered = useRendered({ kind: "reasoning", text: item.text.value }, !!item.provisional);
  return html`<div class="item thinking">
    <${Rendered} className="md reasoning" rendered=${rendered} text=${item.text.value} />
  </div>`;
}

/** @param {{ card: Signal<Card> }} props */
function ToolCard({ card }) {
  const call = card.value;
  const output = call.output_preview;
  const head = html`<${ToolGlyph} name=${call.name} />
    <span class="tool-name">${call.name}</span>
    <span class="tool-label">${call.label}</span>
    <span class="tool-time">${standing(call)}</span>
    ${body(call)}`;
  const outcome = call.name === "question" && output ? questionOutcome(output) : null;
  if (outcome) return html`<div class=${`item tool ${call.state}`}><div class="tool-head">${head}</div><${Answers} outcome=${outcome} /></div>`;
  if (!output) return html`<div class=${`item tool ${call.state}`}><div class="tool-head">${head}</div></div>`;
  return html`<details class=${`item tool ${call.state}`}>
    <summary class="tool-head">${head}</summary>
    <pre class="tool-output">${output}</pre>
  </details>`;
}

/** The mark the TUI's card wears for this tool, which the host knows. @param {{ name: string }} props */
function ToolGlyph({ name }) {
  const glyph = useRendered({ kind: "tool", name }, false);
  if (!glyph) return null;
  return html`<span class="tool-glyph" aria-hidden="true" dangerouslySetInnerHTML=${{ __html: glyph }} />`;
}

/** @param {Card} call */
function body(call) {
  const card = call.card_body;
  if (!card?.text) return null;
  const lines = card.text.split("\n");
  const shown = lines.slice(0, Math.max(1, card.collapsed_lines || 3));
  const more = lines.length - shown.length;
  return html`<pre class="tool-body">${shown.join("\n")}${more > 0 ? html`<span class="dim">\n… ${more} more lines</span>` : null}</pre>`;
}

/**
 * A fence's copy control, for every rendered block at once; the page's policy allows no handler
 * in the host's HTML, so the transcript listens for it.
 * @param {Event} event
 */
function copyCode(event) {
  const button = landedIn(event, ".fence .copy");
  if (!button) return;
  event.stopPropagation();
  const code = button.closest(".fence")?.querySelector("code")?.textContent ?? "";
  navigator.clipboard.writeText(code).then(
    () => flash(button, "copied"),
    () => flash(button, "copy-failed"),
  );
}

/** @param {Element} element @param {string} className */
function flash(element, className) {
  element.classList.add(className);
  setTimeout(() => element.classList.remove(className), COPIED_MS);
}

/** How the call stands, in words: its time once done, otherwise its state. @param {Card} call */
export function standing(call) {
  if (call.state === "failed") return call.summary || "failed";
  if (call.state !== "succeeded") return call.state;
  return call.elapsed_ms == null ? "" : elapsed(call.elapsed_ms);
}

/** A duration the way the TUI writes it. @param {number} ms */
export function elapsed(ms) {
  if (ms < 1) return "<1ms";
  if (ms < 1000) return `${ms}ms`;
  if (ms < 60000) return `${(ms / 1000).toFixed(1)}s`;
  return `${Math.floor(ms / 60000)}m ${Math.round((ms % 60000) / 1000)}s`;
}
