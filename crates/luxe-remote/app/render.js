// @ts-check
// Text as the host renders it, through `/api/render`: the app parses no markdown itself.
//
// Everything asked for in the same moment goes in one request, so a transcript loading a hundred
// messages costs one round trip, not a hundred. A finished piece is kept, so scrolling and
// reconnecting don't ask again; a growing reply isn't, since each version is seen once.

import { useEffect, useRef, useState } from "preact/hooks";

/**
 * @typedef {{ kind: "answer" | "reasoning", text: string }
 *   | { kind: "user", content: { type: string, content?: string }[] }
 *   | { kind: "tool", name: string }
 *   | { kind: "status", [fact: string]: any }
 *   | { kind: "plan", todos: any } | { kind: "changes", changes: any }
 *   | { kind: "diff", diff: any } | { kind: "diff_failed", error: any }
 *   | { kind: "file", file: any } | { kind: "file_failed", error: any }
 *   | { kind: "tree", rows: import("./files.js").TreeRow[], changes: any }
 *   | { kind: "github_list", repository: string | null, section: any, now: number }
 *   | { kind: "github_header" | "github_overview", overview: any, now: number | undefined }
 *   | { kind: "github_conversation", entries: any, now: number | undefined, opened_by_you: boolean }
 *   | { kind: "project_name", name: string, branch: string | null, dirty: boolean }
 *   | { kind: "mode_name", mode: string }
 *   | { kind: "review_guide", review: any, reading: any[], walked: string[] }
 *   | { kind: "review_chapter", chapter: any, files: any[], place: { number: number, total: number, walked: boolean }, reading: any,
 *       findings: any[] }} Piece
 * @typedef {(pieces: Piece[]) => Promise<string[]>} Post
 * @typedef {{
 *   piece: Piece, key: string, keep: boolean,
 *   resolve: (html: string) => void, reject: (error: unknown) => void,
 * }} Asked
 * @typedef {{ post: Post, kept: Map<string, string>, queue: Asked[] }} State
 */

/** The most finished pieces kept; the oldest goes first. */
const KEPT = 2000;

/** How long a growing reply waits between renders, at least. */
const GROWING_PAUSE_MS = 150;

/** A renderer that sends its batches with `post`. */
export function createRenderer(post = postPieces) {
  /** @type {State} */
  const state = { post, kept: new Map(), queue: [] };
  return {
    /** The HTML for `piece` if it's been kept. @param {Piece} piece */
    peek: (piece) => state.kept.get(keyOf(piece)),
    /** @param {Piece} piece @param {{ keep: boolean }} options @returns {Promise<string>} */
    render: (piece, { keep }) => ask(state, piece, keep),
  };
}

/** @param {State} state @param {Piece} piece @param {boolean} keep @returns {Promise<string>} */
function ask(state, piece, keep) {
  const key = keyOf(piece);
  const known = state.kept.get(key);
  if (known !== undefined) return Promise.resolve(known);
  if (!state.queue.length) queueMicrotask(() => flush(state));
  return new Promise((resolve, reject) => state.queue.push({ piece, key, keep, resolve, reject }));
}

/** @param {State} state */
function flush(state) {
  const batch = state.queue;
  state.queue = [];
  sendBatch(state.post, batch, state.kept);
}

/** @param {Post} post @param {Asked[]} batch @param {Map<string, string>} kept */
function sendBatch(post, batch, kept) {
  post(batch.map((asked) => asked.piece)).then(
    (html) => batch.forEach((asked, index) => {
      if (asked.keep) keep(kept, asked.key, html[index]);
      asked.resolve(html[index]);
    }),
    (error) => batch.forEach((asked) => asked.reject(error)),
  );
}

/** @param {Map<string, string>} kept @param {string} key @param {string} html */
function keep(kept, key, html) {
  kept.set(key, html);
  const oldest = kept.keys().next().value;
  if (kept.size > KEPT && oldest !== undefined) kept.delete(oldest);
}

/** @param {Piece} piece */
export function keyOf(piece) {
  return JSON.stringify(piece);
}

/** @type {Post} */
async function postPieces(pieces) {
  const response = await fetch("/api/render", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ pieces }),
  });
  if (!response.ok) throw new Error(`rendering failed: ${response.status}`);
  return (await response.json()).html;
}

/** The app's one renderer, so everything on screen batches together. */
export const renderer = createRenderer();

/**
 * `piece` as HTML, or undefined until the host answers. While `growing`, a newer version waits
 * for the one in flight, so a streaming reply is rendered as fast as the round trip allows and
 * no faster; the last HTML stays up meanwhile.
 * @param {Piece} piece @param {boolean} growing
 */
export function useRendered(piece, growing) {
  const [shown, setShown] = useState(() => renderer.peek(piece));
  const wanted = useRef(piece);
  const asking = useRef(false);
  wanted.current = piece;
  const key = keyOf(piece);
  useEffect(() => {
    if (asking.current) return;
    asking.current = true;
    keepUp(wanted, growing, setShown).finally(() => (asking.current = false));
  }, [key]);
  return shown;
}

/**
 * Renders the wanted piece until what was rendered is still what's wanted.
 * @param {{ current: Piece }} wanted @param {boolean} growing @param {(html: string) => void} setShown
 */
async function keepUp(wanted, growing, setShown) {
  for (;;) {
    const piece = wanted.current;
    try {
      setShown(await renderer.render(piece, { keep: !growing }));
    } catch (error) {
      console.warn(error);
      return;
    }
    // by key: a parent drawing again hands over an equal piece that is a new object
    if (keyOf(wanted.current) === keyOf(piece)) return;
    await new Promise((resolve) => setTimeout(resolve, GROWING_PAUSE_MS));
  }
}
