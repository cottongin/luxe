// @ts-check
// Where you write to the agent. Idle, Send starts a turn; while one runs, what you send steers it
// between tool calls, as typing into the TUI's composer does, and Stop ends it. Steering carries
// only words, so a message with photos waits for the turn to end and starts the next one.
//
// Return is a new line, since a phone's keyboard has no other way to make one; Command- or
// Control-Return sends from a hardware keyboard. The line beside the buttons says what sending
// does while a turn runs, since the buttons are icons. A message that's one of the slash commands
// the phone can carry out does that instead of going to the agent.
//
// One panel, as the TUI's composer is one box: the box takes the whole width, with the buttons
// along its foot, since a narrow phone (or a closed iPhone Duo beside its side bar) has no room
// for both in a row.

import { useEffect, useLayoutEffect, useRef, useState } from "preact/hooks";
import { withMention } from "./files.js";
import { fieldOf, html } from "./html.js";
import { Icon } from "./icons.js";
import { messageFrame } from "./pending.js";
import { forTheWire, usePhotos } from "./photos.js";
import { Picker } from "./picker.js";

/** @import { Frame } from "./connection.js" */
/** @import { Draft, Listings } from "./picker.js" */
/** @typedef {ReturnType<typeof usePhotos>} Photos */

/** @type {Draft} */
const EMPTY = { text: "", caret: 0 };

/** The rows the box grows to before it scrolls. */
const MOST_ROWS = 8;

/**
 * `run` carries out a slash command: null when the text isn't one, else whether it went out.
 * `mention` is what a mention from elsewhere (the project panel) adds to the draft, added and
 * then `taken`.
 * @param {{ busy: boolean, live: boolean, steering: number, listings: Listings,
 *   send: (frame: Frame) => boolean, run: (text: string) => boolean | null, onSteered: () => void,
 *   onDropped: () => void, mention: string | null, taken: () => void }} props
 */
export function Composer({ busy, live, steering, listings, send, run, onSteered, onDropped, mention, taken }) {
  const [draft, setDraft] = useState(EMPTY);
  useMentioned(mention, setDraft, taken);
  const photos = usePhotos();
  const clear = () => setDraft(EMPTY);
  const submit = useSubmit({ text: draft.text, photos, busy, send, run, onSent: clear, onSteered });
  const withPhotos = photos.list.length > 0;
  const ready = live && (draft.text.trim() !== "" || withPhotos);
  return html`<form class="composer" onSubmit=${submit}>
    <${Picker} draft=${draft} listings=${listings} send=${send} place=${setDraft} run=${(/** @type {string} */ text) => run(text) && clear()} />
    <div class="composer-panel">
      <${PhotoStrip} photos=${photos} />
      <${MessageBox} draft=${draft} setDraft=${setDraft} busy=${busy} live=${live} submit=${submit} />
      <${Tools} live=${live} busy=${busy} ready=${ready} steering=${steering} photos=${photos} send=${send} onDropped=${onDropped} />
    </div>
  </form>`;
}

/**
 * The panel's foot: a photo, the steering line, then Stop and Send.
 * @param {{ live: boolean, busy: boolean, ready: boolean, steering: number, photos: Photos,
 *   send: (frame: Frame) => boolean, onDropped: () => void }} props
 */
function Tools({ live, busy, ready, steering, photos, send, onDropped }) {
  const withPhotos = photos.list.length > 0;
  return html`<div class="composer-tools">
    <${Attach} live=${live} onPick=${photos.add} />
    <${Steering} line=${steeringLine(busy, steering, withPhotos)} count=${steering}
      withdraw=${() => send({ kind: "withdraw_steering" }) && onDropped()} />
    <${Actions} label=${sendLabel(busy, withPhotos)} busy=${busy} ready=${ready} stop=${() => send({ kind: "cancel" }) && onDropped()} />
  </div>`;
}

/**
 * The box you write in, growing with what's written; Command- or Control-Return sends. It keeps
 * where the caret is, which is what an `@` mention is read back from.
 * @param {{ draft: Draft, setDraft: (draft: Draft) => void, busy: boolean, live: boolean,
 *   submit: (event: Event) => void }} props
 */
function MessageBox({ draft, setDraft, busy, live, submit }) {
  const box = useRef(/** @type {HTMLTextAreaElement | null} */ (null));
  useGrowing(box, draft.text);
  usePlacedCaret(box, draft);
  /** @param {KeyboardEvent} event */
  const onKey = (event) => event.key === "Enter" && (event.metaKey || event.ctrlKey) && submit(event);
  /** @param {Event} event */
  const onInput = (event) => {
    const field = fieldOf(event);
    setDraft({ text: field.value, caret: field.selectionStart ?? field.value.length });
  };
  return html`<textarea ref=${box} rows="2" value=${draft.text} disabled=${!live} enterkeyhint="enter"
    placeholder=${busy ? "Steer the agent" : "Message the agent"} aria-label="Message"
    onInput=${onInput} onKeyDown=${onKey} />`;
}

/**
 * Adds a mention made outside the composer to the end of the draft.
 * @param {string | null} mention @param {(change: (draft: Draft) => Draft) => void} setDraft
 * @param {() => void} taken
 */
function useMentioned(mention, setDraft, taken) {
  useEffect(() => {
    if (mention === null) return;
    setDraft((draft) => withMention(draft.text, mention));
    taken();
  }, [mention]);
}

/**
 * Puts the caret after what a pick inserted; typing leaves it where the keyboard put it.
 * @param {{ current: HTMLTextAreaElement | null }} box @param {Draft} draft
 */
function usePlacedCaret(box, draft) {
  useLayoutEffect(() => {
    if (draft.placed) box.current?.setSelectionRange(draft.caret, draft.caret);
  }, [draft]);
}

/**
 * Sends what's written and attached, clearing both only once it went out. Words alone that make
 * one of the phone's commands carry it out instead.
 * @param {{ text: string, photos: Photos, busy: boolean, send: (frame: Frame) => boolean,
 *   run: (text: string) => boolean | null, onSent: () => void, onSteered: () => void }} props
 */
function useSubmit({ text, photos, busy, send, run, onSent, onSteered }) {
  return (/** @type {Event | undefined} */ event) => {
    event?.preventDefault();
    const attached = photos.list;
    if (!text.trim() && !attached.length) return;
    const ran = attached.length ? null : run(text);
    if (ran !== null) return void (ran && onSent());
    if (!send(messageFrame(text, busy, forTheWire(attached)))) return;
    onSent();
    photos.clear();
    if (busy && !attached.length) onSteered();
  };
}

/** What the send button says to a screen reader. @param {boolean} busy @param {boolean} withPhotos */
export function sendLabel(busy, withPhotos) {
  if (!busy) return "Send";
  return withPhotos ? "Queue" : "Steer";
}

/**
 * The line over the box: steering this device sent that hasn't reached the agent yet, or else
 * what sending does while a turn runs. Empty when there's nothing to say.
 * @param {boolean} busy @param {number} count @param {boolean} withPhotos
 */
export function steeringLine(busy, count, withPhotos) {
  if (count) return count === 1 ? "1 message waiting to steer" : `${count} messages waiting to steer`;
  if (!busy) return "";
  return withPhotos ? "Photos wait for the turn" : "Send steers the turn";
}

/** @param {{ live: boolean, onPick: (files: FileList) => void }} props */
function Attach({ live, onPick }) {
  /** @param {Event} event */
  const picked = (event) => {
    const field = fieldOf(event);
    if (field.files) onPick(field.files);
    field.value = "";
  };
  return html`<label class=${`attach ${live ? "" : "off"}`}><${Icon} name="attach" />
    <input type="file" accept="image/*" multiple aria-label="Add a photo" disabled=${!live} onChange=${picked} /></label>`;
}

/** @param {{ label: string, busy: boolean, ready: boolean, stop: () => void }} props */
function Actions({ label, busy, ready, stop }) {
  return html`<div class="composer-actions">
    ${busy ? html`<button type="button" class="stop" aria-label="Stop" onClick=${stop}><${Icon} name="stop" /></button>` : null}
    <button type="submit" class="send" aria-label=${label} disabled=${!ready}><${Icon} name="send" /></button>
  </div>`;
}

/** The photos attached so far, each with a way to take it off. @param {{ photos: Photos }} props */
function PhotoStrip({ photos }) {
  if (!photos.list.length && !photos.problem) return null;
  return html`<div class="photos">
    ${photos.problem ? html`<p class="problem">${photos.problem}</p>` : null}
    ${photos.list.map((photo, index) => html`<figure class="photo" key=${photo.preview.slice(-32)}>
      <img src=${photo.preview} alt=${photo.name ?? "photo"} />
      <button type="button" class="unattach" aria-label="Remove photo" onClick=${() => photos.remove(index)}><${Icon} name="close" /></button>
    </figure>`)}
  </div>`;
}

/**
 * The steering line, with a way to take back what's waiting.
 * @param {{ line: string, count: number, withdraw: () => void }} props
 */
function Steering({ line, count, withdraw }) {
  if (!line) return null;
  return html`<p class="steering">${line}
    ${count ? html`<button type="button" class="withdraw" onClick=${withdraw}>withdraw</button>` : null}</p>`;
}

/**
 * The box grows with what's written, up to MOST_ROWS, then scrolls.
 * @param {{ current: HTMLTextAreaElement | null }} box @param {string} text
 */
function useGrowing(box, text) {
  useLayoutEffect(() => {
    const element = box.current;
    if (!element) return;
    element.style.height = "auto";
    const most = parseFloat(getComputedStyle(element).lineHeight) * MOST_ROWS;
    element.style.height = `${Math.min(element.scrollHeight, most)}px`;
  }, [text]);
}
