// @ts-check
// The Review tab: the open review's guide, which the host draws as the TUI's guide pane, and a
// tapped chapter open in the peek with its notes, diffs and comments. Reviewed marks the chapter
// walked, as reading to its end does in the TUI, Next opens the one after, and Comment writes on
// the picked lines or the chapter.

import { useEffect, useState } from "preact/hooks";
import { chapterPiece, guidePiece, nextChapter, targetName } from "./review.js";
import { useRendered } from "./render.js";
import { html, landedIn } from "./html.js";
import { useEscape } from "./dock.js";
import { Icon } from "./icons.js";
import { PickablePiece } from "./lines-view.js";
import { CommentSheet, Unsent, composeAt, composedEdit, tapCard } from "./comments-view.js";

/** @import { PanelHooks } from "./panel.js" */
/** @import { ReviewShown } from "./review.js" */
/** @import { Composing } from "./comments-view.js" */

/** @param {{ hooks: PanelHooks }} props */
export function ReviewTab({ hooks }) {
  const { review, send } = hooks;
  useEffect(() => void review.ask(send), []);
  const shown = review.shown.value;
  if (!shown.review) return html`<${Empty} shown=${shown} hooks=${hooks} />`;
  return html`<p class="rv-target">${targetName(shown.review.target)}</p><${Unsent} shown=${shown} hooks=${hooks} />
    <${Guide} shown=${shown} hooks=${hooks} />`;
}

/** @param {{ shown: ReviewShown, hooks: PanelHooks }} props */
function Guide({ shown, hooks }) {
  const rendered = useRendered(guidePiece(shown), false);
  return html`<div class="rv-view" onClick=${(/** @type {MouseEvent} */ event) => tapChapter(event, hooks)}
    dangerouslySetInnerHTML=${{ __html: rendered ?? "" }}></div>`;
}

/** What the tab says with no review to show, and the way to get one. @param {{ shown: ReviewShown, hooks: PanelHooks }} props */
function Empty({ shown, hooks }) {
  const { review, send } = hooks;
  const again = (/** @type {string} */ words, /** @type {() => void} */ act) =>
    html`<button type="button" class="retry" onClick=${act}>${words}</button>`;
  switch (shown.state) {
    case "lost": return html`<p class="diff-note">Connection lost. ${again("Try again", () => review.ask(send))}</p>`;
    case "failed": return html`<p class="diff-note">${shown.error} ${again("Try again", () => review.load(send))}</p>`;
    case "loading": return html`<p class="diff-note">Reading the changes…</p>`;
    case "shown": return html`<p class="diff-note">No review is open. ${again("Review changes", () => review.load(send))}</p>`;
    default: return html`<p class="diff-note">Loading…</p>`;
  }
}

/** A tap on a chapter opens it in the peek, in place of any other pane. @param {MouseEvent} event @param {PanelHooks} hooks */
export function tapChapter(event, { review, files, github }) {
  const chapter = /** @type {HTMLElement | null} */ (landedIn(event, "[data-chapter]"));
  if (!chapter?.dataset.chapter) return;
  files.close();
  github.close();
  review.pick(chapter.dataset.chapter);
}

/**
 * The open chapter, in the pane a file opens in. Escape closes it before the panel.
 * @param {{ hooks: PanelHooks, wide: boolean }} props
 */
export function ReviewPeek({ hooks, wide }) {
  const { review, send } = hooks;
  const id = review.open.value;
  const [composing, compose] = useState(/** @type {Composing | null} */ (null));
  useEffect(() => compose(null), [id]);
  useEscape(composing ? () => compose(null) : id ? review.close : null);
  const shown = review.shown.value;
  if (!id || !shown.review) return null;
  const piece = chapterPiece(shown, id);
  const submit = (/** @type {string} */ body, /** @type {boolean} */ question, /** @type {boolean} */ now) =>
    composing && (review.edit(send, composedEdit(composing, body, question, now)), compose(null));
  return html`<div class="sheet peek" role="dialog" aria-modal=${!wide} aria-label=${`Chapter ${piece.place.number} of ${piece.place.total}`}>
    <${ChapterBar} hooks=${hooks} id=${id} place=${piece.place} next=${nextChapter(shown.review, id)} />
    ${shown.problem ? html`<p class="diff-note">${shown.problem}</p>` : null}
    <div class="sheet-body rv-view" key=${id} onClick=${(/** @type {MouseEvent} */ event) => tapCard(event, hooks, compose)}>
      <${PickablePiece} piece=${piece} ask=${hooks.mention} comment=${(/** @type {any} */ pick) => compose(composeAt(shown.review, id, pick))} /></div>
    ${composing ? html`<${CommentSheet} key=${composing.heading} composing=${composing} submit=${submit} cancel=${() => compose(null)} />` : null}
  </div>`;
}

/** @param {{ hooks: PanelHooks, id: string, place: { number: number, total: number, walked: boolean }, next: string | null }} props */
function ChapterBar({ hooks: { review, send }, id, place, next }) {
  return html`<header class="bar">
    <span class="title">Chapter ${place.number} of ${place.total}</span>
    <button type="button" class="mention" aria-pressed=${place.walked} onClick=${() => review.walk(id, send)}>Reviewed</button>
    ${next ? html`<button type="button" class="mention" onClick=${() => review.pick(next)}>Next</button>` : null}
    <button type="button" class="close" aria-label="Close" onClick=${review.close}><${Icon} name="close" /></button>
  </header>`;
}
