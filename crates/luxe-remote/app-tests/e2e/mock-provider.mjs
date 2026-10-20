// A model that follows a script, served the way an OpenAI-compatible endpoint streams: asked to
// act, it calls `write`; given the tool's result, it says what it did. A request with no tools
// (a session title, say) gets a word back. Every request is kept for the test to look at.

import { createServer } from "node:http";

/** The file the scripted model writes, and what it writes in it. */
export const WRITTEN = { path: "hello.txt", content: "hello from the mock\n" };

/** Starts the server on a free port. */
export async function startMockProvider() {
  const requests = [];
  const server = createServer((request, response) => answer(request, response, requests));
  await new Promise((resolve) => server.listen(0, "127.0.0.1", () => resolve(undefined)));
  const { port } = /** @type {import("node:net").AddressInfo} */ (server.address());
  return { port, requests, close: () => new Promise((resolve) => server.close(resolve)) };
}

async function answer(request, response, requests) {
  if (request.method !== "POST" || !request.url?.endsWith("/chat/completions")) {
    response.writeHead(404).end();
    return;
  }
  const body = JSON.parse(await readBody(request));
  requests.push(body);
  response.writeHead(200, { "content-type": "text/event-stream" });
  for (const chunk of reply(body)) response.write(`data: ${JSON.stringify(chunk)}\n\n`);
  response.end("data: [DONE]\n\n");
}

async function readBody(request) {
  let text = "";
  for await (const part of request) text += part;
  return text;
}

function reply(body) {
  if (!body.tools?.length) return words("Mock");
  if (body.messages.at(-1)?.role === "tool") return words(`Wrote **${WRITTEN.path}**.`);
  return writeCall();
}

const chunk = (delta, finish = null) => ({
  id: "mock", object: "chat.completion.chunk", created: 0, model: "scripted",
  choices: [{ index: 0, delta, finish_reason: finish }],
});

const usage = { id: "mock", object: "chat.completion.chunk", created: 0, model: "scripted", choices: [],
  usage: { prompt_tokens: 1200, completion_tokens: 20, total_tokens: 1220 } };

function words(text) {
  const half = Math.ceil(text.length / 2);
  return [chunk({ role: "assistant", content: text.slice(0, half) }), chunk({ content: text.slice(half) }),
    chunk({}, "stop"), usage];
}

function writeCall() {
  const call = { index: 0, id: "call_write", type: "function", function: { name: "write", arguments: "" } };
  const args = { index: 0, function: { arguments: JSON.stringify(WRITTEN) } };
  return [chunk({ role: "assistant", content: null, tool_calls: [call] }), chunk({ tool_calls: [args] }),
    chunk({}, "tool_calls"), usage];
}
