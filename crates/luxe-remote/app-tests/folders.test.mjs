// Starting a session from the phone.

import assert from "node:assert/strict";
import { test } from "node:test";
import { matchFolders, startIn } from "../app/folders.js";

const folder = (path) => ({ path, name: path.split("/").pop(), sessions: 0 });
const WORKED_IN = [
  folder("/home/me/code/notes-app"),
  folder("/home/me/code/app"),
  folder("/home/me/apparel/site"),
  folder("/home/me/docs"),
];
const names = (matched) => matched.map((entry) => entry.name);

/** A fetch that records what it was asked and answers `status` with `body`. */
function answering(status, body) {
  const asked = [];
  const post = async (url, init) => {
    asked.push({ url, init });
    const text = typeof body === "string" ? body : JSON.stringify(body);
    return new Response(status === 204 ? null : text, { status });
  };
  return { asked, post };
}

// The break this catches: the request the bridge's start route doesn't take (another path, a GET,
// or the folder under another name), so every tap on a folder is refused
test("a start posts the folder and opens the session the bridge started", async () => {
  const started = { session: { session_id: "abc-123", cwd: "/home/me/app" }, place: "herdr" };
  const { asked, post } = answering(201, started);
  assert.equal(await startIn("~/app", post), "abc-123");
  assert.equal(asked[0].url, "/api/sessions");
  assert.equal(asked[0].init.method, "POST");
  assert.deepEqual(JSON.parse(asked[0].init.body), { folder: "~/app" });
});

// The break this catches: a refused start taken for a session, or shown without the bridge's
// reason, so "isn't a folder on this machine" never reaches the reader
test("a refused start says why, in the bridge's words", async () => {
  const { post } = answering(400, "/nope isn't a folder on this machine");
  await assert.rejects(startIn("/nope", post), { message: "/nope isn't a folder on this machine" });
});

// The break this catches: an empty field showing nothing, so the folders you've worked in only
// appear once you start typing
test("an empty field offers every folder, in the bridge's order", () => {
  assert.deepEqual(matchFolders(WORKED_IN, "  "), WORKED_IN);
});

// The break this catches: the ranks collapsed or reordered, so the folder named for what you typed
// sits under ones that only hold it somewhere
test("a name starting with what's typed leads, then a name holding it, then a path", () => {
  assert.deepEqual(names(matchFolders(WORKED_IN, "app")), ["app", "notes-app", "site"]);
});

// The break this catches: a folder that holds the text nowhere offered anyway
test("a folder that doesn't hold what's typed is left out", () => {
  assert.deepEqual(names(matchFolders(WORKED_IN, "docs")), ["docs"]);
});

// The break this catches: case counted, so "App" finds nothing on a phone that capitalizes
test("case doesn't matter", () => {
  assert.deepEqual(names(matchFolders(WORKED_IN, "DOCS")), ["docs"]);
});

// The break this catches: the `~/` kept, which no absolute path holds, so typing a path the way
// you'd type it in a terminal finds nothing
test("a path typed from home still finds the folder under it", () => {
  assert.deepEqual(names(matchFolders(WORKED_IN, "~/code/app")), ["app"]);
});

// The break this catches: a refusal with no words read as an empty message, which shows nothing
test("a refusal with no reason still says the start failed", async () => {
  const { post } = answering(504, "");
  await assert.rejects(startIn("/slow", post), { message: "starting failed: 504" });
});
