// @ts-check
// What the box offers as you type: the slash commands at the start of a message, and project
// files after an `@`. A tap fills in or runs the choice, and the box keeps the keyboard, since a
// button that took focus would drop it.

import { useEffect } from "preact/hooks";
import { matchCommands } from "./commands.js";
import { html } from "./html.js";
import { matchEntries, mentionAt, pickEntry } from "./mentions.js";

/** @import { Frame } from "./connection.js" */
/** @import { Command } from "./commands.js" */
/** @import { Entry, Listing } from "./mentions.js" */
/**
 * @typedef {{ key: string, label: string, hint: string, command?: Command, entry?: Entry }} Suggestion
 * @typedef {{ text: string, caret: number, placed?: boolean }} Draft
 * @typedef {ReturnType<typeof import("./mentions.js").createListings>} Listings
 */

/** The most entries listed at once; typing narrows the rest. */
const MOST = 8;

/** What to offer for `draft`, from the folders listed so far. @param {Draft} draft @param {Record<string, Listing>} folders */
export function suggestions(draft, folders) {
  const commands = matchCommands(draft.text);
  if (commands.length) return commands.map(commandSuggestion);
  const query = mentionAt(draft.text, draft.caret);
  if (!query) return [];
  return matchEntries(folders[query.folder]?.entries ?? [], query.name).slice(0, MOST).map(entrySuggestion);
}

/** @param {Command} command @returns {Suggestion} */
function commandSuggestion(command) {
  return { key: command.name, label: `/${command.name}`, hint: command.title, command };
}

/** @param {Entry} entry @returns {Suggestion} */
function entrySuggestion(entry) {
  return { key: entry.name, label: entry.kind === "directory" ? `${entry.name}/` : entry.name, hint: "", entry };
}

/**
 * The draft after picking `item`: a file or folder replaces the mention, and a command that takes
 * words goes into the box to finish. Null for a command that runs as it is.
 * @param {Draft} draft @param {Suggestion} item @returns {Draft | null}
 */
export function afterPick(draft, item) {
  const query = mentionAt(draft.text, draft.caret);
  if (item.entry && query) return { ...pickEntry(draft.text, draft.caret, query, item.entry), placed: true };
  if (!item.command?.takes) return null;
  const text = `/${item.command.name} `;
  return { text, caret: text.length, placed: true };
}

/** What the picker says when it has nothing to list yet. @param {Draft} draft @param {Record<string, Listing>} folders */
export function pickerNote(draft, folders) {
  const query = mentionAt(draft.text, draft.caret);
  const listing = query ? folders[query.folder] : undefined;
  const where = query?.folder || "the project";
  if (listing?.state === "asking") return "Listing files…";
  if (listing?.state === "failed") return `Couldn't list ${where}`;
  if (listing?.state === "listed" && !listing.entries.length) return `No files in ${where}`;
  return "";
}

/**
 * @param {{ draft: Draft, listings: Listings, send: (frame: Frame) => boolean,
 *   place: (draft: Draft) => void, run: (text: string) => void }} props
 */
export function Picker({ draft, listings, send, place, run }) {
  const folder = mentionAt(draft.text, draft.caret)?.folder;
  useEffect(() => void (folder !== undefined && listings.ask(folder, send)), [folder]);
  const folders = listings.folders.value;
  const items = suggestions(draft, folders);
  const note = items.length ? "" : pickerNote(draft, folders);
  if (!items.length && !note) return null;
  /** @param {Suggestion} item */
  const choose = (item) => {
    const next = afterPick(draft, item);
    if (next) place(next);
    else run(`/${item.command?.name}`);
  };
  return html`<ul class="picker" aria-label="Suggestions">${note ? html`<li class="dim">${note}</li>` : null}
    ${items.map((item) => html`<li key=${item.key}><button type="button" onPointerDown=${keepFocus} onClick=${() => choose(item)}>
      <span class="name">${item.label}</span>${item.hint ? html` <span class="dim">${item.hint}</span>` : null}</button></li>`)}
  </ul>`;
}

/** Keeps the box focused, and the keyboard up, through a tap on a suggestion. @param {Event} event */
function keepFocus(event) {
  event.preventDefault();
}
