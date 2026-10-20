// @ts-check
// The Files tab's state: which folders of the tree are open, as the TUI's FILES unfolds them in
// place, and the one file open, read with `read_project_file` and matched to its answer by an id no
// other client shares, so a late answer for a closed file is dropped. Folders come from the
// listings the `@` picker keeps (mentions.js).

import { signal } from "@preact/signals";

/** @import { Frame } from "./connection.js" */
/** @import { Entry, Listing } from "./mentions.js" */
/**
 * @typedef {{ path: string, requestId: string, state: "asking" | "read" | "failed" | "lost",
 *   file?: any, error?: any }} OpenFile
 * @typedef {{ kind: "folder", path: string, name: string, depth: number, open: boolean }
 *   | { kind: "file", path: string, name: string, depth: number }
 *   | { kind: "note", depth: number, text: string }} TreeRow
 */

/** Every folder closed and no file open. */
export function createFiles() {
  const unfolded = signal(/** @type {ReadonlySet<string>} */ (new Set()));
  const open = signal(/** @type {OpenFile | null} */ (null));
  /** Asks for `path`, showing it as loading until the answer. @param {string} path @param {(frame: Frame) => boolean} send */
  const read = (path, send) => {
    const request_id = `file:${crypto.randomUUID()}`;
    const state = send({ kind: "read_project_file", content: { request_id, path } }) ? "asking" : "lost";
    open.value = { path, requestId: request_id, state };
  };
  /** @param {Frame} frame */
  const apply = (frame) => (open.value = applyRead(open.value, frame));
  /** Opens `path` if it's closed and closes it if it's open. @param {string} path */
  const toggle = (path) => (unfolded.value = toggled(unfolded.value, path));
  return { unfolded, toggle, open, read, apply, close: () => (open.value = null) };
}

/** `set` with `path` in it if it wasn't, and without it if it was. @param {ReadonlySet<string>} set @param {string} path */
export function toggled(set, path) {
  const next = new Set(set);
  if (!next.delete(path)) next.add(path);
  return next;
}

/**
 * The rows the tree shows: `folder`'s entries at `depth`, and inside each open folder its own, as
 * the TUI's FILES unfolds in place. A folder with nothing to list yet shows a note instead.
 * @param {Record<string, Listing>} folders @param {ReadonlySet<string>} unfolded
 * @param {string} [folder] @param {number} [depth] @returns {TreeRow[]}
 */
export function treeRows(folders, unfolded, folder = "", depth = 0) {
  const listing = folders[folder];
  const note = entriesNote(listing);
  if (note || !listing) return [{ kind: "note", depth, text: note }];
  return listing.entries.flatMap((entry) => entryRows(entry, { folders, unfolded, folder, depth }));
}

/**
 * An entry's row, then, for an open folder, the rows inside it.
 * @param {Entry} entry
 * @param {{ folders: Record<string, Listing>, unfolded: ReadonlySet<string>, folder: string, depth: number }} at
 * @returns {TreeRow[]}
 */
function entryRows(entry, { folders, unfolded, folder, depth }) {
  const path = childPath(folder, entry.name);
  if (entry.kind !== "directory") return [{ kind: "file", path, name: entry.name, depth }];
  const open = unfolded.has(path);
  /** @type {TreeRow} */
  const row = { kind: "folder", path, name: entry.name, depth, open };
  return open ? [row, ...treeRows(folders, unfolded, path, depth + 1)] : [row];
}

/** What to say in place of a folder's entries, or "" when there are some to show. @param {Listing | undefined} listing */
export function entriesNote(listing) {
  if (listing?.entries.length) return "";
  if (!listing || listing.state === "asking") return "Loading…";
  return listing.state === "failed" ? "Couldn't list this folder" : "Empty folder";
}

/**
 * The open file after `frame`: answered, or lost when the connection began again before it was.
 * @param {OpenFile | null} open @param {Frame} frame @returns {OpenFile | null}
 */
export function applyRead(open, { kind, content }) {
  if (open?.state !== "asking") return open;
  if (kind === "ready") return { ...open, state: "lost" };
  if (content?.request_id !== open.requestId) return open;
  if (kind === "project_file_read") return { ...open, state: "read", file: content.file };
  if (kind === "project_file_read_failed") return { ...open, state: "failed", error: content.error };
  return open;
}

/** `name` inside `folder`, as the protocol writes a project path. @param {string} folder @param {string} name */
export function childPath(folder, name) {
  return folder ? `${folder}/${name}` : name;
}

/** What a mention of the file at `path` adds to the draft, as a picked `@` mention writes it. @param {string} path */
export function fileMention(path) {
  return `\`${path}\``;
}

/**
 * `text` with `words` added at its end, a space apart, and the caret after.
 * @param {string} text @param {string} words
 */
export function withMention(text, words) {
  const gap = text && !/\s$/.test(text) ? " " : "";
  const next = `${text}${gap}${words} `;
  return { text: next, caret: next.length, placed: true };
}
