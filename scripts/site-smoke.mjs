#!/usr/bin/env node
/**
 * site-smoke — load the built site in a real browser and fail on anything broken.
 *
 * `astro check` type-checks and `astro build` emits files; neither one ever *renders* a page, so a
 * component that throws at runtime builds clean and ships blank. That is exactly how the demo
 * players went out as empty boxes: the CSP blocked the player's WebAssembly, every `create()` threw
 * into a floating promise, and nothing in the pipeline was looking.
 *
 * So this drives headless Chrome over CDP and asserts three things per page: no uncaught
 * exceptions, no CSP violations, and the elements that are supposed to exist actually do. It runs
 * against `wrangler pages dev`, not `astro preview`, because `_headers` — where the CSP lives — is
 * only applied by Pages.
 *
 * One thing about the site decides how the checking works: the demos are `client:visible` islands,
 * and such an island mounts where the viewport comes to *rest*, not where it passes. A single jump
 * to the bottom therefore hydrates whatever happens to be in view when it lands — which depends on
 * how tall the page is at that instant, which depends on how fast the images arrived. That is
 * measured rather than assumed: with the network throttled to a cold runner's speed, this script
 * failed six runs out of six on the jump and none out of six once it walked the page a screen at a
 * time instead. So it walks, and it waits for the load event rather than guessing at one.
 *
 * Usage: node scripts/site-smoke.mjs [base-url]
 */

import { spawn } from "node:child_process";
import { setTimeout as sleep } from "node:timers/promises";

const BASE = process.argv[2] ?? "http://127.0.0.1:8788";
const PORT = 9333;

/** What each page must contain to count as rendered, keyed by path. */
const PAGES = [
	{
		path: "/",
		expect: [".ap-player", ".still__img", ".masthead__mark"],
		shine: ".hero__mark .wordmark",
		captures: true,
	},
	// The measured bars come from a JSON a script writes, the part that could go missing
	{ path: "/tools/", expect: [".bench__bar", ".contrast", ".masthead__mark"] },
	{ path: "/docs/", expect: [".docs__body", ".docs__nav"] },
	// Highlighted code and an inlined screenshot, the part a broken plugin would drop silently
	{ path: "/docs/quickstart/", expect: [".docs__body", ".astro-code", ".guide-shot svg"] },
	{ path: "/brand/", expect: [".masthead__mark"] },
	{ path: "/404", expect: [] },
];

/**
 * The guide as text, for agents: each must be served as the type it claims, with UTF-8, since
 * Pages only applies `_headers` and a wrong charset turns every em dash into mojibake for a model.
 */
const TEXTS = [
	{ path: "/llms.txt", type: "text/plain", expect: ["# luxe", "/docs/install.md"] },
	{ path: "/llms-full.txt", type: "text/plain", expect: ["# Install"] },
	{ path: "/docs/install.md", type: "text/markdown", expect: ["# Install"] },
];

/** What is wrong with a text endpoint, if anything. */
async function checkText({ path, type, expect }) {
	const response = await fetch(BASE + path);
	if (!response.ok) return [`HTTP ${response.status}`];
	const contentType = response.headers.get("content-type") ?? "";
	const bad = [];
	if (!contentType.startsWith(type) || !/charset=utf-8/i.test(contentType)) {
		bad.push(`content-type is ${contentType || "missing"}, wanted ${type}; charset=utf-8`);
	}
	const body = await response.text();
	for (const text of expect) if (!body.includes(text)) bad.push(`missing ${JSON.stringify(text)}`);
	return bad;
}

/** CSP reports and network failures arrive as Log entries, not exceptions. */
const IGNORABLE = [/favicon/i, /Failed to load resource.*404/i];

async function cdp(url) {
	const problems = [];
	const ws = new WebSocket(url);
	let seq = 0;
	const pending = new Map();
	const send = (method, params = {}) =>
		new Promise((resolve) => {
			const id = ++seq;
			pending.set(id, resolve);
			ws.send(JSON.stringify({ id, method, params }));
		});

	await new Promise((resolve, reject) => {
		ws.addEventListener("open", resolve, { once: true });
		ws.addEventListener("error", reject, { once: true });
	});

	ws.addEventListener("message", (event) => {
		let msg;
		try {
			msg = JSON.parse(event.data);
		} catch {
			return; // a frame we can't read tells us nothing; the assertions still hold
		}
		if (msg.id && pending.has(msg.id)) {
			pending.get(msg.id)(msg.result);
			pending.delete(msg.id);
			return;
		}
		if (msg.method === "Runtime.exceptionThrown") {
			const d = msg.params.exceptionDetails;
			problems.push(`uncaught: ${d.exception?.description ?? d.text}`);
		}
		if (msg.method === "Log.entryAdded") {
			const { level, text, source } = msg.params.entry;
			if (level === "error" && !IGNORABLE.some((re) => re.test(text))) {
				problems.push(`${source}: ${text}`);
			}
		}
	});

	return { send, problems, close: () => ws.close() };
}

/**
 * Evaluate `expression` until it returns something truthy, or the attempts run out.
 *
 * `scroll` walks the page down one screen per attempt while it waits, which is what gives a
 * `client:visible` island its chance to mount: the page rests at each stop long enough for a frame,
 * and never returns to the top, so nothing is skipped however tall the page turns out to be.
 */
async function poll(send, expression, { attempts = 60, waitMs = 250, scroll = false } = {}) {
	for (let attempt = 0; attempt < attempts; attempt++) {
		const { result } = await send("Runtime.evaluate", {
			expression,
			returnByValue: true,
		});
		if (result?.value) return result.value;
		if (scroll) {
			await send("Runtime.evaluate", {
				expression: `scrollTo(0, Math.min(${attempt} * innerHeight * 0.8, document.body.scrollHeight))`,
			});
		}
		await sleep(waitMs);
	}
	return 0;
}

/**
 * Milliseconds from load: mid-pass across the letters, and parked after it.
 *
 * Offset by the front page's `--shine-delay`, which holds the first sweep until the opening has
 * finished typing the logotype in. Sampled at 800ms the band had not started, and this probe
 * correctly reported that it could not see the shine move — which is the failure it exists to
 * catch, so the window moves rather than the assertion loosening.
 */
const SHINE_DELAY = 7800;
const MID_PASS = SHINE_DELAY + 800;
const AT_REST = SHINE_DELAY + 3000;

/**
 * How far right of the bottom the top of the band has to sit, as a fraction of the mark's width.
 *
 * The terminal's lean is one column per row over seven rows of a forty-four column mark, and the
 * slices measured are a third of the height apart — so the true figure is near a tenth. A third of
 * that is enough to tell a `/` from a `\` or from no lean at all, without pinning the exact angle.
 */
const LEAN_FLOOR = 0.035;

/**
 * Hold everything in the mark's corner of the page at an exact frame, and report where that leaves
 * the band.
 *
 * The Web Animations API, not a negative `animation-delay`: changing the delay on an animation that
 * is already running *shifts* its timeline rather than setting it, so the frame you get back
 * depends on how long the page happens to have been open. Two arbitrary frames differ on a broken
 * build as readily as on a good one, which is a probe that passes whatever it is shown.
 * `currentTime` is the frame you asked for.
 *
 * **Every** animation over the captured region, not only the sweep. Two captures are subtracted to
 * isolate the band, so anything else still moving between them lands in the difference and is
 * measured as if it were shine. The front page has two such things: the opening `print` reveal,
 * which is a `clip-path` on the `<svg>` itself, and `.hero__caret` — an opaque bar in the accent
 * color that runs the width of the mark while it types. Pinning the band alone left both of them
 * advancing in real time, so the probe's answer depended on how long the page had been open when
 * it ran: green on a slow runner where the opening had finished, and a wrong lean on a fast one
 * where it had not. A caret caught mid-sweep is a bright stripe in the diff, which is exactly the
 * shape this is trying to measure.
 *
 * Scoped to the mark's own container rather than the document, so a check that runs after this one
 * still finds the rest of the page animating.
 *
 * `null` means there was no sweep to sample at all, which is a broken probe rather than a pass.
 */
const holdAt = (selector, ms) => `(() => {
	const svg = document.querySelector(${JSON.stringify(selector)});
	const band = svg?.querySelector(".wordmark__band");
	if (!band?.getAnimations().length) return null;
	// The opening reveal and the caret are siblings of the mark, not children of it.
	const region = svg.closest(".hero__mark") ?? svg;
	for (const animation of document.getAnimations()) {
		const target = animation.effect?.target;
		if (!target || !(region === target || region.contains(target))) continue;
		animation.pause();
		// Settle the one-shots on their last frame — the reveal fully open, the caret gone — so the
		// two captures differ by the band and nothing else. An infinite animation has no last
		// frame; the drift parks at zero, where it is identical in both.
		const end = animation.effect?.getComputedTiming?.().endTime;
		animation.currentTime = Number.isFinite(end) ? end : 0;
	}
	for (const animation of band.getAnimations()) animation.currentTime = ${ms};
	return getComputedStyle(band).transform;
})()`;

/**
 * The mark's box in page coordinates, plus a patch inside it that no letter covers.
 *
 * As near the middle as the letters allow, because that is where the band is at `MID_PASS`. Simply
 * taking the first bare point puts the patch in the top-left corner, which the band has not reached
 * at either sampled frame — and a patch the shine could never touch cannot report a mask that has
 * stopped containing it.
 */
const probeGeometry = (selector) => `(() => {
	const svg = document.querySelector(${JSON.stringify(selector)});
	if (!svg) return null;
	// The band sits over the letters, and this finds them by hit testing.
	const band = svg.querySelector(".wordmark__band");
	if (band) band.style.pointerEvents = "none";
	const box = svg.getBoundingClientRect();
	const ink = [...svg.querySelectorAll(".wordmark__ink path")];
	const middle = box.left + box.width / 2;
	let bare = null;
	for (let row = 1; row < 16; row++) {
		for (let col = 1; col < 32; col++) {
			const x = box.left + (box.width * col) / 32;
			const y = box.top + (box.height * row) / 16;
			if (ink.includes(document.elementFromPoint(x, y))) continue;
			if (!bare || Math.abs(x - middle) < Math.abs(bare.x - middle)) bare = { x, y };
		}
	}
	return {
		mark: { x: box.left + scrollX, y: box.top + scrollY, width: box.width, height: box.height },
		bare: bare && { x: bare.x + scrollX - 4, y: bare.y + scrollY - 4, width: 8, height: 8 },
	};
})()`;

/**
 * Where the shine sits, per horizontal slice of the mark.
 *
 * The lit capture minus the resting one is the band on its own, with the letters and the drift
 * subtracted out. Its weighted mean column, taken across the top slice and the bottom slice, is the
 * lean: the terminal's band gains a column of lead per row upward, so the top slice has to sit to
 * the right of the bottom one.
 */
const measureLean = (litPng, restPng) => `(async () => {
	const load = (data) =>
		new Promise((resolve, reject) => {
			const image = new Image();
			image.onload = () => resolve(image);
			image.onerror = () => reject(new Error("the capture would not decode"));
			image.src = "data:image/png;base64," + data;
		});
	const pixels = (image) => {
		const canvas = document.createElement("canvas");
		[canvas.width, canvas.height] = [image.width, image.height];
		const context = canvas.getContext("2d", { willReadFrequently: true });
		context.drawImage(image, 0, 0);
		return context.getImageData(0, 0, image.width, image.height).data;
	};
	const [litImage, restImage] = await Promise.all([
		load(${JSON.stringify(litPng)}),
		load(${JSON.stringify(restPng)}),
	]);
	const { width, height } = litImage;
	const [lit, rest] = [pixels(litImage), pixels(restImage)];
	const meanColumn = (from, to) => {
		let weight = 0;
		let moment = 0;
		for (let y = from; y < to; y++) {
			for (let x = 0; x < width; x++) {
				const at = (y * width + x) * 4;
				let added = 0;
				for (const channel of [0, 1, 2]) {
					added += Math.max(0, lit[at + channel] - rest[at + channel]);
				}
				weight += added;
				moment += added * x;
			}
		}
		return weight > 0 ? moment / weight : null;
	};
	const slice = Math.max(1, Math.floor(height / 3));
	return { top: meanColumn(0, slice), bottom: meanColumn(height - slice, height), width };
})()`;

/**
 * Assert the shine actually paints, paints only on the letters, and leans the way the terminal's does.
 *
 * The band is a `<rect>` that is always in the DOM, so every selector check above passes whether or
 * not one pixel of it ever lands on the mark — and for the whole life of the site, none did. The
 * mask sat on the element the sweep transforms, and an element's own transform redefines the user
 * space a `userSpaceOnUse` mask resolves in, so the letters traveled with the band and the two
 * never met. Nothing threw, nothing was missing, and the site shipped with no shine at all.
 *
 * Pixels are the only honest witness, so this compares the mark mid-pass against the mark at rest.
 * They must differ — that is the shine arriving — while a patch of background inside the same box
 * must not, which is the mask still doing its job rather than the band washing over everything.
 *
 * The lean gets the same treatment, and for the same reason: an SVG gradient's vector runs across
 * the stripe it draws, not along it, so `/` and `\` are one sign apart and look like the same
 * arithmetic on the page. Only the paint says which one shipped.
 */
async function shinePaints(send, selector) {
	// `instant`, because the site sets `scroll-behavior: smooth` and a clip captured mid-scroll comes
	// back blank — two blank captures compare equal, which is this check's own failure signal.
	await send("Runtime.evaluate", {
		expression: `scrollTo({ top: 0, left: 0, behavior: "instant" })`,
	});
	const { result } = await send("Runtime.evaluate", {
		expression: probeGeometry(selector),
		returnByValue: true,
	});
	const geometry = result?.value;
	if (!geometry) return [`no shine to check: ${selector} is not on the page`];
	if (!geometry.bare) return [`no background inside ${selector} to check the mask against`];

	const capture = async (ms, clip) => {
		const { result: held } = await send("Runtime.evaluate", {
			expression: holdAt(selector, ms),
			returnByValue: true,
		});
		// Two frames: one for the hold to land, one for the compositor to draw it.
		await send("Runtime.evaluate", {
			awaitPromise: true,
			expression: `new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)))`,
		});
		const shot = await send("Page.captureScreenshot", {
			format: "png",
			clip: { ...clip, scale: 1 },
			captureBeyondViewport: true,
		});
		return { at: held?.value ?? null, png: shot?.data ?? "" };
	};

	const lit = await capture(MID_PASS, geometry.mark);
	const rest = await capture(AT_REST, geometry.mark);
	if (!lit.at || !rest.at) return [`no sweep animation on the band inside ${selector}`];
	if (lit.at === rest.at) {
		return [`the probe is blind: the band does not move between ${MID_PASS}ms and ${AT_REST}ms`];
	}

	const problems = [];
	if (!lit.png || lit.png === rest.png) {
		problems.push(`the shine paints nothing: ${selector} is identical mid-pass and at rest`);
	}
	// The same frame twice. Two captures are subtracted to isolate the band, so the whole method
	// rests on nothing *else* over the mark moving between them — and when something did, the
	// difference carried it and the lean below measured it as shine. This is that assumption
	// asserted rather than assumed: the opening reveal and the caret used to fail it.
	const again = await capture(MID_PASS, geometry.mark);
	if (again.png !== lit.png) {
		problems.push(
			`the mark is not still at one frame: two captures of ${MID_PASS}ms inside ${selector} ` +
				`differ, so something over it is animating unpinned and lands in the difference`,
		);
	}
	const litBare = await capture(MID_PASS, geometry.bare);
	const restBare = await capture(AT_REST, geometry.bare);
	if (litBare.png !== restBare.png) {
		problems.push(`the shine escapes the letters: background inside ${selector} lit up too`);
	}

	const { result: lean } = await send("Runtime.evaluate", {
		expression: measureLean(lit.png, rest.png),
		awaitPromise: true,
		returnByValue: true,
	});
	const { top, bottom, width } = lean?.value ?? {};
	if (typeof top !== "number" || typeof bottom !== "number") {
		problems.push(`could not read where the shine fell inside ${selector}`);
	} else if (top - bottom < width * LEAN_FLOOR) {
		problems.push(
			`the shine leans the wrong way inside ${selector}: the top of the band sits ` +
				`${(top - bottom).toFixed(1)}px right of the bottom, wanted at least ` +
				`${(width * LEAN_FLOOR).toFixed(1)}px`,
		);
	}
	return problems;
}

/**
 * The advance width the captures are drawn against, as a fraction of the type size.
 *
 * `fixture_svg.rs`'s `ADVANCE`. Every row of a capture is stretched to `cols × CELL_W` by
 * `textLength`, so a face that advances anything else has its glyphs squeezed or pulled apart —
 * across every picture on the page at once, and silently.
 */
const CAPTURE_ADVANCE = 0.6;

/**
 * Whether the captures are set in the face this site ships, measured rather than assumed.
 *
 * An SVG loaded through `<img src>` is a document of its own in secure static mode: it cannot see
 * this page's `@font-face`, so the subset webfont never reached a single screenshot and every Nerd
 * Font icon in them drew as tofu. None of that is visible to anyone with the face installed
 * locally, which is anyone who works on luxe — so the check has to be a measurement taken in the
 * page.
 *
 * `foreign` is the structural half: a capture that is not an `<svg>` holding real `<text>` in *this*
 * document cannot be reached by a face declared in it, whatever the pixels say. `advance` is the
 * numeric half, and it is the figure the export's geometry is pinned to.
 */
const captureTypeface = `(async () => {
	// Both kinds of capture. A .shot__img is generated from the Rust source; a .still__img is a
	// frame cut out of a recording. Same page font, same failure, so the probe has to see both or
	// it silently stops covering whichever a page happens to use. (No backticks in here: this
	// whole function is a template literal, and one would end it.)
	const shots = [...document.querySelectorAll(".shot__img, .still__img")];
	if (!shots.length) return { none: true };

	// The structural half first, and the measurement only from a capture that passed it: an <img>
	// carries no font-family to read, so asking it what face it uses would report a broken probe
	// where the answer is the defect itself.
	const inline = shots.filter((shot) => shot.localName === "svg" && shot.querySelector("text"));
	const report = { total: shots.length, foreign: shots.length - inline.length };

	const stack = inline[0]?.getAttribute("font-family");
	if (!stack) return report;
	report.stack = stack;

	const font = \`400 100px \${stack}\`;
	try {
		await document.fonts.load(font);
	} catch (error) {
		return { ...report, error: String(error) };
	}
	await document.fonts.ready;

	const canvas = document.createElement("canvas").getContext("2d");
	canvas.font = font;
	report.loaded = [...document.fonts].filter(
		(face) => stack.includes(face.family) && face.status === "loaded",
	).length;
	report.advance = canvas.measureText("M").width / 100;
	return report;
})()`;

async function capturesAreTypeset(send) {
	const { result } = await send("Runtime.evaluate", {
		expression: captureTypeface,
		awaitPromise: true,
		returnByValue: true,
	});
	const report = result?.value;
	if (!report) return ["could not read how the captures are typeset"];
	if (report.none) return ["no captures on the page to check"];

	const problems = [];
	if (report.foreign) {
		problems.push(
			`${report.foreign} of ${report.total} captures are not inline <svg> with <text> — an ` +
				"SVG in an <img> is its own document and cannot use this page's @font-face",
		);
	}
	if (report.error) {
		problems.push(`the face the captures ask for would not load: ${report.error}`);
		return problems;
	}
	if (!report.stack) {
		// Only reachable when every capture is foreign, which the line above has already said.
		if (!problems.length) problems.push("the captures name no font-family to measure");
		return problems;
	}
	if (!report.loaded) {
		problems.push("the site declares no loaded webfont for the face the captures ask for");
	}
	if (Math.abs(report.advance - CAPTURE_ADVANCE) > 0.005) {
		problems.push(
			`the captures' face advances ${report.advance.toFixed(3)}em where the export is pinned ` +
				`to ${CAPTURE_ADVANCE}em — every row is squeezed or pulled apart by that ratio`,
		);
	}
	return problems;
}

async function main() {
	const chrome = spawn(
		process.env.CHROME ?? "google-chrome-stable",
		[
			"--headless",
			"--disable-gpu",
			"--no-sandbox",
			"--hide-scrollbars",
			`--remote-debugging-port=${PORT}`,
			"about:blank",
		],
		// Kept rather than discarded: when the port never opens, Chrome's own complaint is the only
		// thing that says why, and throwing it away left a failure reading "never came up" with
		// nothing to act on.
		{ stdio: ["ignore", "ignore", "pipe"] },
	);
	let stderr = "";
	chrome.stderr?.on("data", (chunk) => {
		stderr += chunk;
	});
	let exit = null;
	chrome.on("exit", (code, signal) => {
		exit = signal ?? code;
	});

	// A cold runner starts Chrome a lot slower than a warm laptop, and this budget is waiting on a
	// process launch rather than on anything the site does — so it is generous. A wrong answer here
	// fails the build for a reason unrelated to the thing being checked.
	let target;
	for (let attempt = 0; attempt < 240 && !target; attempt++) {
		await sleep(250);
		try {
			const list = await fetch(`http://127.0.0.1:${PORT}/json/list`).then((r) =>
				r.json(),
			);
			target = list.find((t) => t.type === "page");
		} catch {
			/* not up yet */
		}
	}
	if (!target) {
		chrome.kill();
		const why = [
			exit === null ? "still running" : `exited with ${exit}`,
			stderr.trim() || "no output on stderr",
		].join("; ");
		throw new Error(`headless Chrome never opened port ${PORT} (${why})`);
	}

	const { send, problems, close } = await cdp(target.webSocketDebuggerUrl);
	await send("Runtime.enable");
	await send("Log.enable");
	await send("Page.enable");
	// Pinned rather than inherited: a runner that reports `reduce` hides the shine by design, and the
	// pixel probe below would read that as the defect it exists to catch.
	await send("Emulation.setEmulatedMedia", {
		features: [{ name: "prefers-reduced-motion", value: "no-preference" }],
	});

	let failed = 0;
	for (const page of PAGES) {
		problems.length = 0;
		const url = `${BASE}${page.path}`;
		await send("Page.navigate", { url });

		// The real signal rather than a guess at it: a fixed sleep is either wasted on a warm run or
		// too short on a cold one, and what follows wants a page that has finished laying out.
		await poll(send, "document.readyState === 'complete'");

		const missing = [];
		for (const selector of page.expect) {
			const expression = `document.querySelectorAll(${JSON.stringify(selector)}).length`;
			if (!(await poll(send, expression, { scroll: true }))) missing.push(selector);
		}

		// A page with nothing to wait for still gets a beat to surface a late error.
		await sleep(750);

		const bad = [...problems];
		if (page.captures) bad.push(...(await capturesAreTypeset(send)));
		if (page.shine) bad.push(...(await shinePaints(send, page.shine)));
		if (missing.length) {
			bad.push(`missing from the DOM: ${missing.join(", ")}`);
			// What the page looked like when we gave up. A missing element with no error beside it is
			// otherwise indistinguishable from a page that never got the chance to render one, which is
			// how the last failure here cost a re-run to diagnose.
			const { result } = await send("Runtime.evaluate", {
				expression: `JSON.stringify({ readyState: document.readyState, height: document.body.scrollHeight,
					scrollY: Math.round(scrollY), viewport: innerHeight, islands: document.querySelectorAll("astro-island").length })`,
				returnByValue: true,
			});
			bad.push(`page state: ${result?.value ?? "unreadable"}`);
		}
		if (bad.length) {
			failed++;
			console.error(`✗ ${page.path}`);
			for (const problem of bad) console.error(`    ${problem}`);
		} else {
			console.log(`✓ ${page.path}`);
		}
	}

	close();
	chrome.kill();
	for (const text of TEXTS) {
		const bad = await checkText(text);
		if (bad.length) {
			failed++;
			console.error(`✗ ${text.path}`);
			for (const problem of bad) console.error(`    ${problem}`);
		} else {
			console.log(`✓ ${text.path}`);
		}
	}
	if (failed) {
		console.error(`\n${failed} page(s) failed the smoke check.`);
		process.exit(1);
	}
	console.log("\nall pages rendered clean.");
}

main().catch((error) => {
	console.error(error);
	process.exit(1);
});
