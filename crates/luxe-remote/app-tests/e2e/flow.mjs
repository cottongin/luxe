// The app end to end in WebKit, Safari's engine: pair by pasting the invitation link, open the
// session from the list, send a message, grant the write it asks for, and watch the mock model's
// turn arrive as the TUI would show it: the rendered answer, the changed file in the dock, and its
// diff.
//
// Needs a built `luxe` (`LUXE_BIN`, or target/debug/luxe) and Playwright's WebKit
// (`npx playwright install webkit`). Run: npm run e2e

import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { basename } from "node:path";
import { after, before, test } from "node:test";
import { LOGIN, startHost } from "./host.mjs";
import { WRITTEN, startMockProvider } from "./mock-provider.mjs";

const playwright = createRequire(import.meta.url)("playwright");

let model, host, browser, page;
const troubles = [];

before(async () => {
  model = await startMockProvider();
  host = await startHost(model.port);
  browser = await playwright.webkit.launch();
  const context = await browser.newContext({
    ...playwright.devices["iPad Pro 11 landscape"],
    extraHTTPHeaders: { "Tailscale-User-Login": LOGIN },
  });
  page = await context.newPage();
  page.on("pageerror", (error) => troubles.push(`page error: ${error.message}`));
  page.on("console", (message) => message.type() === "error" && troubles.push(`console: ${message.text()}`));
});

after(async () => {
  await browser?.close();
  await host?.stop();
  await model?.close();
});

// The break this catches: any link in the chain from invitation to diff (the unpaired app's
// pairing field, pairing, the session list, the socket, the composer, the permission card,
// rendering, the dock, the diff request) broken in Safari's engine
test("a paired iPad drives a turn and reads its diff", async () => {
  await pair();
  await page.locator("a.session").click();
  await page.locator(".composer textarea").fill("Write the file, please.");
  await page.locator(".send").click();
  await page.locator(".prompt .choice.once").click();
  await page.locator(".item.reply strong", { hasText: WRITTEN.path }).waitFor();
  await page.locator(`button.change[data-path="${WRITTEN.path}"]`).click();
  const added = page.locator(".sheet .dl-addition code");
  assert.equal((await added.textContent())?.trim(), WRITTEN.content.trim());
  assert.deepEqual(troubles, [], "the page ran without errors or refusals");
});

// Pairs as an app on the home screen would: the unpaired app takes the pasted link to the
// invitation page. Serve puts HTTPS in front of the bridge, so the credential is a Secure cookie,
// and WebKit drops one that arrives over this test's plain HTTP; the test carries it over itself,
// then opens the app as the page's redirect would.
async function pair() {
  await page.goto(host.url);
  await page.locator(".pair-link input").fill(`https://desk.example.ts.net/pair#${host.invitation}`);
  await page.locator(".pair-link button").click();
  const answered = page.waitForResponse("**/api/pair");
  await page.locator("#pair button").click();
  const cookie = (await (await answered).headerValue("set-cookie"))?.split(";")[0] ?? "";
  // let the page's own redirect land and finish asking, or the visit below races it
  await page.waitForURL((url) => url.pathname === "/", { waitUntil: "networkidle" });
  const at = cookie.indexOf("=");
  await page.context().addCookies([{ name: cookie.slice(0, at), value: cookie.slice(at + 1), url: host.url }]);
  // the page's own redirect may have asked for the list before the cookie was carried over
  troubles.length = 0;
  await page.goto(host.url);
}

// The break this catches: the turn reaching the model without the tool's result, so the answer
// on screen came from somewhere other than the loop the app drove
test("the model saw the message and the tool's result", () => {
  const turns = model.requests.filter((request) => request.tools?.length);
  assert.equal(turns.length, 2, "one request to act, one with the result");
  const sent = JSON.stringify(turns[0].messages);
  assert.ok(sent.includes("Write the file, please."), sent);
  assert.equal(turns[1].messages.at(-1).role, "tool");
});

// The break this catches: the Files tab's chain (the header's button, `list_directory` and
// `read_project_file` through the bridge, the file piece's rendering, Mention into the composer)
// broken in Safari's engine
test("the project panel lists the project, shows a file and mentions it", async () => {
  await page.keyboard.press("Escape");
  // The break this catches: the session's header naming its folder bare, without the branch it's on
  const header = page.locator("header.bar:has(.back) .title .project-name");
  await header.locator(".project-branch").waitFor();
  assert.match((await header.textContent()) ?? "", new RegExp(`${basename(host.project)}.*\\S+ \\*$`));
  await page.locator(".panel-button").click();
  const named = page.locator(".panel .bar .project-name");
  await named.waitFor();
  assert.ok((await named.textContent())?.includes(basename(host.project)), "the bar names the project");
  assert.ok(await named.locator(".project-mark").first().textContent(), "behind the tree's folder mark");
  // the project is a git repository the turn just wrote into, so its branch shows, starred
  assert.match((await named.locator(".project-branch").textContent()) ?? "", /\S+ \*$/);
  // the file the turn wrote wears its change, as it does in the TUI's tree
  await page.locator(`button.tree-row[data-file="${WRITTEN.path}"] .tree-name[class*="change-"]`).waitFor();
  await page.locator("button.tree-row", { hasText: "README.md" }).click();
  assert.equal((await page.locator(".peek .file .fl code").first().textContent())?.trim(), "# e2e");
  // Escape closes the file and leaves the tree, as the TUI's Escape leaves a peek
  await page.keyboard.press("Escape");
  await page.locator(".peek").waitFor({ state: "detached" });
  await page.locator("button.tree-row", { hasText: "README.md" }).click();
  await page.locator(".peek .mention").click();
  await page.waitForFunction(() => document.querySelector("textarea")?.value === "`README.md` ");
  assert.equal(await page.locator(".panel, .peek").count(), 0, "Mention leaves the composer in view");
  await page.locator(".panel-button").click();
  await page.locator("button.tree-row").first().waitFor();
  assert.equal(await page.locator(".peek").count(), 0, "the mentioned file isn't waiting in the panel");
  // straight away: Escape is armed as soon as the panel is on screen, not a frame later
  await page.keyboard.press("Escape");
  await page.locator(".panel").waitFor({ state: "detached", timeout: 2000 });
  assert.deepEqual(troubles, [], "the page ran without errors or refusals");
});

// The break this catches: the Review tab's chain (`show_open_review` through the engine to the
// review desk, `load_guided_review` of the working changes, the guide and chapter pieces, and
// `record_review_progress` kept in the session the next ask reads) broken in Safari's engine
test("the review tab reviews the changes and keeps a chapter marked", async () => {
  await page.locator(".panel-button").click();
  await page.locator('button.tab:has-text("Review")').click();
  await page.locator('.panel button.retry:has-text("Review changes")').click();
  await page.locator(".rv-target", { hasText: "Working changes" }).waitFor();
  await page.locator(".rv-chapter .rv-title").first().click();
  await page.locator(".peek .rv-chapter-view .rv-file", { hasText: WRITTEN.path }).waitFor();
  await page.locator('.peek button.mention:has-text("Reviewed")').click();
  await page.locator('.peek button.mention[aria-pressed="true"]').waitFor();
  // Escape closes the chapter, then the panel, as it closes a file and then the panel
  await page.keyboard.press("Escape");
  await page.locator(".peek").waitFor({ state: "detached" });
  await page.keyboard.press("Escape");
  await page.locator(".panel").waitFor({ state: "detached", timeout: 2000 });
  await page.locator(".panel-button").click();
  await page.locator('button.tab:has-text("Review")').click();
  // asked afresh, the host's session says which chapter was walked
  await page.locator(".rv-chapter .rv-walked").first().waitFor();
  await page.keyboard.press("Escape");
  assert.deepEqual(troubles, [], "the page ran without errors or refusals");
});

/** Opens README.md from the Files tab, its lines drawn. */
async function openReadme() {
  await page.locator(".panel").waitFor({ state: "detached" });
  await page.locator(".panel-button").click();
  await page.locator('button.tab:has-text("Files")').click();
  await page.locator("button.tree-row", { hasText: "README.md" }).click();
  await page.locator('.peek .fl[data-line="4"]').waitFor();
}

/** Selects `words` inside line `line` of the open file, as a long press and drag would. */
async function selectWithin(/** @type {number} */ line, /** @type {string} */ words) {
  await page.evaluate(([line, words]) => {
    const code = document.querySelector(`.peek .fl[data-line="${line}"] code`);
    const walker = document.createTreeWalker(/** @type {Node} */ (code), NodeFilter.SHOW_TEXT);
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      const at = node.textContent?.indexOf(String(words)) ?? -1;
      if (at < 0) continue;
      const range = document.createRange();
      range.setStart(node, at);
      range.setEnd(node, at + String(words).length);
      document.getSelection()?.removeAllRanges();
      document.getSelection()?.addRange(range);
      return;
    }
  }, [line, words]);
}

// The break this catches: line picks lost between the host's markup and the composer (rows not
// naming their lines, a tap on a number not picking, a second tap not stretching, picked rows not
// marked), or a selection inside a line, the phone's own way, not offering Ask for that line, or
// Ask writing other words than the TUI sends for the same lines
test("lines picked by number, or selected in part, are asked about in the TUI's words", async () => {
  await openReadme();
  const number = (/** @type {number} */ line) => page.locator(`.peek .fl[data-line="${line}"] .ln`);
  await number(3).click();
  await page.locator(".line-ask button", { hasText: "Ask about line 3" }).waitFor();
  await number(4).click();
  await page.locator(".line-ask button", { hasText: "Ask about lines 3–4" }).waitFor();
  assert.equal(await page.locator(".peek .fl.picked").count(), 2, "the picked rows are marked");
  await page.locator(".line-ask button").click();
  await page.waitForFunction(() => document.querySelector("textarea")?.value.includes("`README.md` lines 3–4"));
  assert.equal(await page.locator(".panel, .peek").count(), 0, "Ask leaves the composer in view");
  await openReadme();
  await selectWithin(3, "4519");
  await page.locator(".line-ask button", { hasText: "Ask about line 3" }).click();
  await page.waitForFunction(() => document.querySelector("textarea")?.value.includes("`README.md` line 3"));
  assert.deepEqual(troubles, [], "the page ran without errors or refusals");
});

/** Writes a comment in the open composer, as a question or not, and saves it unsent. */
async function saveComment(/** @type {string} */ words, question = false) {
  if (question) await page.locator(".rv-kinds button", { hasText: "Question" }).click();
  await page.locator(".rv-compose textarea").fill(words);
  await page.locator(".rv-compose button", { hasText: "Save" }).click();
}

// The break this catches: comments lost between the phone and the review desk (a line pick not
// anchoring to its line, the chapter comment not offered with nothing picked, the desk's saved
// session not redrawn as cards, a card's Delete not reaching the desk, or the guide not counting
// what's unsent)
test("a review chapter takes comments on its lines and on itself, kept by the host", async () => {
  await page.locator(".panel").waitFor({ state: "detached" });
  await page.locator(".panel-button").click();
  await page.locator('button.tab:has-text("Review")').click();
  await page.locator(".rv-chapter .rv-title").first().click();
  await page.locator('.peek .dl[data-line="1"] .ln').first().click();
  await page.locator(".line-ask button", { hasText: /^Comment$/ }).click();
  await page.locator(".rv-compose", { hasText: "Comment on line 1" }).waitFor();
  await saveComment("Why this file?", true);
  const asked = page.locator(".peek .rv-finding", { hasText: "Why this file?" });
  await asked.locator(".rv-state", { hasText: "unsent" }).waitFor();
  assert.match((await asked.locator(".rv-finding-head").textContent()) ?? "", /question.*line 1/);
  await page.locator(".line-ask button", { hasText: "Comment on the chapter" }).click();
  await saveComment("Split this up.");
  await page.locator(".peek .rv-finding", { hasText: "Split this up." }).waitFor();
  await asked.locator('button[data-act="delete"]').click();
  await asked.waitFor({ state: "detached" });
  await page.keyboard.press("Escape");
  await page.locator(".peek").waitFor({ state: "detached" });
  await page.locator(".rv-unsent", { hasText: "1 unsent comment" }).waitFor();
  await page.keyboard.press("Escape");
  assert.deepEqual(troubles, [], "the page ran without errors or refusals");
});

/** Sends `command` from the composer, as typed. */
async function runCommand(/** @type {string} */ command) {
  await page.locator(".composer textarea").fill(command);
  await page.locator(".send").click();
}

/** The label of the panel tab showing. */
const shownTab = () => page.locator('.panel button.tab[aria-selected="true"]').textContent();

// The break this catches: a phone keeping two header buttons, the panel's button not reaching the
// dock, its tabs wrapping to a second row on the narrowest iPhone, a diff opened from the dock
// drawn under the panel, Escape closing the panel with the diff, or `/dock` on a phone doing
// nothing now the dock lives in the panel
test("on a phone one header button opens the dock and the project", async () => {
  await page.locator(".panel").waitFor({ state: "detached" });
  await page.setViewportSize({ width: 375, height: 812 });
  // no dock button beside the panel's, once the app sees the narrow screen
  await page.locator(".dock-toggle").waitFor({ state: "detached", timeout: 2000 });
  await page.locator(".panel-button").click();
  assert.equal(await shownTab(), "Dock");
  const rows = await page.$$eval(".panel button.tab", (tabs) => new Set(tabs.map((tab) => tab.getBoundingClientRect().top)).size);
  assert.equal(rows, 1, "the tabs fit one row");
  await page.locator(`.panel button.change[data-path="${WRITTEN.path}"]`).click();
  await page.locator(".peek .dl-addition").waitFor();
  const onTop = await page.evaluate(() => document.elementFromPoint(innerWidth / 2, innerHeight / 2)?.closest(".peek") !== null);
  assert.ok(onTop, "the diff covers the panel");
  await page.keyboard.press("Escape");
  await page.locator(".peek").waitFor({ state: "detached" });
  assert.equal(await page.locator(".panel").count(), 1, "the diff took the Escape, not the panel");
  await page.keyboard.press("Escape");
  await page.locator(".panel").waitFor({ state: "detached", timeout: 2000 });
  await runCommand("/dock");
  assert.equal(await shownTab(), "Dock");
  await page.locator(".panel .close").click();
  await page.setViewportSize(playwright.devices["iPad Pro 11 landscape"].viewport);
  assert.deepEqual(troubles, [], "the page ran without errors or refusals");
});
