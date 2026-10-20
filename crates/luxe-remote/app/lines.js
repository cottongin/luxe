// @ts-check
// Line picks in a file, a diff or a review chapter, to ask about those lines as the TUI's File view
// does. A tap on a line's number picks it and a tap on another stretches the pick; a tap on the same
// line, or anywhere else, lets it go. A selection made the phone's own way, down to part of a line,
// offers the same Ask for the lines it covers, and copying it stays the browser's. Ask writes what
// the TUI sends for `@[file:path:12-18]`: the path, then the lines.

/** @typedef {{ path: string, first: number, last: number }} LinePick */

/**
 * The pick after a tap on line `line` of `path`: a first tap picks it, a second stretches to it,
 * and a tap on a lone picked line, or after a stretch, starts over.
 * @param {LinePick | null} pick @param {string} path @param {number} line @returns {LinePick | null}
 */
export function tapLine(pick, path, line) {
  if (!pick || pick.path !== path || pick.first !== pick.last) return { path, first: line, last: line };
  if (pick.first === line) return null;
  return { path, first: Math.min(pick.first, line), last: Math.max(pick.first, line) };
}

/** What Ask writes into the message, as the TUI expands its line mention. @param {LinePick} pick */
export function lineWords({ path, first, last }) {
  return `\`${path}\` ${first === last ? `line ${first}` : `lines ${first}–${last}`}`;
}

/** The Ask button's words, as the TUI's action row says them. @param {LinePick} pick */
export function askLabel({ first, last }) {
  return first === last ? `Ask about line ${first}` : `Ask about lines ${first}–${last}`;
}

/**
 * The pick a tap makes: the line whose number was tapped, or nothing for a tap anywhere else.
 * @param {LinePick | null} pick @param {Element | null} target @returns {LinePick | null}
 */
export function tapped(pick, target) {
  const row = target?.closest(".ln")?.closest("[data-line]");
  const path = row?.closest("[data-path]")?.getAttribute("data-path");
  if (!row || !path) return null;
  return tapLine(pick, path, Number(row.getAttribute("data-line")));
}

/**
 * The lines a selection covers when it sits within one file's block: every row it touches, so one
 * that starts partway through a line counts that line, and deleted lines, which name none, are
 * passed over. A selection across two files, or outside `root`, offers nothing.
 * @param {Selection | null} selection @param {Element} root @returns {LinePick | null}
 */
export function selectedLines(selection, root) {
  if (!selection || selection.isCollapsed || !selection.rangeCount) return null;
  const range = selection.getRangeAt(0);
  const block = blockOf(range.commonAncestorContainer);
  const path = block?.getAttribute("data-path");
  if (!block || !path || !root.contains(block)) return null;
  const rows = [...block.querySelectorAll("[data-line]")].filter((row) => range.intersectsNode(row));
  const lines = rows.map((row) => Number(row.getAttribute("data-line")));
  return lines.length ? { path, first: Math.min(...lines), last: Math.max(...lines) } : null;
}

/** The file block a node sits in. @param {Node} node */
function blockOf(node) {
  const element = node.nodeType === 1 ? /** @type {Element} */ (node) : node.parentElement;
  return element?.closest("[data-path]") ?? null;
}

/**
 * Marks the picked rows under `root`, and only those, since the page is the host's HTML.
 * @param {Element} root @param {LinePick | null} pick
 */
export function markPicked(root, pick) {
  for (const row of root.querySelectorAll("[data-line]")) {
    const line = Number(row.getAttribute("data-line"));
    const path = row.closest("[data-path]")?.getAttribute("data-path");
    row.classList.toggle("picked", Boolean(pick && path === pick.path && line >= pick.first && line <= pick.last));
  }
}
