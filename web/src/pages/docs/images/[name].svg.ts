import type { APIRoute, GetStaticPaths } from "astro";
import { readdirSync, readFileSync } from "node:fs";
import { join, resolve } from "node:path";

/**
 * The guide's screenshots, generated into `docs/guide/images/` by `scripts/site-assets.sh`.
 *
 * Resolved from where Astro runs, `web/`, not from this module: the build bundles it elsewhere.
 */
const directory = resolve("../docs/guide/images");

/**
 * Each screenshot at `/docs/images/<name>.svg`, which is where a link in a page's markdown copy
 * (`/docs/<page>.md`) resolves `images/<name>.svg` to. The rendered pages draw them inline.
 */
export const getStaticPaths = (() =>
  readdirSync(directory)
    .filter((file) => file.endsWith(".svg"))
    .map((file) => ({ params: { name: file.slice(0, -4) } }))) satisfies GetStaticPaths;

export const GET: APIRoute = ({ params }) =>
  new Response(readFileSync(join(directory, `${params.name}.svg`)), {
    headers: { "Content-Type": "image/svg+xml" },
  });
