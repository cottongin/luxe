// @ts-check
// Notifications on this device: the switch, and what it takes to turn them on.
//
// iOS gives Web Push only to an app on the Home Screen, and asks permission only in answer to a
// tap, so the switch subscribes inside its tap handler, and a Safari tab shows the way to the
// Home Screen instead. Notifications say what's waiting and where, never what: the app is where
// you read and answer.

import { signal } from "@preact/signals";
import { html } from "./html.js";

/**
 * What the switch can do here.
 * @typedef {"unsupported" | "home-screen" | "ready"} Support
 * @typedef {"unsupported" | "home-screen" | "off" | "on" | "blocked" | "busy"} PushState
 * @typedef {{ worker: boolean, push: boolean, standalone: boolean }} Browser
 */

/** @type {import("@preact/signals").Signal<PushState>} */
const state = signal("unsupported");
const key = signal("");
const problem = signal("");

/**
 * Whether this browser can take notifications. Of the browsers people use, only iOS's lack push
 * in a tab, and they have it once the app is on the Home Screen. @param {Browser} browser
 * @returns {Support}
 */
export function support(browser) {
  if (browser.worker && browser.push) return "ready";
  return browser.standalone ? "unsupported" : "home-screen";
}

/** @returns {Browser} */
function thisBrowser() {
  const standalone = matchMedia("(display-mode: standalone)").matches;
  return { worker: "serviceWorker" in navigator, push: "PushManager" in window, standalone };
}

/** The bridge's key as `PushManager.subscribe` takes it. @param {string} base64url */
export function keyBytes(base64url) {
  const base64 = base64url.replaceAll("-", "+").replaceAll("_", "/");
  return Uint8Array.from(atob(base64), (char) => char.charCodeAt(0));
}

/** Starts the service worker, which shows what arrives and opens the session one names. */
export function startWorker() {
  if (support(thisBrowser()) !== "ready") return;
  navigator.serviceWorker.register("/service-worker.js").catch((error) => {
    console.warn("the service worker didn't start", error);
  });
  navigator.serviceWorker.addEventListener("message", (event) => {
    if (event.data?.kind !== "open_session") return;
    location.hash = `#/session/${encodeURIComponent(event.data.session)}`;
  });
}

/** Reads where the switch stands: what the browser allows, and what the bridge has. */
export async function refreshNotifications() {
  const allowed = support(thisBrowser());
  if (allowed !== "ready") return void (state.value = allowed);
  if (Notification.permission === "denied") return void (state.value = "blocked");
  const response = await fetch("/api/push");
  if (!response.ok) return;
  const status = await response.json();
  key.value = status.key;
  const here = await (await navigator.serviceWorker.ready).pushManager.getSubscription();
  // the bridge's record alone isn't enough: a reinstalled app has lost its subscription
  if (state.value !== "busy") state.value = status.subscribed && here ? "on" : "off";
}

/** Asks for permission and subscribes; the switch's tap handler. */
async function turnOn() {
  const permission = await Notification.requestPermission();
  if (permission !== "granted") return void (state.value = permission === "denied" ? "blocked" : "off");
  const manager = (await navigator.serviceWorker.ready).pushManager;
  // one made with a key the bridge no longer has can't be reused, so start fresh
  await (await manager.getSubscription())?.unsubscribe();
  const subscription = await manager.subscribe({ userVisibleOnly: true, applicationServerKey: keyBytes(key.value) });
  await send("PUT", JSON.stringify(subscription));
  state.value = "on";
}

async function turnOff() {
  await send("DELETE");
  const manager = (await navigator.serviceWorker.ready).pushManager;
  await (await manager.getSubscription())?.unsubscribe();
  state.value = "off";
}

/** @param {string} method @param {string} [body] */
async function send(method, body) {
  const headers = body ? { "content-type": "application/json" } : undefined;
  const response = await fetch("/api/push", { method, headers, body });
  if (!response.ok) throw new Error((await response.text()) || `the bridge said ${response.status}`);
}

/** Runs `change` with the switch busy, putting back what was there if it fails. @param {() => Promise<void>} change */
async function flip(change) {
  const before = state.value;
  state.value = "busy";
  problem.value = "";
  try {
    await change();
  } catch (error) {
    state.value = before;
    problem.value = `couldn't change notifications: ${/** @type {Error} */ (error).message}`;
  }
}

export function NotificationSwitch() {
  return html`<section class="notifications">
    <h2>Notifications</h2>
    ${problem.value ? html`<p class="problem">${problem.value}</p>` : null}
    <${Choice} state=${state.value} />
  </section>`;
}

/** @param {{ state: PushState }} props */
function Choice({ state }) {
  switch (state) {
    case "unsupported": return html`<p class="dim">Notifications aren't supported in this browser.</p>`;
    case "home-screen": return html`<p class="dim">Add Luxe to your Home Screen to enable notifications.</p>`;
    case "blocked": return html`<p class="dim">Allow notifications for Luxe in this device's settings.</p>`;
    case "on": return html`<p class="dim">Notifications are on for this device.</p>
      <button type="button" class="quiet" onClick=${() => flip(turnOff)}>Turn off</button>`;
    default: return html`<p class="dim">Get notified when a session needs an answer or finishes a long turn.</p>
      <button type="button" disabled=${state === "busy"} onClick=${() => flip(turnOn)}>
        ${state === "busy" ? "Turning on…" : "Turn on"}</button>`;
  }
}
