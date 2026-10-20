// @ts-check
// A file or a diff whose lines can be picked, with the bar that asks about them. The page is the
// host's HTML, so taps are read by delegation and picked rows are marked on the DOM once it's there.

import { useLayoutEffect, useRef, useState } from "preact/hooks";
import { html } from "./html.js";
import { askLabel, lineWords, markPicked, selectedLines, tapped } from "./lines.js";
import { useRendered } from "./render.js";

/** @import { LinePick } from "./lines.js" */
/** @import { Piece } from "./render.js" */

/**
 * How long a selection's lines outlive the selection. Tapping Ask can collapse it in Safari before
 * the tap lands, and the lines must still be there to send.
 */
const SELECTION_GRACE_MS = 400;

/**
 * What a pickable pane does with lines: ask about them, and, in a review chapter, comment on them
 * or, with none picked, on the chapter.
 * @typedef {{ ask: (words: string) => void, comment?: (pick: LinePick | null) => void }} LineActs
 */

/** A piece the host draws, its lines pickable. @param {{ piece: Piece } & LineActs} props */
export function PickablePiece({ piece, ask, comment }) {
  const rendered = useRendered(piece, false);
  return html`<${Pickable} markup=${rendered ?? ""} ask=${ask} comment=${comment} />`;
}

/** @param {{ markup: string } & LineActs} props */
export function Pickable({ markup, ask, comment }) {
  const root = useRef(/** @type {HTMLDivElement | null} */ (null));
  const [pick, setPick] = useState(/** @type {LinePick | null} */ (null));
  const selected = useSelectedLines(root);
  useLayoutEffect(() => void (root.current && markPicked(root.current, pick)), [pick, markup]);
  const target = selected.lines ?? pick;
  /** Lets the pick and the selection go, then hands the lines on. @param {(lines: LinePick | null) => void} then */
  const done = (then) => () => {
    document.getSelection()?.removeAllRanges();
    selected.drop();
    setPick(null);
    then(target);
  };
  return html`<div ref=${root} onClick=${(/** @type {MouseEvent} */ event) => setPick((it) => tapped(it, /** @type {Element} */ (event.target)))}
      dangerouslySetInnerHTML=${{ __html: markup }}></div>
    <${LineBar} target=${target} ask=${done((lines) => lines && ask(lineWords(lines)))} comment=${comment && done(comment)} />`;
}

/**
 * The bar at the foot of the pane: Ask for picked lines, and Comment where comments go, which with
 * nothing picked is the chapter's. Nothing to offer, no bar.
 * @param {{ target: LinePick | null, ask: () => void, comment?: () => void }} props
 */
function LineBar({ target, ask, comment }) {
  if (!target && !comment) return null;
  const button = (/** @type {string} */ words, /** @type {() => void} */ act) =>
    html`<button type="button" class="mention" onClick=${act}>${words}</button>`;
  return html`<div class="line-ask">
    ${comment ? button(target ? "Comment" : "Comment on the chapter", comment) : null}
    ${target ? button(askLabel(target), ask) : null}
  </div>`;
}

/**
 * The lines the phone's own selection covers under `root`, kept a moment after it collapses.
 * @param {{ current: Element | null }} root
 */
function useSelectedLines(root) {
  const [lines, setLines] = useState(/** @type {LinePick | null} */ (null));
  const timer = useRef(/** @type {ReturnType<typeof setTimeout> | undefined} */ (undefined));
  // armed as the pane reaches the page and read once then: a file drawn from the cache is there at
  // once, and a selection made before a later listener attached would be missed
  useLayoutEffect(() => {
    const read = () => {
      const now = root.current && selectedLines(document.getSelection(), root.current);
      clearTimeout(timer.current);
      if (now) return setLines((was) => (samePick(was, now) ? was : now));
      timer.current = setTimeout(() => setLines(null), SELECTION_GRACE_MS);
    };
    read();
    document.addEventListener("selectionchange", read);
    return () => (clearTimeout(timer.current), document.removeEventListener("selectionchange", read));
  }, []);
  return { lines, drop: () => (clearTimeout(timer.current), setLines(null)) };
}

/** @param {LinePick | null} a @param {LinePick} b */
const samePick = (a, b) => Boolean(a && a.path === b.path && a.first === b.first && a.last === b.last);
