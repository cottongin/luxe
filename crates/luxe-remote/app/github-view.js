// @ts-check
// The GitHub tab: `/github`'s sections as badges, the shown one's rows as the TUI draws them, and
// a tapped row open in the peek, its header over an Overview and a Conversation. The host draws
// every row, check and card; the app only says which, and owns the taps.

import { useEffect } from "preact/hooks";
import { githubMention, rowSubject, shownSection } from "./github.js";
import { useRendered } from "./render.js";
import { html, landedIn } from "./html.js";
import { useEscape } from "./dock.js";
import { Icon } from "./icons.js";

/** @import { PanelHooks } from "./panel.js" */
/** @import { OpenItem, Reading, Subject } from "./github.js" */

/** @param {{ hooks: PanelHooks }} props */
export function GithubTab({ hooks }) {
  const { github, send } = hooks;
  useEffect(() => void github.list(send), []);
  const listing = github.listing.value;
  if (listing.state === "lost" && !listing.sections.length) return html`<${Lost} retry=${() => github.list(send)} />`;
  if (!listing.sections.length) return html`<p class="diff-note">${listing.state === "listed" ? "No sections" : "Loading…"}</p>`;
  const shown = shownSection(listing.sections, github.section.value);
  return html`<${Sections} sections=${listing.sections} shown=${shown.name} pick=${github.pick} />
    <${List} hooks=${hooks} section=${shown} />`;
}

/** @param {{ retry: () => void }} props */
function Lost({ retry }) {
  return html`<p class="diff-note">Connection lost. <button type="button" class="retry" onClick=${retry}>Try again</button></p>`;
}

/**
 * The section tabs on one line, as `/github` draws them: the open one a slab, the rest quiet
 * words, each with how many it holds, scrolling sideways when they don't fit.
 * @param {{ sections: any[], shown: string, pick: (name: string) => void }} props
 */
function Sections({ sections, shown, pick }) {
  return html`<nav class="tabs gh-sections" aria-label="GitHub sections">
    ${sections.map(({ name, items, total }) => html`<button type="button" key=${name} class="tab" aria-pressed=${name === shown}
      onClick=${() => pick(name)}><span class=${name === shown ? "badge picked" : "gh-tab"}>${name} ${total ?? items.length}</span></button>`)}
  </nav>`;
}

/** @param {{ hooks: PanelHooks, section: any }} props */
function List({ hooks, section }) {
  const { repository, at } = hooks.github.listing.value;
  const rendered = useRendered({ kind: "github_list", repository, section, now: at }, false);
  return html`<div class="gh-view" onClick=${(/** @type {MouseEvent} */ event) => tapGithubRow(event, hooks)}
    dangerouslySetInnerHTML=${{ __html: rendered ?? "" }}></div>`;
}

/** A tap on a row opens it in the peek, in place of any open file. @param {MouseEvent} event @param {PanelHooks} hooks */
export function tapGithubRow(event, { github, files, review, send }) {
  const row = landedIn(event, "button.gh-row");
  if (!row) return;
  files.close();
  review.close();
  github.read(rowSubject(row, github.listing.value.repository), send);
}

/**
 * The open item, in the pane a file opens in. Escape closes it before the panel.
 * @param {{ hooks: PanelHooks, wide: boolean }} props
 */
export function GithubPeek({ hooks, wide }) {
  const { github } = hooks;
  const item = github.open.value;
  useEscape(item ? github.close : null);
  if (!item) return null;
  const here = github.listing.value.repository;
  const name = `${item.subject.repository ?? ""}#${item.subject.number}`;
  return html`<div class="sheet peek" role="dialog" aria-modal=${!wide} aria-label=${name}>
    <header class="bar">
      <span class="title">${name}</span>
      <a class="mention" href=${item.subject.url} target="_blank" rel="noopener noreferrer">GitHub</a>
      <button type="button" class="mention" onClick=${() => hooks.mention(githubMention(item.subject, here))}>Mention</button>
      <button type="button" class="close" aria-label="Close" onClick=${github.close}><${Icon} name="close" /></button>
    </header>
    <div class="sheet-body gh-view" onClick=${(/** @type {MouseEvent} */ event) => tapDetail(event, hooks)}><${Detail} item=${item} hooks=${hooks} /></div>
  </div>`;
}

/** @param {{ item: OpenItem, hooks: PanelHooks }} props */
function Detail({ item, hooks }) {
  const retry = () => hooks.github.read(item.subject, hooks.send);
  const { overview } = item;
  if (overview.state !== "read") return html`<${Waiting} reading=${overview} retry=${retry} what="overview" />`;
  const shown = item.tab === "overview" ? overview : item.conversation;
  return html`<${Piece} piece=${{ kind: "github_header", overview: overview.value, now: overview.at }} />
    <${DetailTabs} tab=${item.tab} show=${hooks.github.show} said=${overview.value.overview?.conversation_count ?? 0} />
    ${shown.state === "read" ? html`<${Piece} piece=${detailPiece(item)} />` : html`<${Waiting} reading=${shown} retry=${retry} what="conversation" />`}`;
}

/** The shown tab's piece. @param {OpenItem} item */
function detailPiece({ tab, overview, conversation }) {
  if (tab === "overview") return { kind: "github_overview", overview: overview.value, now: overview.at };
  const opened_by_you = overview.value.overview?.viewer?.author === true;
  return { kind: "github_conversation", entries: conversation.value, now: conversation.at, opened_by_you };
}

/** @param {{ reading: Reading, retry: () => void, what: string }} props */
function Waiting({ reading, retry, what }) {
  if (reading.state === "lost") return html`<${Lost} retry=${retry} />`;
  if (reading.state === "failed") return html`<p class="diff-note">${reading.error}</p>`;
  return html`<p class="diff-note">Reading the ${what}…</p>`;
}

/**
 * The detail's tabs, on the TUI's tab line, Conversation counting what's been said.
 * @param {{ tab: import("./github.js").DetailTab, show: (tab: import("./github.js").DetailTab) => void, said: number }} props
 */
function DetailTabs({ tab, show, said }) {
  /** @type {[import("./github.js").DetailTab, string][]} */
  const tabs = [["overview", "Overview"], ["conversation", said ? `Conversation ${said}` : "Conversation"]];
  return html`<nav class="tabs gh-sections" role="tablist" aria-label="Views">
    ${tabs.map(([id, title]) => html`<button type="button" role="tab" key=${id} class="tab" aria-selected=${id === tab}
      onClick=${() => show(id)}><span class=${id === tab ? "badge picked" : "gh-tab"}>${title}</span></button>`)}
  </nav>`;
}

/** @param {{ piece: import("./render.js").Piece }} props */
function Piece({ piece }) {
  const rendered = useRendered(piece, false);
  return html`<div dangerouslySetInnerHTML=${{ __html: rendered ?? "" }}></div>`;
}

/**
 * Taps inside the detail: "open #N" opens that pull request, "more lines in Conversation" shows
 * it, and "Show all" unfolds a long comment. @param {MouseEvent} event @param {PanelHooks} hooks
 */
export function tapDetail(event, { github, send }) {
  const item = github.open.value;
  const open = landedIn(event, "button.gh-open");
  if (item && open) return github.read(linkedSubject(item.subject, Number(open.dataset.number), github.listing.value.repository), send);
  if (landedIn(event, "button.gh-more")) return github.show("conversation");
  landedIn(event, "button.gh-unfold")?.closest(".gh-card")?.classList.remove("folded");
}

/**
 * The pull request `number` in the same repository as `subject`.
 * @param {Subject} subject @param {number} number @param {string | null} here @returns {Subject}
 */
export function linkedSubject(subject, number, here) {
  const repository = subject.repository ?? here;
  return { number, pull_request: true, repository: subject.repository, url: `https://github.com/${repository}/pull/${number}` };
}
