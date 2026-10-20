// The transcript as the phone shows it, folded from a session's frames.
//
// Each item carries its own signals, so a reply streaming in re-renders one text node rather
// than the list. The fold mirrors the TUI's: deltas grow provisional items, `message_committed`
// replaces them with what was saved, tool cards follow `tool_activity_updated` by call id, and
// `session_loaded` starts over from the saved history.

import { batch, signal } from "@preact/signals";

/** @import { Signal } from "@preact/signals" */
/** @import { Frame } from "./connection.js" */
/**
 * @typedef {{ type: string, content: any }} Block
 * @typedef {{ role: string, content: Block[] }} Message
 * @typedef {"user" | "assistant" | "thinking" | "notice" | "error"} TextKind
 * @typedef {{
 *   key: string,
 *   kind: TextKind,
 *   text: Signal<string>,
 *   content?: { type: string, content?: string }[],
 *   provisional?: boolean,
 * }} TextItem
 * @typedef {{ key: string, kind: "tool", card: Signal<Card> }} ToolItem
 * @typedef {TextItem | ToolItem} Item
 * @typedef {{
 *   items: Signal<Item[]>, busy: Signal<boolean>,
 *   cards: Map<string, ToolItem>, live: Map<string, TextItem>, serial: number,
 *   compacting?: TextItem | null,
 * }} State
 * @typedef {{
 *   call_id: string, name: string, label: string, state: string, revision: number,
 *   elapsed_ms?: number | null, summary?: string | null, output_preview?: string | null,
 *   card_body?: { text: string, total_lines: number, collapsed_lines: number } | null,
 * }} Card
 */

/** An empty transcript, and the one function that changes it. */
export function createTranscript() {
  /** @type {State} */
  const state = { items: signal([]), busy: signal(false), cards: new Map(), live: new Map(), serial: 0 };
  /** @param {Frame} frame */
  const fold = (frame) => batch(() => apply(state, frame));
  return { items: state.items, busy: state.busy, apply: fold };
}

// The engine says a turn started with turn_started; the message that opened it, or anything it
// streams, stands in for that from a session whose Luxe predates the event
const TURN_SIGNS = new Set(["turn_started", "token_delta", "thinking_delta", "tool_call_composing"]);

/** @param {State} state @param {Frame} frame */
function apply(state, { kind, content }) {
  if (TURN_SIGNS.has(kind) || (kind === "message_committed" && content.message.role === "user")) {
    state.busy.value = true;
  }
  switch (kind) {
    case "session_loaded": return load(state, content.messages);
    case "token_delta": return grow(state, "assistant", content.block, content.delta);
    case "thinking_delta": return grow(state, "thinking", content.block, content.delta);
    case "message_committed": return commit(state, content.message);
    case "tool_activity_updated": return showCard(state, content);
    case "state_changed": state.busy.value = content.streaming; return;
    case "turn_ended": return endTurn(state);
    case "notice": return push(state, textItem(state, "notice", content));
    case "error": push(state, textItem(state, "error", content)); return endTurn(state);
    case "action_rejected": return push(state, textItem(state, "error", `Refused: ${content.reason}`));
    case "compaction_started": return startCompaction(state, content.tokens_before);
    case "compaction_done": return settleCompaction(state, compacted(content));
    case "compaction_stopped": return settleCompaction(state, "Compaction stopped.");
  }
}

// A summary takes a minute and nothing else streams meanwhile, so it's said as it starts, and the
// same line becomes how it ended
/** @param {State} state @param {number} tokens */
function startCompaction(state, tokens) {
  state.compacting = textItem(state, "notice", `Compacting the conversation — ${formatTokens(tokens)} of context…`);
  push(state, state.compacting);
}

/** @param {State} state @param {string} text */
function settleCompaction(state, text) {
  const line = state.compacting;
  state.compacting = null;
  if (line && state.items.value.includes(line)) line.text.value = text;
  else push(state, textItem(state, "notice", text));
}

/** @param {{ tokens_before: number, tokens_after: number }} content */
function compacted({ tokens_before, tokens_after }) {
  return `Compacted the conversation — ${formatTokens(tokens_before)} → ${formatTokens(tokens_after)} of context.`;
}

/** A token count as the TUI writes it: `812`, `579.4k`, `1.2M`. @param {number} count */
export function formatTokens(count) {
  if (count >= 1_000_000) return `${(count / 1_000_000).toFixed(1)}M`;
  if (count >= 1_000) return `${(count / 1_000).toFixed(1)}k`;
  return String(count);
}

/** @param {State} state @param {{ message: Message }[]} messages */
function load(state, messages) {
  state.items.value = [];
  state.cards.clear();
  state.live.clear();
  for (const { message } of messages) replay(state, message);
  for (const item of state.cards.values()) settleUnfinished(item, "interrupted");
}

/** @param {State} state @param {Message} message */
function replay(state, message) {
  if (message.role === "assistant") {
    for (const item of assistantItems(state, message.content)) push(state, item);
    for (const block of message.content) if (block.type === "tool_call") cardFromCall(state, block.content);
    return;
  }
  if (message.role === "tool") return settleResults(state, message.content);
  commit(state, message);
}

/** @param {State} state @param {TextKind} kind @param {number} block @param {string} delta */
function grow(state, kind, block, delta) {
  const slot = `${kind}:${block}`;
  let item = state.live.get(slot);
  if (!item) {
    item = { ...textItem(state, kind, ""), provisional: true };
    state.live.set(slot, item);
    push(state, item);
  }
  item.text.value += delta;
}

/** @param {State} state @param {Message} message */
function commit(state, message) {
  if (message.role === "user") {
    // a photo sent on its own is still a message, shown by its "attached" line
    const words = message.content.find((block) => block.type === "text");
    const shown = words || message.content.some((block) => block.type === "image");
    if (shown) push(state, { ...textItem(state, "user", words?.content ?? ""), content: wordsOnly(message.content) });
    return;
  }
  if (message.role === "assistant") return commitAssistant(state, message.content);
  if (message.role === "tool") return settleResults(state, message.content);
  if (message.role === "system") for (const block of message.content) {
    if (block.type === "text") push(state, textItem(state, "notice", block.content));
  }
}

// Line the saved blocks up with the streamed ones in place; if they don't match, swap them out
/** @param {State} state @param {Block[]} content */
function commitAssistant(state, content) {
  const saved = assistantItems(state, content);
  const streamed = state.items.value.filter(isProvisional);
  state.live.clear();
  const linesUp = saved.length === streamed.length
    && saved.every((item, index) => item.kind === streamed[index].kind);
  if (linesUp) {
    streamed.forEach((item, index) => settleText(item, saved[index].text.value));
    return;
  }
  replaceStreamed(state, streamed, saved);
}

/** @param {Item} item @returns {item is TextItem} */
function isProvisional(item) {
  return item.kind !== "tool" && item.provisional === true;
}

/** @param {State} state @param {TextItem[]} streamed @param {TextItem[]} saved */
function replaceStreamed(state, streamed, saved) {
  const items = state.items.value;
  const at = streamed.length ? items.indexOf(streamed[0]) : items.length;
  /** @type {Item[]} */
  const kept = items.filter((item) => !isProvisional(item));
  kept.splice(Math.min(at, kept.length), 0, ...saved);
  state.items.value = kept;
}

/** @param {TextItem} item @param {string} text */
function settleText(item, text) {
  item.text.value = text;
  item.provisional = false;
}

/** @param {State} state @param {Block[]} content */
function assistantItems(state, content) {
  /** @type {TextItem[]} */
  const items = [];
  for (const block of content) {
    // Saved reasoning often ends in blank lines, which would draw as a gap
    const thinking = block.type === "thinking" ? block.content.text.trimEnd() : "";
    const text = block.type === "text" ? block.content.trimEnd() : "";
    if (thinking) items.push(textItem(state, "thinking", thinking));
    if (text) items.push(textItem(state, "assistant", text));
  }
  return items;
}

/** @param {State} state @param {Card} activity */
function showCard(state, activity) {
  const known = state.cards.get(activity.call_id);
  if (!known) {
    /** @type {ToolItem} */
    const item = { key: `tool:${activity.call_id}`, kind: "tool", card: signal(activity) };
    state.cards.set(activity.call_id, item);
    return push(state, item);
  }
  if (activity.revision >= known.card.value.revision) known.card.value = activity;
}

/** @param {State} state @param {{ id: string, name: string, arguments: any }} call */
function cardFromCall(state, { id, name, arguments: args }) {
  const label = describeCall(args);
  // A described command shows the command under it, as a live card does
  const command = typeof args?.command === "string" && args.command !== label ? args.command : "";
  const card_body = command ? { text: command, total_lines: 1, collapsed_lines: 3 } : null;
  showCard(state, { call_id: id, name, label, card_body, state: "running", revision: 0 });
}

/** @param {State} state @param {Block[]} content */
function settleResults(state, content) {
  for (const block of content) {
    if (block.type !== "tool_result") continue;
    const { call_id, content: output, is_error } = block.content;
    const item = state.cards.get(call_id);
    if (!item) continue;
    /** @type {Block[]} */
    const parts = output;
    const text = parts.filter((part) => part.type === "text").map((part) => part.content).join("\n");
    const card = item.card.value;
    item.card.value = {
      ...card,
      state: is_error ? "failed" : "succeeded",
      output_preview: card.output_preview ?? text,
      revision: card.revision + 1,
    };
  }
}

/** @param {State} state */
function endTurn(state) {
  state.live.clear();
  for (const item of state.items.value) if (item.kind !== "tool") item.provisional = false;
  state.busy.value = false;
}

/** @param {ToolItem} item @param {string} final */
function settleUnfinished(item, final) {
  const card = item.card.value;
  if (card.state === "running" || card.state === "queued" || card.state === "composing") {
    item.card.value = { ...card, state: final, revision: card.revision + 1 };
  }
}

/** What a call from history was about, from its arguments: the label live cards are sent with. */
/** @param {any} args @returns {string} */
export function describeCall(args) {
  for (const field of ["description", "command", "path", "pattern", "query", "url"]) {
    if (typeof args?.[field] === "string") return args[field];
  }
  if (Array.isArray(args?.paths)) return args.paths.join(", ");
  return "";
}

// The blocks as the host needs them to say what a message shows: text, and the place of the rest,
// so a photo's bytes aren't sent back to be rendered
/** @param {Block[]} content */
function wordsOnly(content) {
  return content.map((block) => (block.type === "text" ? block : { type: block.type }));
}

/** @param {State} state @param {TextKind} kind @param {string} text @returns {TextItem} */
function textItem(state, kind, text) {
  return { key: `${kind}:${state.serial++}`, kind, text: signal(text) };
}

/** @param {State} state @param {Item} item */
function push(state, item) {
  state.items.value = [...state.items.value, item];
}
