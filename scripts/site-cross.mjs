#!/usr/bin/env node
/**
 * Opens the site in every browser engine the machine can run, and asserts the demos actually start.
 *
 * `site-check.sh` already drives the *built* site in headless Chrome, which is the right check for
 * the thing it guards — a real CSP, real headers, no uncaught exceptions. This one exists because
 * two separate bugs slipped past it, and both were invisible for the same two reasons.
 *
 * **One engine is not coverage.** The demo player is behind a dynamic `import()`, and Vite's dev
 * server optimizes it mid-session and invalidates the graph it had already served. Chrome quietly
 * re-requests. Firefox calls the interrupted response corrupted, refuses it for a disallowed MIME
 * type, and every demo on the page stays blank for the rest of the session.
 *
 * **The built site is not the dev site.** That failure never touched a production bundle, so no
 * amount of checking `dist/` would have found it — and `npm run dev` is what anyone actually looks
 * at while working.
 *
 * So this drives `astro dev` by default, in Chromium, Firefox and WebKit, and asserts the islands
 * boot rather than that the markup exists. An engine the host cannot launch is reported and
 * skipped: Playwright's WebKit build wants Ubuntu's ICU and libxml2 sonames, which a rolling distro
 * does not have, and refusing to run anywhere it is missing would just mean nobody runs it.
 *
 *   node scripts/site-cross.mjs                  # astro dev, every engine available
 *   node scripts/site-cross.mjs --built          # the built site, served statically
 *   node scripts/site-cross.mjs --require webkit # fail if that engine cannot launch
 */

import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { readFile, rm } from "node:fs/promises";
import { createRequire } from "node:module";
import { extname, join, normalize } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const web = fileURLToPath(new URL("../web/", import.meta.url));

/** Pages worth opening, and what has to be alive on each once it has settled. */
const PAGES = [
	{ path: "/", demos: true, stills: true },
	{ path: "/docs/", demos: false, stills: false },
];

/**
 * Console noise that is the browser talking about itself rather than about us.
 *
 * Firefox warns about scroll-linked effects on any page with a scroll-driven reveal, which is a
 * performance note about async panning and not a defect. Everything else is reported.
 */
const IGNORABLE = [
	/scroll-linked positioning effect/i,
	/\[vite\]/i,
	/Download the .* DevTools/i,
	// A *cancelled* request, not a failed one. The dev server's HMR client and the island scripts
	// are routinely abandoned when a page settles, and Firefox reports that as a binding abort where
	// Chrome says nothing at all. Three engines, three spellings for one condition — WebKit's was
	// missing, so the optimizer reload that Chrome swallows and Firefox names failed the run outright
	// on the engine nobody can reproduce locally.
	/NS_BINDING_ABORTED/i,
	/net::ERR_ABORTED/i,
	/Load request cancelled/i,
];

/**
 * Dev-server churn, which is Vite talking about itself.
 *
 * When the optimizer discovers a dependency it re-bundles and answers in-flight module requests
 * with a 504, and its client recovers by reloading the page. Chrome swallows that silently; Firefox
 * surfaces the rejected import first. Treating it as a defect makes this check fail on a page whose
 * demos all started, which is a check nobody will keep.
 *
 * So dev is judged on its *end state* — did every island boot — and only the built site is held to
 * zero errors. That still catches the failure this file was written for, because that one left the
 * demos blank for the rest of the session rather than recovering.
 *
 * WebKit's spelling carries no URL, so unlike Firefox's it cannot be scoped to `.vite/deps`. It is
 * forgiven anyway, and only here: a dev-only reload that recovers is not a defect, and holding one
 * engine to a stricter rule than the other two only means its check gets switched off.
 */
const DEV_CHURN = [
	/error loading dynamically imported module.*\.vite\/deps/i,
	/Outdated Optimize Dep/i,
	/Importing a module script failed/i,
];

const args = process.argv.slice(2);
const built = args.includes("--built");
const required = args.flatMap((arg, index) => (arg === "--require" ? [args[index + 1]] : []));

// === Serving ===

const MIME = {
	".html": "text/html; charset=utf-8",
	".js": "text/javascript; charset=utf-8",
	".css": "text/css; charset=utf-8",
	".svg": "image/svg+xml",
	".json": "application/json",
	".cast": "application/json",
	".woff2": "font/woff2",
	".png": "image/png",
	".wasm": "application/wasm",
};

/**
 * Where both servers listen and every browser and probe connects: an address, not `localhost`.
 * In a container `localhost` can name `::1` to the server binding it and `127.0.0.1` to the
 * client dialing it, and in Playwright's image the dev server came up and refused every request.
 */
const LOOPBACK = "127.0.0.1";

/** Serves `web/dist` the way a static host would, including the `.wasm` type the player needs. */
function serveBuilt(root) {
	return new Promise((resolve) => {
		const server = createServer(async (request, response) => {
			const url = new URL(request.url ?? "/", "http://localhost");
			const asked = normalize(join(root, decodeURIComponent(url.pathname)));
			if (!asked.startsWith(root)) return response.writeHead(403).end();
			// The type comes from the file that is actually sent, not from the path that was asked
			// for. A directory has no extension, so deriving it from the URL served `index.html` as
			// `application/octet-stream` — and a browser handed that downloads the page instead of
			// rendering it, which surfaces as every engine failing to navigate.
			for (const file of [asked, join(asked, "index.html")]) {
				try {
					const body = await readFile(file);
					const type = MIME[extname(file)] ?? "application/octet-stream";
					response.writeHead(200, { "content-type": type });
					return response.end(body);
				} catch {
					/* try the next candidate */
				}
			}
			response.writeHead(404).end("not found");
		});
		server.listen(0, LOOPBACK, () => resolve({ server, port: server.address().port }));
	});
}

/**
 * Starts `astro dev` on a cold dependency cache and waits for it to answer.
 *
 * The cache is cleared on purpose, and this is the whole reason the dev mode restarts the server
 * per engine rather than sharing one. The failure being guarded against is a *race*: Vite optimizes
 * the player the first time something asks for it, mid-session, and only the browser that arrives
 * before that has happened is exposed to it. Run Chromium first against a shared server and it
 * warms the cache for everyone — Firefox then sails through, the check goes green, and it has
 * proved nothing. Verified by deleting the fix and watching this catch it.
 */
async function serveDev(port) {
	await rm(join(web, "node_modules/.vite"), { recursive: true, force: true });
	// Astro's own entry, not `npx`. A wrapper means the server is a *grandchild*, so stopping it
	// needs a signal to the whole process group — and a group kill with the wrong pid takes the
	// shell that launched this with it. Spawned directly, `child.kill()` is the server.
	const child = spawn(
		process.execPath,
		[join(web, "node_modules/astro/bin/astro.mjs"), "dev", "--host", LOOPBACK, "--port", String(port)],
		{ cwd: web, stdio: ["ignore", "pipe", "pipe"] },
	);
	return watchServer({ child, port, output: [], exit: null });
}

/**
 * Keeps the last of what `target`'s server prints, and how it ended, so a server that never
 * answers can say why. Read either way: a pipe nobody drains fills up and stalls its server.
 */
function watchServer(target) {
	const keep = (chunk) => {
		target.output.push(...String(chunk).split("\n").filter((line) => line.trim()));
		target.output.splice(0, Math.max(0, target.output.length - 30));
	};
	target.child.stdout.on("data", keep);
	target.child.stderr.on("data", keep);
	target.child.on("exit", (code, signal) => {
		target.exit = signal ?? `exit code ${code}`;
	});
	return target;
}

/** Why `target`'s server never answered, with the last of what it printed. */
function reportSilentServer(name, target) {
	const why = target.exit ? `it ended (${target.exit})` : "it was still running";
	console.log(`✗ ${name.padEnd(9)} the dev server never answered; ${why}`);
	console.log(`    the last ask got: ${target.lastAnswer ?? "nothing yet"}`);
	for (const line of target.output) console.log(`    ${line}`);
}

function shutdown(target) {
	if (target?.server) target.server.close();
	if (target?.child && !target.child.killed) {
		target.child.kill("SIGTERM");
	}
}

async function waitForServer(target, seconds = 90) {
	for (let attempt = 0; attempt < seconds * 2 && !target.exit; attempt += 1) {
		try {
			const response = await fetch(`http://${LOOPBACK}:${target.port}/`);
			if (response.ok) return true;
			target.lastAnswer = `HTTP ${response.status}: ${(await response.text()).slice(0, 300)}`;
		} catch (error) {
			// Not up yet, usually; the cause is what tells a refused port from a wrong address
			target.lastAnswer = `${error.message}${error.cause ? ` (${error.cause.code ?? error.cause.message})` : ""}`;
		}
		await new Promise((done) => setTimeout(done, 500));
	}
	return false;
}

// === Checking ===

/**
 * Brings the hero to rest before anything is scrolled to.
 *
 * The hero's demo waits a screen below its place until the opening has played. Centring it from
 * there scrolls the page to where it is *waiting*, the scroll assembles the hero, and the demo rises
 * into its real place above the viewport — so whether it was ever seen depends on which frame each
 * engine paints, and WebKit missed it. A reader never jumps to an element mid-assembly, so the check
 * does what a reader's first scroll does and lets it settle.
 */
async function settleHero(tab) {
	const assembling = await tab.evaluate(() => {
		const hero = document.querySelector("[data-hero]");
		if (!hero || getComputedStyle(hero).getPropertyValue("--p").trim() === "1") return false;
		dispatchEvent(new Event("scroll"));
		return true;
	});
	if (assembling) await tab.waitForTimeout(1400);
}

/**
 * Whether each demo's terminal fills the frame the player sized it for, no wider and no narrower.
 *
 * The player sets its type from a measured `ch`, and Chromium then rounded each glyph's advance to a
 * whole pixel, so a 120-cell line came out 120 roundings off: at this window the hero ran 7% off the
 * right of its frame and cut the statusline mid-word, and at others it stopped short and left a
 * margin of dead canvas. Nothing threw. Measured at 1600px and twice the density, because that is a
 * size where the rounding goes the harmful way, and the system Chrome the smoke run drives already
 * uses fractional advances and never showed it.
 */
async function demosFit(browser, port, path) {
	const context = await browser.newContext({
		viewport: { width: 1600, height: 1000 },
		deviceScaleFactor: 2,
		colorScheme: "dark",
	});
	const tab = await context.newPage();
	const problems = [];
	try {
		await tab.goto(`http://${LOOPBACK}:${port}${path}`, { waitUntil: "networkidle" });
		await settleHero(tab);
		const demos = tab.locator(".demo");
		const count = await demos.count();
		for (let index = 0; index < count; index += 1) {
			const demo = demos.nth(index);
			await demo.evaluate((node) => node.scrollIntoView({ block: "center", behavior: "instant" }));
			const started = await demo
				.locator(".ap-line")
				.first()
				.waitFor({ timeout: 60_000 })
				.then(() => true)
				.catch(() => false);
			if (!started) continue; // the boot check reports a demo that never starts
			// The terminal against the frame the player sized it for. Its own width grows with its
			// lines, so comparing those two with each other sees nothing
			// Watched for a few seconds rather than read once: the terminal is as wide as its longest
			// line so far, and a take whose first frame is short lines is narrow for that instant at
			// the right scale. A rounding fault never comes right, so it still fails.
			const measure = () =>
				demo.evaluate((node) => ({
					frame: node.querySelector(".ap-wrapper").getBoundingClientRect().width,
					term: node.querySelector(".ap-term").getBoundingClientRect().width,
				}));
			const fits = (fit) => Math.abs(fit.term - fit.frame) <= Math.max(2, fit.frame * 0.01);
			let fit = await measure();
			for (let tries = 0; tries < 24 && !fits(fit); tries += 1) {
				await tab.waitForTimeout(250);
				fit = await measure();
			}
			if (!fits(fit)) {
				problems.push(
					`${path}: demo ${index + 1} is ${fit.term.toFixed(0)}px in a ` +
						`${fit.frame.toFixed(0)}px frame — glyph advances are being rounded`,
				);
			}
		}
	} finally {
		await context.close();
	}
	return problems;
}

/**
 * Opens every page in one engine and reports what went wrong.
 *
 * The assertion that matters is `players === mounts`: markup being present proves the component
 * rendered, and every failure this file exists for left the markup perfectly intact and the island
 * dead inside it.
 */
async function checkEngine(engine, name, port) {
	const browser = await engine.launch();
	const problems = [];
	try {
		for (const page of PAGES) {
			const context = await browser.newContext({
				viewport: { width: 1440, height: 900 },
				colorScheme: "dark",
			});
			const tab = await context.newPage();
			const noise = [];
			tab.on("pageerror", (error) => noise.push(`uncaught: ${error}`));
			tab.on("console", (message) => {
				if (message.type() === "error") noise.push(`console: ${message.text()}`);
			});
			tab.on("requestfailed", (request) => {
				noise.push(`request failed: ${request.url()} (${request.failure()?.errorText})`);
			});

			await tab.goto(`http://${LOOPBACK}:${port}${page.path}`, { waitUntil: "networkidle" });
			await settleHero(tab);

			if (page.demos) {
				// A demo waits until most of it is on screen before it starts, so bringing its top edge
				// into view is not enough to ask whether it works — `scrollIntoViewIfNeeded` scrolls the
				// minimum distance, which for a demo taller than half the viewport leaves it below the
				// threshold and looking broken. Centring it is what a reader does.
				const demos = tab.locator(".demo");
				const count = await demos.count();
				for (let index = 0; index < count; index += 1) {
					const demo = demos.nth(index);
					await demo.evaluate((node) =>
						node.scrollIntoView({ block: "center", behavior: "instant" }),
					);
					// Scoped to *this* demo rather than to the page's nth player: they start lazily now,
					// so a global index counts whichever happened to boot first.
					//
					// Generous, because a demo plays once and never pauses: by the time this asks
					// about the fifth, four players are still rendering frames behind it. That is
					// what a reader scrolling the page has too, but on a shared CI runner it starved
					// the later ones past a 20s budget — which failed a different demo each run, in
					// one engine, on a page nothing had changed. The claim being made is that a demo
					// *starts*, not that it starts inside some particular second.
					await demo
						.locator(".ap-player")
						.waitFor({ timeout: 60_000 })
						.catch(() => problems.push(`${page.path}: demo ${index + 1} never started`));
				}
				if (count === 0) problems.push(`${page.path}: no demos on the page at all`);
			}
			if (page.stills && (await tab.locator(".still__img").count()) === 0) {
				problems.push(`${page.path}: no stills on the page at all`);
			}

			if (page.demos) problems.push(...(await demosFit(browser, port, page.path)));

			const forgiven = built ? IGNORABLE : [...IGNORABLE, ...DEV_CHURN];
			for (const line of noise) {
				if (!forgiven.some((pattern) => pattern.test(line))) problems.push(`${page.path}: ${line}`);
			}
			await context.close();
		}
	} finally {
		await browser.close();
	}
	return problems;
}

// === Run ===

// Resolved against `web/`, not against this file. The script lives in `scripts/`, which has no
// `node_modules` of its own, so a bare specifier here looks in the wrong tree and fails with a
// message about Playwright being missing while it sits installed one directory over.
const { chromium, firefox, webkit } = await (async () => {
	try {
		const require = createRequire(new URL("../web/package.json", import.meta.url));
		const loaded = await import(pathToFileURL(require.resolve("playwright")).href);
		// Playwright's entry is CommonJS, and importing one by path puts its exports on `default`
		// rather than lifting them — so the named bindings come back undefined and every engine fails
		// to launch for a reason that reads like a browser problem.
		return loaded.chromium ? loaded : loaded.default;
	} catch (error) {
		console.error(`playwright is not available: ${error}`);
		console.error("run `npm install` in web/, then `npx playwright install`");
		process.exit(2);
	}
})();

let shared = built ? await serveBuilt(join(web, "dist")) : null;
let current = shared;
process.on("exit", () => shutdown(current));

console.log(`checking the ${built ? "built site" : "dev server"}\n`);

let failed = false;
let port = 4399;
for (const [name, engine] of [
	["chromium", chromium],
	["firefox", firefox],
	["webkit", webkit],
]) {
	// A fresh server per engine in dev mode, each on its own port so a lingering listener from the
	// previous one cannot be mistaken for this one.
	if (!built) {
		shutdown(current);
		current = await serveDev((port += 1));
		if (!(await waitForServer(current))) {
			reportSilentServer(name, current);
			failed = true;
			continue;
		}
	}

	let problems;
	try {
		problems = await checkEngine(engine, name, current.port);
	} catch (error) {
		const missing = /missing dependencies|Executable doesn't exist/i.test(String(error));
		if (missing && !required.includes(name)) {
			console.log(`  ${name.padEnd(9)} skipped — not installed on this host`);
			continue;
		}
		console.log(`✗ ${name.padEnd(9)} could not launch: ${String(error).split("\n")[0]}`);
		failed = true;
		continue;
	}
	if (problems.length === 0) {
		console.log(`✓ ${name.padEnd(9)} every demo started, no errors`);
	} else {
		failed = true;
		console.log(`✗ ${name}`);
		for (const problem of problems.slice(0, 8)) console.log(`    ${problem}`);
	}
}

shutdown(current);
if (failed) {
	console.error("\na browser disagreed with the others — see above.");
	process.exit(1);
}
console.log("\nevery engine agrees.");
