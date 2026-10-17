import { defineCollection } from "astro:content";
import { glob } from "astro/loaders";
// Astro 6 deprecated re-exporting `z` from `astro:content`; pull it from the Zod (v4) it bundles.
import { z } from "astro/zod";

/** Sidebar groups, in order. `scripts/docs-check.py` holds the same list. */
export const GROUPS = ["Get started", "Using Luxe", "Customize", "Extend", "Reference"] as const;

/**
 * The manual, loaded straight out of `docs/guide/` at the repo root.
 *
 * Deliberately not copied into `web/`: the same files are what someone reads on GitHub, what the
 * agent reads when you ask it how to do something, and what this site publishes. A copy would be a
 * third version that drifts from both.
 */
const docs = defineCollection({
	loader: glob({ pattern: "**/*.md", base: "../docs/guide" }),
	schema: z.object({
		title: z.string(),
		summary: z.string(),
		/** Position within its group. Low numbers first; the index page is 0. */
		order: z.number(),
		/** The sidebar group it sits under */
		group: z.enum(GROUPS),
	}),
});

export const collections = { docs };
