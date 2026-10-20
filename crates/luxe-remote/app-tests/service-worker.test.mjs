import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";

const source = readFileSync(new URL("../app/worker/service-worker.js", import.meta.url), "utf8");

/** Runs the worker against a stand-in for the browser, with `windows` open. */
function worker(windows = []) {
  const listeners = {};
  const shown = [];
  const opened = [];
  const self = {
    addEventListener: (type, listener) => (listeners[type] = listener),
    registration: { showNotification: async (title, options) => shown.push({ title, options }) },
    clients: { matchAll: async () => windows, openWindow: async (url) => opened.push(url) },
  };
  vm.runInNewContext(source, { self });
  const fire = async (type, event) => {
    let pending;
    listeners[type]({ ...event, waitUntil: (promise) => (pending = promise) });
    await pending;
  };
  return { fire, shown, opened };
}

// objects made in the worker's context have its own Object, which strict equality tells apart
const plain = (value) => JSON.parse(JSON.stringify(value));

const push = (payload) => ({ data: { json: () => (typeof payload === "string" ? JSON.parse(payload) : payload) } });

// The break this catches: a notice shown without its folder or what's waiting, or with the
// session it should open lost.
test("a push shows what the bridge sent", async () => {
  const { fire, shown } = worker();
  await fire("push", push({ title: "luxe-dev", body: "Approve bash", session: "s-1" }));
  const options = { body: "Approve bash", icon: "/icons/192.png", data: { session: "s-1" } };
  assert.deepEqual(plain(shown), [{ title: "luxe-dev", options }]);
});

// The break this catches: a push that shows nothing, which iOS answers by ending the subscription.
test("a push it can't read still shows a notification", async () => {
  const { fire, shown } = worker();
  await fire("push", push("not json"));
  await fire("push", { data: null });
  assert.deepEqual(shown.map((notice) => notice.title), ["luxe", "luxe"]);
});

const click = (session) => {
  const closed = [];
  return { closed, event: { notification: { data: { session }, close: () => closed.push(true) } } };
};

// The break this catches: a tap that opens the app's list instead of the session, or opens nothing
// when the app isn't running.
test("a tap with the app closed opens it on the session", async () => {
  const { fire, opened } = worker();
  const { closed, event } = click("s 1");
  await fire("notificationclick", event);
  assert.deepEqual(opened, ["/#/session/s%201"]);
  assert.deepEqual(closed, [true]);
});

// The break this catches: a second copy of the app opened beside the first, or the open one
// brought forward without being told which session to show.
test("a tap with the app open brings it forward on the session", async () => {
  const told = [];
  const open = { focus: async () => told.push("focus"), postMessage: (message) => told.push(message) };
  const { fire, opened } = worker([open]);
  await fire("notificationclick", click("s-1").event);
  assert.deepEqual(plain(told), ["focus", { kind: "open_session", session: "s-1" }]);
  assert.deepEqual(opened, []);
});
