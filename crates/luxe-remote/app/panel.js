// @ts-check
// The project panel: the TUI's views of the project, gathered behind one header button. On a phone
// it covers the page and holds the dock's plan and changes too, so one button reaches everything;
// on a wide screen it's a rail in the dock's place, as the TUI keeps FILES in its dock, and a file,
// a pull request or a review chapter opens in a pane beside it, leaving the conversation up where
// there's room. One pane at a time: opening one closes the others.

import { signal } from "@preact/signals";
import { DockTab, useEscape } from "./dock.js";
import { FilePeek, FilesTab } from "./files-view.js";
import { GithubPeek, GithubTab } from "./github-view.js";
import { html } from "./html.js";
import { Icon } from "./icons.js";
import { panelTabs, shownTab, useWide } from "./layout.js";
import { useRendered } from "./render.js";
import { ReviewPeek, ReviewTab } from "./review-view.js";

/** @import { Frame } from "./connection.js" */
/** @import { Listings } from "./picker.js" */
/**
 * @typedef {import("./layout.js").Tab} Tab
 * @typedef {ReturnType<typeof createPanel>} Panel
 * @typedef {{ send: (frame: Frame) => boolean, listings: Listings, files: ReturnType<typeof import("./files.js").createFiles>,
 *   github: ReturnType<typeof import("./github.js").createGithub>, review: ReturnType<typeof import("./review.js").createReview>,
 *   mention: (words: string) => void, changes: any, todos: any, openDiff: (path: string) => void, diffOpen: boolean,
 *   project: Project }} PanelHooks
 * @typedef {{ name: string, branch: string | null, dirty: boolean }} Project
 */

/** @type {Record<Tab, (props: { hooks: PanelHooks }) => any>} */
const TAB_VIEWS = { dock: DockTab, files: FilesTab, github: GithubTab, review: ReviewTab };

/** Closed; `open` shows a tab, the screen's first unless told. */
export function createPanel() {
  const tab = signal(/** @type {Tab | null} */ (null));
  return { tab, open: (/** @type {Tab | null} */ id = null) => (tab.value = id ?? "dock"), close: () => (tab.value = null) };
}

/** The header's button that opens the panel. @param {{ open: () => void }} props */
export function PanelButton({ open }) {
  const label = useWide() ? "Project" : "Dock and project";
  return html`<button type="button" class="panel-button" aria-label=${label} onClick=${open}><${Icon} name="menu" /></button>`;
}

/** @param {{ panel: Panel, hooks: PanelHooks }} props */
export function ProjectPanel({ panel, hooks }) {
  const wide = useWide();
  // an open file, item or diff takes Escape first
  useEscape(panel.tab.value && !paneOpen(hooks) ? panel.close : null);
  if (!panel.tab.value) return null;
  const tab = shownTab(panel.tab.value, wide);
  /** @param {Tab} next */
  const pick = (next) => switchTab(panel, hooks, next);
  return html`<div class="sheet panel" role="dialog" aria-modal=${!wide} aria-label="Project">
    <header class="bar">
      <span class="title"><span class="badge">Project</span><${ProjectName} project=${hooks.project} /></span>
      <button type="button" class="close" aria-label="Close the project panel" onClick=${panel.close}><${Icon} name="close" /></button>
    </header>
    <${Tabs} tab=${tab} pick=${pick} wide=${wide} />
    <div class="sheet-body"><${TAB_VIEWS[tab]} hooks=${hooks} /></div>
  </div>
  <${FilePeek} hooks=${hooks} wide=${wide} />
  <${GithubPeek} hooks=${hooks} wide=${wide} />
  <${ReviewPeek} hooks=${hooks} wide=${wide} />`;
}

/** Whether a file, an item, a chapter or a diff is open over the panel. @param {PanelHooks} hooks */
const paneOpen = ({ files, github, review, diffOpen }) =>
  Boolean(files.open.value || github.open.value || review.open.value || diffOpen);

/**
 * The folder the session works in and its branch, as the TUI's project section has them, in the
 * session's header and the panel's. The host draws it so the glyphs are the TUI's, at the tree's
 * tier; the bare name stands in until it does.
 * @param {{ project: Project }} props
 */
export function ProjectName({ project }) {
  const rendered = useRendered({ kind: "project_name", ...project }, true);
  if (!project.name) return null;
  if (!rendered) return html`<span class="project-name">${project.name}</span>`;
  return html`<span class="project-name" dangerouslySetInnerHTML=${{ __html: rendered }}></span>`;
}

/** Shows `next`, closing the pane the last tab opened. @param {Panel} panel @param {PanelHooks} hooks @param {Tab} next */
function switchTab(panel, hooks, next) {
  hooks.files.close();
  hooks.github.close();
  hooks.review.close();
  panel.open(next);
}

/** @param {{ tab: Tab, pick: (tab: Tab) => void, wide: boolean }} props */
function Tabs({ tab, pick, wide }) {
  return html`<nav class="tabs" role="tablist" aria-label="Project views">
    ${panelTabs(wide).map(({ id, title }) => html`<button type="button" role="tab" key=${id} class="tab"
      aria-selected=${id === tab} onClick=${() => pick(id)}><span class=${id === tab ? "badge raised" : "badge idle"}>${title}</span></button>`)}
  </nav>`;
}
