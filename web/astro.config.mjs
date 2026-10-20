// @ts-check
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { defineConfig } from "astro/config";
import svelte from "@astrojs/svelte";
import sitemap from "@astrojs/sitemap";
import shikiDarkJson from "./src/assets/generated/shiki-luxe.json" with {
	type: "json",
};

// A JSON import widens `type` to `string`, and Shiki's own type wants the literal `"light" | "dark"`.
// The generator asserts that field is exactly one of those, so this narrows something TypeScript
// cannot see rather than claiming something unchecked.
/** @type {import("shiki").ThemeRegistrationRaw} */
const shikiDark = /** @type {any} */ (shikiDarkJson);

/**
 * The guide's links between its own pages, pointed at the site's routes.
 *
 * `docs/guide/*.md` is read on GitHub and in the terminal as much as here, so it links the way a
 * repository does, `configuration.md#github`. Left alone, that resolved against the page's own URL
 * on the site and led nowhere. A page's route is its file's name, so the rewrite is exact.
 *
 * Needs `@astrojs/markdown-remark`: Astro 7's default Markdown processor takes no rehype plugins.
 */
function guideLinks() {
	/** @param {any} node */
	const walk = (node) => {
		const href = node.type === "element" && node.tagName === "a" ? node.properties?.href : null;
		const page = typeof href === "string" ? href.match(/^([a-z0-9-]+)\.md(#.*)?$/) : null;
		if (page) {
			node.properties.href = `/docs/${page[1] === "index" ? "" : `${page[1]}/`}${page[2] ?? ""}`;
		}
		node.children?.forEach(walk);
	};
	return walk;
}

/** The guide's generated screenshots, `scripts/site-assets.sh` writes them. */
const guideImages = fileURLToPath(new URL("../docs/guide/images/", import.meta.url));

/**
 * The guide's screenshots, inlined: `![The dashboard](images/quickstart-dashboard.svg)`.
 *
 * Written as an ordinary image so it shows on GitHub, but drawn inline here: an SVG behind `<img>`
 * is its own document and cannot see the page's `@font-face`, so every Nerd Font icon in it would
 * be tofu for anyone without the face installed. The alt text doubles as the caption. A remark
 * plugin, so it runs before Astro's own image handling would claim the node.
 */
function guideShots() {
	const escape = (/** @type {string} */ text) =>
		text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/"/g, "&quot;");
	/** @param {any} node */
	const walk = (node) => {
		node.children?.forEach((/** @type {any} */ child, /** @type {number} */ index) => {
			const file = child.type === "image" ? child.url.match(/^images\/([a-z0-9-]+\.svg)$/)?.[1] : null;
			if (!file) return walk(child);
			const alt = escape(child.alt ?? "");
			const svg = readFileSync(guideImages + file, "utf8").replace(
				"<svg ",
				`<svg role="img" aria-label="${alt}" `,
			);
			node.children[index] = {
				type: "html",
				value: `<figure class="guide-shot"><div class="guide-shot__plate">${svg}</div><figcaption>${alt}</figcaption></figure>`,
			};
		});
	};
	return walk;
}

/**
 * A code block's file, from its fence: ` ```json title="~/.config/luxe/config.json" `, shown as a
 * caption above the block.
 *
 * The guide names the file every config example belongs in, so a reader (or an agent) knows where
 * to paste it, and the Rust test `guide_check` loads each named example as that file. Done in Shiki
 * rather than a rehype plugin because Astro highlights after the user's rehype plugins run, so the
 * block they would see is not yet the one that ships.
 */
/** @type {import("shiki").ShikiTransformer} */
const codeTitle = {
	name: "code-title",
	/** @param {any} root */
	root(root) {
		const title = this.options.meta?.__raw?.match(/title="([^"]+)"/)?.[1];
		if (!title) return;
		root.children = [
			{
				type: "element",
				tagName: "figure",
				properties: { className: ["code-file"] },
				children: [
					{ type: "element", tagName: "figcaption", properties: {}, children: [{ type: "text", value: title }] },
					...root.children,
				],
			},
		];
	},
};

/**
 * A code block holding a file tree gets the line height the terminal face's box-drawing glyphs are
 * drawn for, so each `│` meets the one below it instead of leaving a gap between rows.
 */
/** @type {import("shiki").ShikiTransformer} */
const codeTree = {
	name: "code-tree",
	pre(node) {
		if (/[├└]──/.test(this.source)) this.addClassToHast(node, "code-tree");
	},
};

// Static output. The site is a document, not an application: nothing here needs a server at
// request time, and a static bundle is the one deployment that cannot be down for a reason
// specific to us.
export default defineConfig({
	site: "https://luxe.dev",
	integrations: [
		svelte(),
		// The social card and the press screenshots are pages so they can be rendered from the real
		// design system; they're not documents, so a search engine isn't offered them.
		sitemap({
			filter: (page) => !page.includes("/social-card") && !page.includes("/press-shots"),
		}),
	],
	output: "static",
	// Astro 7 flipped the default to 'jsx', which strips whitespace between inline elements the way
	// React does — wrong for a prose site, where a space between `<a>` and the next word is content.
	// `true` keeps the HTML-aware compression this site was built against.
	compressHTML: true,
	markdown: {
		remarkPlugins: [guideShots],
		rehypePlugins: [guideLinks],
		shikiConfig: {
			// luxe's own palette, generated from the same JSON the terminal loads. The docs describe a
			// terminal program; coloring their code in someone else's palette would be a small lie about
			// what it looks like. Every color in it clears 4.5:1 on the code background — asserted where
			// it is generated, because syntax highlighting is the one surface that puts all six accents
			// together, adjacent, in small type.
			themes: { dark: shikiDark },
			// As CSS variables rather than inline colors, so the code sits on the site's own plate.
			defaultColor: false,
			transformers: [codeTitle, codeTree],
		},
	},
	build: {
		// One stylesheet rather than a per-page graph. The whole site is a handful of pages sharing
		// one design system, so splitting it costs a request and saves nothing.
		inlineStylesheets: "auto",
	},
	vite: {
		build: {
			// The assets are already minimal and mostly text; the default 4KB inline threshold would
			// base64 the wordmark into the HTML, where it cannot be cached separately.
			assetsInlineLimit: 0,
		},
		optimizeDeps: {
			// Pre-bundle the player instead of discovering it.
			//
			// It is reached through a dynamic `import()` inside an island, so Vite does not see it while
			// crawling entry points and only optimizes it once something asks — mid-session, which
			// invalidates the module graph it had already served. Chrome re-requests and carries on.
			// Firefox treats the interrupted response as `NS_ERROR_CORRUPTED_CONTENT`, refuses it for a
			// "disallowed MIME type", and the import rejects — so every demo on the page stays blank for
			// the whole session. Naming it here means it is bundled before the first request.
			//
			// `astro dev` only: a built site serves one hashed chunk and was never affected.
			// Every dependency an island reaches, not just the one that failed first. Naming only the
			// player moved the failure to Vite's own runtime chunk: anything discovered *after* the
			// server starts serving triggers the same re-optimization and the same rejected import.
			include: ["asciinema-player", "svelte", "svelte/internal/client"],
		},
	},
});
