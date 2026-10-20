// @ts-check
/// <reference lib="webworker" />
// The service worker: it shows the bridge's notifications, and opens the session one names. It
// has no fetch handler, so the app loads exactly as it would without one.
//
// iOS ends a subscription that gets a push without a notification to show for it, so every push
// shows one, even a push this can't read.

const worker = /** @type {ServiceWorkerGlobalScope} */ (/** @type {unknown} */ (self));
const ICON = "/icons/192.png";

/**
 * A notification as the bridge sends it.
 * @typedef {{ title: string, body: string, session: string }} Notice
 */

worker.addEventListener("push", (event) => event.waitUntil(show(read(event.data))));

worker.addEventListener("notificationclick", (event) => {
  event.notification.close();
  event.waitUntil(openSession(event.notification.data?.session));
});

/** @param {PushMessageData | null} data @returns {Notice | null} */
function read(data) {
  try {
    return data?.json() ?? null;
  } catch {
    return null;
  }
}

/** @param {Notice | null} notice */
function show(notice) {
  if (!notice?.title) {
    return worker.registration.showNotification("luxe", { body: "A session wants you", icon: ICON });
  }
  const { title, body, session } = notice;
  return worker.registration.showNotification(title, { body, icon: ICON, data: { session } });
}

/**
 * Brings the app forward on `session`: the open window if there is one, a new one if not.
 * @param {string | undefined} session
 */
async function openSession(session) {
  const windows = await worker.clients.matchAll({ type: "window", includeUncontrolled: true });
  const open = windows[0];
  if (!open) {
    await worker.clients.openWindow(session ? `/#/session/${encodeURIComponent(session)}` : "/");
    return;
  }
  await open.focus();
  if (session) open.postMessage({ kind: "open_session", session });
}
