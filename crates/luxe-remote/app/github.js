// @ts-check
// The GitHub tab's state: the sections `list_github` brings, which one is shown, and the one pull
// request or issue open, its overview and conversation read side by side. Everything here reads;
// a joined session runs with GitHub writes off, so nothing offers one.
//
// An answer is matched to what was asked by its number and repository, so a late answer for an
// item already closed is dropped. Each answer keeps the moment it came, the clock its ages are
// drawn from, so a redraw asks the host for the same piece and gets it from the cache.

import { signal } from "@preact/signals";

/** @import { Frame } from "./connection.js" */
/**
 * @typedef {{ number: number, pull_request: boolean, repository: string | null, url: string }} Subject
 * @typedef {{ state: "asking" | "read" | "failed" | "lost", value?: any, error?: string, at?: number }} Reading
 * @typedef {{ subject: Subject, tab: DetailTab, overview: Reading, conversation: Reading }} OpenItem
 * @typedef {"overview" | "conversation"} DetailTab
 * @typedef {{ state: "idle" | "asking" | "listed" | "lost", repository: string | null, sections: any[], at: number }} GithubListing
 */

/** Nothing listed and nothing open. */
export function createGithub() {
  const listing = signal(/** @type {GithubListing} */ ({ state: "idle", repository: null, sections: [], at: 0 }));
  const section = signal(/** @type {string | null} */ (null));
  const open = signal(/** @type {OpenItem | null} */ (null));
  /** Asks for every section afresh, keeping the last rows up meanwhile. @param {(frame: Frame) => boolean} send */
  const list = (send) => (listing.value = { ...listing.value, state: send({ kind: "list_github", content: {} }) ? "asking" : "lost" });
  /** Opens `subject`, asking for its overview and its conversation. @param {Subject} subject @param {(frame: Frame) => boolean} send */
  const read = (subject, send) => (open.value = opened(subject, send));
  /** @param {Frame} frame */
  const apply = (frame) => {
    listing.value = applyListed(listing.value, frame);
    open.value = applyItem(open.value, frame);
  };
  /** @param {DetailTab} tab */
  const show = (tab) => open.value && (open.value = { ...open.value, tab });
  return { listing, section, open, list, read, apply, show, pick: (/** @type {string} */ name) => (section.value = name), close: () => (open.value = null) };
}

/** Seconds since the Unix epoch, the clock the host draws ages from. */
const now = () => Math.floor(Date.now() / 1000);

/** @param {Subject} subject @param {(frame: Frame) => boolean} send @returns {OpenItem} */
function opened(subject, send) {
  const content = { number: subject.number, pull_request: subject.pull_request, repository: subject.repository };
  const sent = send({ kind: "read_github_overview", content }) && send({ kind: "read_github_conversation", content });
  /** @type {Reading} */
  const reading = { state: sent ? "asking" : "lost" };
  return { subject, tab: "overview", overview: reading, conversation: reading };
}

/**
 * The listing after `frame`: listed, or lost when the connection began again before it was.
 * @param {GithubListing} listing @param {Frame} frame @returns {GithubListing}
 */
export function applyListed(listing, { kind, content }) {
  if (listing.state !== "asking") return listing;
  if (kind === "ready") return { ...listing, state: "lost" };
  if (kind !== "github_listed") return listing;
  return { state: "listed", repository: content.repository ?? null, sections: content.sections ?? [], at: now() };
}

/**
 * The open item after `frame`: each half read, failed, or lost when the connection began again.
 * @param {OpenItem | null} item @param {Frame} frame @returns {OpenItem | null}
 */
export function applyItem(item, { kind, content }) {
  if (!item) return item;
  if (kind === "ready") return { ...item, overview: lost(item.overview), conversation: lost(item.conversation) };
  if (!answers(item.subject, content)) return item;
  if (kind === "github_overview_read" && item.overview.state === "asking") {
    return { ...item, overview: answered(content.overview, content.error) };
  }
  if (kind === "github_conversation_read" && item.conversation.state === "asking") {
    return { ...item, conversation: answered(content.entries, content.error) };
  }
  return item;
}

/** @param {Reading} reading @returns {Reading} */
const lost = (reading) => (reading.state === "asking" ? { state: "lost" } : reading);

/** @param {any} value @param {string | null | undefined} error @returns {Reading} */
const answered = (value, error) => (error ? { state: "failed", error, at: now() } : { state: "read", value, at: now() });

/** Whether an answer is for `subject`: its number, and its repository when one was named. @param {Subject} subject @param {any} content */
function answers(subject, content) {
  return content?.number === subject.number && (content.repository ?? null) === subject.repository;
}

/**
 * Which section the tab shows: the one picked while it's still there, else the first with rows,
 * as the TUI opens on the first tab with something in it.
 * @param {any[]} sections @param {string | null} picked
 */
export function shownSection(sections, picked) {
  return sections.find((section) => section.name === picked) ?? sections.find((section) => section.items?.length) ?? sections[0];
}

/**
 * A tapped row's subject, from the data the host drew on it. The checkout's own repository is
 * left unnamed, as the TUI asks for it. @param {HTMLElement} row @param {string | null} here
 * @returns {Subject}
 */
export function rowSubject(row, here) {
  const repository = row.dataset.repository || null;
  return {
    number: Number(row.dataset.number),
    pull_request: row.dataset.pull === "true",
    repository: repository === here ? null : repository,
    url: row.dataset.url ?? "",
  };
}

/** What a mention of `subject` adds to the draft: what it is and where, which the agent can read. @param {Subject} subject @param {string | null} here */
export function githubMention({ number, pull_request, repository, url }, here) {
  const noun = pull_request ? "pull request" : "issue";
  return `${noun} ${repository ?? here ?? ""}#${number} (${url})`;
}
