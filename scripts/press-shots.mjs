#!/usr/bin/env node
// Render the press kit's screenshots to web/public/press/<name>.png.
//
// Each one is a still the site already draws (`web/src/pages/press-shots.astro`), photographed at
// twice its size so it stays sharp in an article or on a slide. Separate from `site-assets.sh`
// for the reason `social-card.sh` is: that one turns Rust into assets and CI checks it, and this
// needs Node, a build and a browser.
//
// Usage: node scripts/press-shots.mjs   (from the repository root, after `npm install` in web/)

import { spawn, spawnSync } from "node:child_process";
import { mkdirSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const web = join(dirname(fileURLToPath(import.meta.url)), "..", "web");
const out = join(web, "public", "press");
const port = process.env.PORT ?? "4332";
const origin = `http://localhost:${port}`;

const require = createRequire(join(web, "package.json"));
const playwright = await import(pathToFileURL(require.resolve("playwright")).href);
const { chromium } = playwright.chromium ? playwright : playwright.default;

function build() {
  const built = spawnSync("npm", ["run", "build"], { cwd: web, stdio: "inherit" });
  if (built.status !== 0) throw new Error(`the site build failed with status ${built.status}`);
}

async function waitForPreview() {
  for (let attempt = 0; attempt < 30; attempt++) {
    await new Promise((done) => setTimeout(done, 1000));
    const reply = await fetch(`${origin}/press-shots/`).catch(() => null);
    if (reply?.ok) return;
  }
  throw new Error(`the preview never came up on ${origin}`);
}

// A tall window, so a still is sized by its own width cap rather than by the screen's height
async function photograph() {
  const browser = await chromium.launch();
  const page = await browser.newPage({
    viewport: { width: 1980, height: 2400 },
    deviceScaleFactor: 2,
    reducedMotion: "reduce",
  });
  await page.goto(`${origin}/press-shots/`, { waitUntil: "networkidle" });
  await page.evaluate(() => document.fonts.ready);
  mkdirSync(out, { recursive: true });
  for (const shot of await page.$$("[data-shot]")) {
    const name = await shot.getAttribute("data-shot");
    await (await shot.$(".still__plate")).screenshot({ path: join(out, `${name}.png`) });
    console.log(`wrote web/public/press/${name}.png`);
  }
  await browser.close();
}

build();
const preview = spawn("npx", ["astro", "preview", "--port", port], { cwd: web, stdio: "ignore" });
try {
  await waitForPreview();
  await photograph();
} finally {
  preview.kill();
}
