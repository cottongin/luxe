// @ts-check
// The sessions running on your computer, kept current while the list is on screen.

import { signal } from "@preact/signals";
import { useEffect } from "preact/hooks";
import { offered, refreshFolders } from "./folders.js";
import { html } from "./html.js";
import { NotificationSwitch, refreshNotifications } from "./notifications.js";
import { PairWithLink } from "./pairing.js";
import { loadWordmark, Wordmark } from "./wordmark.js";

/**
 * A running session, as `GET /api/sessions` lists it.
 * @typedef {{ session_id: string, cwd: string, pid: number, luxe_version: string }} Session
 */

const REFRESH_MS = 5000;

/** @type {import("@preact/signals").Signal<Session[] | null>} */
const sessions = signal(null);
const problem = signal("");
// 401 is the bridge's answer to a device it doesn't know; the tailnet login itself is never
// missing behind Serve, so the way in to offer is pairing
const unpaired = signal(false);

async function refresh() {
  if (document.visibilityState !== "visible") return;
  try {
    const response = await fetch("/api/sessions");
    unpaired.value = response.status === 401;
    if (!response.ok) {
      problem.value = await response.text();
      return;
    }
    problem.value = "";
    sessions.value = await response.json();
    await refreshFolders();
  } catch (error) {
    problem.value = `couldn't reach your computer: ${error}`;
  }
}

/**
 * The computer this app reaches, as the bridge names the page for it, or nothing when it couldn't
 * ask Tailscale. One phone can have an app for each of several computers.
 * @param {Pick<Document, "querySelector">} page @returns {string}
 */
export function machineName(page) {
  return page.querySelector('meta[name="luxe-machine"]')?.getAttribute("content") ?? "";
}

/** @param {{ name: string }} props */
function Machine({ name }) {
  return name ? html`<span class="machine">${name}</span>` : null;
}

export function SessionList() {
  useEffect(() => {
    refresh();
    refreshNotifications().catch((error) => console.warn("couldn't read the notifications", error));
    loadWordmark().catch((error) => console.warn("couldn't draw the wordmark", error));
    const timer = setInterval(refresh, REFRESH_MS);
    return () => clearInterval(timer);
  }, []);
  return html`
    <header class="bar"><${Wordmark} /><${Machine} name=${machineName(document)} />${offered.value && !unpaired.value ? html`<a class="new" href="#/new">+ New</a>` : null}</header>
    <main>
      ${problem.value ? html`<p class="problem">${problem.value}</p>` : null}
      ${unpaired.value ? html`<${PairWithLink} />` : html`
        <h2>Running</h2>
        <${Rows} sessions=${sessions.value} />
        <${NotificationSwitch} />`}
    </main>
  `;
}

/** @param {{ sessions: Session[] | null }} props */
function Rows({ sessions }) {
  if (!sessions) return html`<p class="dim">Loading sessions…</p>`;
  if (!sessions.length) return html`<p class="dim">No sessions running.</p>`;
  return html`<ul class="sessions">
    ${sessions.map((session) => html`<li key=${session.session_id}><${Row} session=${session} /></li>`)}
  </ul>`;
}

/** @param {{ session: Session }} props */
function Row({ session }) {
  return html`<a class="session" href=${`#/session/${encodeURIComponent(session.session_id)}`}>
    <span class="name">${folderName(session.cwd)}</span>
    <span class="dim path">${session.cwd}</span>
    <span class="dim">${session.session_id.slice(0, 8)} · Luxe ${session.luxe_version}</span>
  </a>`;
}

/** The last part of a path, from either kind of separator. @param {string} path */
export function folderName(path) {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path;
}
