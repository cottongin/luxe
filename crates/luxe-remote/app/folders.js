// @ts-check
// Starting a session: a page of its own, with the folders you've worked in narrowed by what you
// type, and any other folder you type.
//
// The bridge answers a start once the session can be joined, which takes a few seconds, so the
// page says which folder it's starting in meanwhile and opens the session when it's ready.

import { signal } from "@preact/signals";
import { useEffect, useState } from "preact/hooks";
import { fieldOf, html } from "./html.js";

/**
 * A place a session can start, as `GET /api/folders` lists it.
 * @typedef {{ path: string, name: string, sessions: number }} Folder
 * @typedef {{ folders: Folder[], herdr: boolean }} Folders
 */

/** @type {import("@preact/signals").Signal<Folders | null>} */
const folders = signal(null);
/** Whether this bridge starts sessions at all; one run without a launcher says not. */
export const offered = signal(true);
const starting = signal("");
const problem = signal("");

/** Reads the folders again. @param {typeof fetch} get */
export async function refreshFolders(get = fetch) {
  const response = await get("/api/folders");
  if (response.status === 501) offered.value = false;
  if (!response.ok) return;
  folders.value = await response.json();
}

/**
 * Starts a session in `folder`, resolving to its id once it can be joined; the host's reason
 * when it can't. @param {string} folder @param {typeof fetch} post
 */
export async function startIn(folder, post = fetch) {
  const response = await post("/api/sessions", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ folder }),
  });
  if (!response.ok) throw new Error((await response.text()) || `starting failed: ${response.status}`);
  return (await response.json()).session.session_id;
}

/**
 * The folders `typed` could mean, best first: a name that starts with it, then a name holding it,
 * then a path holding it; nothing typed starts every name, so all of them come back in order.
 * A leading `~/` is dropped, since the phone can't know which folder is home, and the rest still
 * finds the path under it. @param {Folder[]} all @param {string} typed
 */
export function matchFolders(all, typed) {
  const needle = typed.trim().replace(/^~[\\/]?/, "").toLowerCase();
  /** @param {Folder} folder */
  const rank = (folder) => {
    const name = folder.name.toLowerCase();
    if (name.startsWith(needle)) return 0;
    if (name.includes(needle)) return 1;
    return folder.path.toLowerCase().includes(needle) ? 2 : 3;
  };
  const ranked = all.map((folder) => ({ folder, rank: rank(folder) })).filter((entry) => entry.rank < 3);
  return ranked.sort((a, b) => a.rank - b.rank).map((entry) => entry.folder);
}

/** @param {string} folder */
async function start(folder) {
  starting.value = folder;
  problem.value = "";
  try {
    location.hash = `#/session/${encodeURIComponent(await startIn(folder))}`;
  } catch (error) {
    problem.value = /** @type {Error} */ (error).message;
  } finally {
    starting.value = "";
  }
}

export function NewSession() {
  const [typed, setTyped] = useState("");
  useEffect(() => {
    refreshFolders().catch((error) => (problem.value = `couldn't read the folders: ${error}`));
  }, []);
  return html`
    <header class="bar"><a class="back" href="#/" aria-label="Sessions">‹</a><span class="title">New session</span></header>
    <main class="new-session">
      <${FolderField} typed=${typed} setTyped=${setTyped} />
      ${problem.value ? html`<p class="problem">${problem.value}</p>` : null}
      <${Where} />
      <${Suggestions} typed=${typed} />
    </main>`;
}

/** @param {{ typed: string, setTyped: (typed: string) => void }} props */
function FolderField({ typed, setTyped }) {
  const asked = typed.trim();
  /** @param {SubmitEvent} event */
  const submit = (event) => {
    event.preventDefault();
    if (asked) start(asked);
  };
  return html`<form class="other-folder" onSubmit=${submit}>
    <input type="text" placeholder="Folder path, like ~/code/app" value=${typed} autocapitalize="off"
      autocorrect="off" spellcheck="false" enterkeyhint="go" aria-label="Folder"
      onInput=${(/** @type {Event} */ event) => setTyped(fieldOf(event).value)} />
    <button type="submit" disabled=${!asked || starting.value !== ""}>
      ${starting.value === asked && asked ? "Starting…" : "Start"}</button>
  </form>`;
}

function Where() {
  if (!folders.value) return null;
  const herdr = folders.value.herdr;
  return html`<p class="dim">${herdr ? "Opens in a herdr pane on your computer." : "Runs until luxe remote stops."}</p>`;
}

/** @param {{ typed: string }} props */
function Suggestions({ typed }) {
  if (!folders.value) return html`<p class="dim">Loading folders…</p>`;
  const matched = matchFolders(folders.value.folders, typed);
  if (!matched.length) return html`<p class="dim">${typed.trim() ? "No matching folders." : "No recent folders."}</p>`;
  return html`<ul class="sessions">
    ${matched.map((folder) => html`<li key=${folder.path}><${FolderRow} folder=${folder} /></li>`)}
  </ul>`;
}

/** @param {{ folder: Folder }} props */
function FolderRow({ folder }) {
  const busy = starting.value !== "";
  const running = folder.sessions ? ` · ${folder.sessions} running` : "";
  return html`<button type="button" class="session folder" disabled=${busy} onClick=${() => start(folder.path)}>
    <span class="name">${starting.value === folder.path ? `Starting in ${folder.name}…` : folder.name}</span>
    <span class="dim path">${folder.path}${running}</span>
  </button>`;
}
