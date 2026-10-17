import type { APIRoute } from "astro";
import { guidePages, asMarkdown } from "../lib/guide";

/** The whole guide as one markdown document, in reading order, for a model to take in at once. */
export const GET: APIRoute = async () => {
  const pages = await guidePages();
  const body = pages.map(asMarkdown).join("\n---\n\n");
  return new Response(body, { headers: { "Content-Type": "text/plain; charset=utf-8" } });
};
