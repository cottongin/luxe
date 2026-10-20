// @ts-check
// The Files tab: the project as the TUI's FILES tree, drawn by the host so its guides, marks and
// change counts are the TUI's, with folders unfolding in place, and a file read in place,
// highlighted as the host renders it, with a way to mention it to the agent.

import { useEffect } from "preact/hooks";
import { useRendered } from "./render.js";
import { fileMention, treeRows } from "./files.js";
import { html, landedIn } from "./html.js";
import { useEscape } from "./dock.js";
import { Icon } from "./icons.js";
import { PickablePiece } from "./lines-view.js";

/** @import { PanelHooks } from "./panel.js" */
/** @import { OpenFile } from "./files.js" */

/** @param {{ hooks: PanelHooks }} props */
export function FilesTab({ hooks }) {
  return html`<${TreeView} hooks=${hooks} />`;
}

/** @param {{ hooks: PanelHooks }} props */
function TreeView({ hooks }) {
  const { files, listings, changes } = hooks;
  useFreshFolders(hooks);
  const rows = treeRows(listings.folders.value, files.unfolded.value);
  const rendered = useRendered({ kind: "tree", rows, changes }, false);
  return html`<div class="tree-view" onClick=${(/** @type {MouseEvent} */ event) => tapRow(event, hooks)}
    dangerouslySetInnerHTML=${{ __html: rendered ?? "" }}></div>`;
}

/**
 * Lists the root and every open folder afresh when the tree is shown, since the agent may have
 * changed them since. @param {PanelHooks} hooks
 */
function useFreshFolders({ files, listings, send }) {
  useEffect(() => {
    for (const folder of ["", ...files.unfolded.value]) listings.ask(folder, send, { fresh: true });
  }, []);
}

/** A tap on a folder unfolds or folds it; one on a file opens it. @param {MouseEvent} event @param {PanelHooks} hooks */
export function tapRow(event, { files, github, review, listings, send }) {
  const row = landedIn(event, "button.tree-row");
  const folder = row?.dataset.folder;
  if (folder !== undefined) {
    files.toggle(folder);
    if (files.unfolded.value.has(folder)) listings.ask(folder, send, { fresh: true });
    return;
  }
  const file = row?.dataset.file;
  if (file === undefined) return;
  github.close();
  review.close();
  files.read(file, send);
}

/**
 * The open file, in the pane a diff opens in: over the page on a phone, beside the tree on a wide
 * screen, as the TUI peeks a file beside its dock. Escape closes it before the panel.
 * @param {{ hooks: PanelHooks, wide: boolean }} props
 */
export function FilePeek({ hooks, wide }) {
  const { files, send } = hooks;
  const open = files.open.value;
  useEscape(open ? files.close : null);
  if (!open) return null;
  return html`<div class="sheet peek" role="dialog" aria-modal=${!wide} aria-label=${open.path}>
    <header class="bar">
      <span class="title">${open.path}</span>
      <button type="button" class="mention" onClick=${() => hooks.mention(fileMention(open.path))}>Mention</button>
      <button type="button" class="close" aria-label="Close the file" onClick=${files.close}><${Icon} name="close" /></button>
    </header>
    <div class="sheet-body"><${FileBody} open=${open} retry=${() => files.read(open.path, send)} ask=${hooks.mention} /></div>
  </div>`;
}

/** @param {{ open: OpenFile, retry: () => void, ask: (words: string) => void }} props */
function FileBody({ open, retry, ask }) {
  if (open.state === "asking") return html`<p class="diff-note">Loading file…</p>`;
  if (open.state === "lost") {
    return html`<p class="diff-note">Connection lost. <button type="button" class="retry" onClick=${retry}>Try again</button></p>`;
  }
  return html`<${PickablePiece} piece=${open.state === "read" ? { kind: "file", file: open.file } : { kind: "file_failed", error: open.error }} ask=${ask} />`;
}
