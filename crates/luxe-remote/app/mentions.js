// @ts-check
// `@` mentions of project files, picked from the project's own folders as `list_directory` gives
// them. A picked file goes into the box as its path in backticks, which is what the TUI sends the
// model for one, so what you see is what the model reads; a picked folder opens the next level.

import { signal } from "@preact/signals";

/** @import { Frame } from "./connection.js" */
/**
 * @typedef {{ name: string, kind: "directory" | "file" }} Entry
 * @typedef {{ state: "asking" | "listed" | "failed", entries: Entry[] }} Listing
 * @typedef {{ start: number, folder: string, name: string }} MentionQuery
 */

/**
 * The mention being typed just before `caret`: an `@` at the start or after a space, then no
 * space. `folder` is what's typed up to its last slash, `name` the rest.
 * @param {string} text @param {number} caret @returns {MentionQuery | null}
 */
export function mentionAt(text, caret) {
  const typed = /(?:^|\s)@([^\s@`]*)$/.exec(text.slice(0, caret));
  if (!typed) return null;
  const query = typed[1];
  const cut = query.lastIndexOf("/");
  return { start: caret - query.length - 1, folder: query.slice(0, Math.max(cut, 0)), name: query.slice(cut + 1) };
}

/** Entries whose name starts with `name`, then those holding it, in the listing's order. @param {Entry[]} entries @param {string} name */
export function matchEntries(entries, name) {
  const needle = name.toLowerCase();
  const starting = entries.filter((entry) => entry.name.toLowerCase().startsWith(needle));
  const holding = entries.filter((entry) => !starting.includes(entry) && entry.name.toLowerCase().includes(needle));
  return [...starting, ...holding];
}

/**
 * `text` with the mention at `query` replaced by `entry`, and where the caret goes after it.
 * @param {string} text @param {number} caret @param {MentionQuery} query @param {Entry} entry
 */
export function pickEntry(text, caret, query, entry) {
  const path = query.folder ? `${query.folder}/${entry.name}` : entry.name;
  const inserted = entry.kind === "directory" ? `@${path}/` : `\`${path}\` `;
  return { text: text.slice(0, query.start) + inserted + text.slice(caret), caret: query.start + inserted.length };
}

/**
 * Each folder's entries, kept while the session is open: the picker asks once, and the Files tab
 * asks `fresh` each time it opens one, since the agent may have changed it since.
 */
export function createListings() {
  const folders = signal(/** @type {Record<string, Listing>} */ ({}));
  /** @type {Map<string, string>} */
  const asked = new Map();
  /** @param {string} folder @param {Listing} listing */
  const store = (folder, listing) => (folders.value = { ...folders.value, [folder]: listing });
  /**
   * Asks for `folder` unless it's on its way, or known and not wanted `fresh`.
   * @param {string} folder @param {(frame: Frame) => boolean} send @param {{ fresh?: boolean }} [options]
   */
  const ask = (folder, send, { fresh = false } = {}) => askListing({ folders, asked, store }, folder, send, fresh);
  /** @param {Frame} frame */
  const apply = (frame) => {
    if (frame.kind === "ready") folders.value = withoutAsking(folders.value);
    else applyListing(frame, asked, store);
  };
  return { folders, ask, apply };
}

/**
 * @param {{ folders: { value: Record<string, Listing> }, asked: Map<string, string>,
 *   store: (folder: string, listing: Listing) => void }} listings
 * @param {string} folder @param {(frame: Frame) => boolean} send @param {boolean} fresh
 */
function askListing({ folders, asked, store }, folder, send, fresh) {
  const known = folders.value[folder];
  if (known && (!fresh || known.state === "asking")) return;
  const request_id = askFor(folder, send);
  if (!request_id) return;
  asked.set(request_id, folder);
  // what was listed before stays up until the answer replaces it
  store(folder, { state: "asking", entries: known?.entries ?? [] });
}

/**
 * Sends the question for `folder`, returning its id, or null when it went nowhere.
 * @param {string} folder @param {(frame: Frame) => boolean} send
 */
function askFor(folder, send) {
  // every client hears every answer, so the id has to be ours alone
  const request_id = `listing:${crypto.randomUUID()}`;
  return send({ kind: "list_directory", content: { request_id, path: folder } }) ? request_id : null;
}

/**
 * `folders` less those still on their way, whose answers a new connection never brings, so the
 * next look asks again.
 * @param {Record<string, Listing>} folders
 */
export function withoutAsking(folders) {
  return Object.fromEntries(Object.entries(folders).filter(([, listing]) => listing.state !== "asking"));
}

/**
 * Files the answer to one of `asked`'s questions under its folder.
 * @param {Frame} frame @param {Map<string, string>} asked
 * @param {(folder: string, listing: Listing) => void} store
 */
function applyListing({ kind, content }, asked, store) {
  const folder = asked.get(content?.request_id);
  if (folder === undefined) return;
  if (kind === "directory_listed") store(folder, { state: "listed", entries: content.listing.entries });
  if (kind === "directory_list_failed") store(folder, { state: "failed", entries: [] });
}
