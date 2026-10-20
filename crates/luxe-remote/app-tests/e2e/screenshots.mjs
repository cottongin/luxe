// The phone app's screenshots for the guide's "Luxe on your phone" page, taken of the real app: a
// session in a scratch project, a bridge, and a scripted model that reads a file, edits it (which
// asks permission), runs it, and says what it did. Nothing here is drawn by hand.
//
// Needs a built `luxe` (`LUXE_BIN`, or target/debug/luxe) and Playwright's WebKit or Chromium.
// Run from app-tests: node e2e/screenshots.mjs [out-dir], by default docs/guide/phone/

import { execFileSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { createServer } from "node:http";
import { createRequire } from "node:module";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { LOGIN, startHost } from "./host.mjs";

const playwright = createRequire(import.meta.url)("playwright");
const REPO = resolve(dirname(fileURLToPath(import.meta.url)), "../../../..");
const OUT = resolve(process.argv[2] ?? join(REPO, "docs/guide/phone"));

const APP = `import argparse


def main():
    parser = argparse.ArgumentParser(description="Count the words in a file.")
    parser.add_argument("path")
    args = parser.parse_args()
    with open(args.path) as file:
        print(len(file.read().split()))


if __name__ == "__main__":
    main()
`;

const OLD = `    parser.add_argument("path")\n    args = parser.parse_args()\n    with open(args.path) as file:\n        print(len(file.read().split()))`;
const NEW = `    parser.add_argument("path")\n    parser.add_argument("--top", type=int, help="also list the most common words")\n    args = parser.parse_args()\n    with open(args.path) as file:\n        words = file.read().lower().split()\n    print(len(words))\n    for word, count in Counter(words).most_common(args.top or 0):\n        print(f"{count:>5}  {word}")`;

/** Each step of the script, picked by how many tool results the request carries. */
const STEPS = [
  { say: "I'll read the script first.", tool: "read", args: { path: "wc.py" } },
  { say: "It counts but doesn't rank. Adding `--top`:", tool: "edit", args: { path: "wc.py", edits: [{ oldText: OLD, newText: NEW }, { oldText: "import argparse\n", newText: "import argparse\nfrom collections import Counter\n" }] } },
  { say: null, tool: "bash", args: { command: "python3 wc.py README.md --top 3", description: "Try the new flag" } },
  { done: "Added **`--top N`** to `wc.py`. It still prints the total first, then the `N` most common words with their counts:\n\n```text\n12\n    2  the\n    1  port\n    1  is\n```\n\nWords are lowercased before counting, so `The` and `the` are one word. Without `--top` the output is unchanged." },
];

const chunk = (delta, finish = null) => ({ id: "shot", object: "chat.completion.chunk", created: 0, model: "scripted", choices: [{ index: 0, delta, finish_reason: finish }] });
const usage = { id: "shot", object: "chat.completion.chunk", created: 0, model: "scripted", choices: [], usage: { prompt_tokens: 18400, completion_tokens: 240, total_tokens: 18640 } };

function reply(body) {
  if (!body.tools?.length) return [chunk({ role: "assistant", content: "Rank words" }), chunk({}, "stop"), usage];
  const step = STEPS[Math.min(body.messages.filter((m) => m.role === "tool").length, STEPS.length - 1)];
  if (step.done) return [chunk({ role: "assistant", content: step.done }), chunk({}, "stop"), usage];
  const call = { index: 0, id: `call_${step.tool}`, type: "function", function: { name: step.tool, arguments: JSON.stringify(step.args) } };
  return [chunk({ role: "assistant", content: step.say, tool_calls: [call] }), chunk({}, "tool_calls"), usage];
}

async function startModel() {
  const server = createServer(async (request, response) => {
    let text = "";
    for await (const part of request) text += part;
    response.writeHead(200, { "content-type": "text/event-stream" });
    for (const piece of reply(JSON.parse(text))) response.write(`data: ${JSON.stringify(piece)}\n\n`);
    response.end("data: [DONE]\n\n");
  });
  await new Promise((done) => server.listen(0, "127.0.0.1", () => done(undefined)));
  return { port: /** @type {import("node:net").AddressInfo} */ (server.address()).port, close: () => server.close() };
}

// As flow.mjs pairs: paste the link, carry the Secure cookie over plain HTTP by hand
async function pair(page, host) {
  await page.goto(host.url);
  await page.locator(".pair-link input").fill(`https://desk.example.ts.net/pair#${host.invitation}`);
  await page.locator(".pair-link button").click();
  const answered = page.waitForResponse("**/api/pair");
  await page.locator("#pair button").click();
  const cookie = (await (await answered).headerValue("set-cookie"))?.split(";")[0] ?? "";
  await page.waitForURL((url) => url.pathname === "/", { waitUntil: "networkidle" });
  const at = cookie.indexOf("=");
  await page.context().addCookies([{ name: cookie.slice(0, at), value: cookie.slice(at + 1), url: host.url }]);
  await page.goto(host.url);
}

async function launch() {
  try {
    return await playwright.webkit.launch();
  } catch (error) {
    console.warn(`WebKit wouldn't start, using Chromium: ${String(error).split("\n")[0]}`);
    return playwright.chromium.launch();
  }
}

async function shoot(page, name) {
  await page.waitForTimeout(400);
  await page.screenshot({ path: join(OUT, `${name}.png`) });
  console.log(`wrote ${join(OUT, `${name}.png`)}`);
}

async function main() {
  mkdirSync(OUT, { recursive: true });
  const model = await startModel();
  const host = await startHost(model.port, "wordcount");
  writeFileSync(join(host.project, "wc.py"), APP);
  const git = (...args) => execFileSync("git", ["-c", "user.name=e2e", "-c", "user.email=e2e@luxe.test", ...args], { cwd: host.project });
  git("add", "wc.py");
  git("commit", "-qm", "wc");
  const browser = await launch();
  const { defaultBrowserType: _, ...phone } = playwright.devices["iPhone 15 Pro"];
  const context = await browser.newContext({ ...phone, deviceScaleFactor: 2, extraHTTPHeaders: { "Tailscale-User-Login": LOGIN } });
  const page = await context.newPage();
  try {
    await pair(page, host);
    await page.locator("a.session").click();
    await page.locator(".composer textarea").fill("Add a --top flag to wc.py that lists the most common words");
    await page.locator(".send").click();
    await page.locator(".prompt .choice.once").waitFor();
    await shoot(page, "remote-permission");
    await page.locator(".prompt .choice.once").click();
    await page.locator(".item.reply", { hasText: "Without --top" }).waitFor({ timeout: 20000 });
    await shoot(page, "remote-turn");
  } finally {
    await browser.close();
    await host.stop();
    model.close();
  }
}

await main();
