import { test } from "node:test";
import assert from "node:assert/strict";

/** A WebSocket that only records: the test plays the browser's part. */
class FakeSocket {
  static OPEN = 1;
  static made = [];
  constructor(url) {
    this.url = url;
    this.closedWith = null;
    FakeSocket.made.push(this);
  }
  close(code) {
    this.closedWith = code;
  }
}

const page = /** @type {EventTarget & { visibilityState: string }} */ (Object.assign(new EventTarget(), { visibilityState: "visible" }));
Object.assign(globalThis, { document: page, WebSocket: FakeSocket, location: { protocol: "http:", host: "bridge" } });
const { connect } = await import("../app/connection.js");

function show(state) {
  page.visibilityState = state;
  page.dispatchEvent(new Event("visibilitychange"));
}

function joined() {
  FakeSocket.made = [];
  const statuses = [];
  const link = connect("s-1", { onFrame: () => {}, onStatus: (status) => statuses.push(status) });
  return { link, statuses };
}

// The break this catches: a socket held open while the app is hidden, so the bridge thinks you're
// looking and never notifies you, or a hidden app's close taken for the session ending.
test("the socket closes while the app is hidden and opens again on return", () => {
  const { link, statuses } = joined();
  const first = FakeSocket.made[0];
  first.onopen();
  show("hidden");
  assert.equal(first.closedWith, 1000);
  first.onclose?.({ code: 1000 });
  assert.ok(!statuses.includes("ended"), `${statuses}`);
  show("visible");
  assert.equal(FakeSocket.made.length, 2, "a new socket on return");
  assert.deepEqual(statuses, ["connecting", "live", "connecting"]);
  link.close();
});

// The break this catches: a session opened while the app is out of sight (a retry coming due, or
// a load in the background), holding a socket open with nobody looking.
test("nothing opens while the app is hidden", () => {
  show("hidden");
  const { link } = joined();
  assert.equal(FakeSocket.made.length, 0);
  show("visible");
  assert.equal(FakeSocket.made.length, 1);
  link.close();
});
