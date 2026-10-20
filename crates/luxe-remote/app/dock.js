// @ts-check
// The dock: the plan and the changed files, as the host renders them. A rail beside the transcript
// on a wide screen, shown and hidden by the header's button and `/dock` (layout.js); the project
// panel's Dock tab on a phone. A long plan scrolls in its own box, and a changed file opens its
// diff.

import { useLayoutEffect, useRef } from "preact/hooks";
import { html, landedIn } from "./html.js";
import { Icon } from "./icons.js";
import { useRendered } from "./render.js";
import { PickablePiece } from "./lines-view.js";
import { hasDock, planScroll } from "./workspace.js";
import { useWide } from "./layout.js";

/** @import { Changes, OpenDiff, Todos } from "./workspace.js" */

/** @param {{ todos: Todos | null, changes: Changes | null, shown: boolean, open: (path: string) => void }} props */
export function Dock({ todos, changes, shown, open }) {
  return html`<aside class=${shown ? "dock shown" : "dock"} aria-label="Context Dock"
      onClick=${(/** @type {Event} */ event) => pickChange(event, open)}>
    <${DockSections} todos=${todos} changes=${changes} />
  </aside>`;
}

/**
 * The dock as the project panel's Dock tab, on a phone, where the panel is the one way in.
 * @param {{ hooks: { todos: Todos | null, changes: Changes | null, openDiff: (path: string) => void } }} props
 */
export function DockTab({ hooks: { todos, changes, openDiff } }) {
  return html`<div class="dock-tab" onClick=${(/** @type {Event} */ event) => pickChange(event, openDiff)}>
    <${DockSections} todos=${todos} changes=${changes} />
  </div>`;
}

/** @param {{ todos: Todos | null, changes: Changes | null }} props */
function DockSections({ todos, changes }) {
  return html`
    ${hasDock(todos, changes) ? null : html`<p class="dim dock-empty">No plan or changes yet.</p>`}
    ${todos?.total ? html`<${Section} piece=${{ kind: "plan", todos }} />` : null}
    ${changes?.total_files ? html`<${Section} piece=${{ kind: "changes", changes }} />` : null}`;
}

/**
 * The rows are host HTML, so a tap is read off the button it landed in.
 * @param {Event} event @param {(path: string) => void} open
 */
function pickChange(event, open) {
  const path = landedIn(event, "button.change")?.dataset.path;
  if (path) open(path);
}

/** @param {{ piece: import("./render.js").Piece }} props */
function Section({ piece }) {
  const rendered = useRendered(piece, true);
  const section = useRef(/** @type {HTMLElement | null} */ (null));
  const handsOn = usePlanFollow(section, rendered);
  return html`<section class="dock-section" ref=${section} onTouchStart=${handsOn} onWheel=${handsOn}
    dangerouslySetInnerHTML=${{ __html: rendered ?? "" }}></section>`;
}

/**
 * Scrolls a long plan to the work in hand each time it's drawn, as the TUI's rail does, until the
 * reader scrolls it themselves. Returns what marks that.
 * @param {{ current: HTMLElement | null }} section @param {string | null | undefined} rendered
 */
function usePlanFollow(section, rendered) {
  const moved = useRef(false);
  useLayoutEffect(() => {
    const list = section.current?.querySelector(".todos");
    if (!(list instanceof HTMLElement) || moved.current) return;
    const rows = [...list.children].map((row) => ({
      bottom: /** @type {HTMLElement} */ (row).offsetTop + /** @type {HTMLElement} */ (row).offsetHeight,
      status: row.className.match(/todo-(\S+)/)?.[1] ?? "",
    }));
    list.scrollTop = planScroll(rows, list.clientHeight);
  }, [rendered]);
  return () => (moved.current = true);
}

/** The header's button that shows and hides the dock. @param {{ shown: boolean, toggle: () => void }} props */
export function DockToggle({ shown, toggle }) {
  return html`<button type="button" class=${shown ? "dock-toggle on" : "dock-toggle"}
    aria-label=${shown ? "Hide the Context Dock" : "Show the Context Dock"} aria-expanded=${shown}
    onClick=${toggle}><${Icon} name="dock" /></button>`;
}

/**
 * One file's diff until it's closed: over the page on a phone, beside the dock on a wide screen.
 * @param {{ open: OpenDiff | null, close: () => void, retry: () => void, ask: (words: string) => void }} props
 */
export function DiffSheet({ open, close, retry, ask }) {
  const wide = useWide();
  useEscape(open ? close : null);
  if (!open) return null;
  return html`<div class="sheet peek" role="dialog" aria-modal=${!wide} aria-label=${`Changes to ${open.path}`}>
    <header class="bar">
      <span class="title">${open.path}</span>
      <button type="button" class="close" aria-label="Close the diff" onClick=${close}><${Icon} name="close" /></button>
    </header>
    <div class="sheet-body"><${DiffBody} open=${open} retry=${retry} ask=${ask} /></div>
  </div>`;
}

/** @param {{ open: OpenDiff, retry: () => void, ask: (words: string) => void }} props */
function DiffBody({ open, retry, ask }) {
  if (open.lost) {
    return html`<p class="diff-note">Connection lost before the diff loaded.
      <button type="button" class="retry" onClick=${retry}>Try again</button></p>`;
  }
  if (open.waiting) return html`<p class="diff-note">Loading diff…</p>`;
  /** @type {import("./render.js").Piece} */
  const piece = open.diff ? { kind: "diff", diff: open.diff } : { kind: "diff_failed", error: open.error };
  return html`<${PickablePiece} piece=${piece} ask=${ask} />`;
}

/**
 * Escape closes the sheet, for an iPad with a keyboard. Armed as the sheet reaches the page rather
 * than after the next paint, so an Escape pressed straight after a pane opens or closes isn't lost.
 * @param {(() => void) | null} onEscape
 */
export function useEscape(onEscape) {
  useLayoutEffect(() => {
    if (!onEscape) return;
    /** @param {KeyboardEvent} event */
    const listener = (event) => event.key === "Escape" && onEscape();
    addEventListener("keydown", listener);
    return () => removeEventListener("keydown", listener);
  }, [onEscape]);
}
