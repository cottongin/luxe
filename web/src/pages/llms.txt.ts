import type { APIRoute } from "astro";
import { guidePages, grouped, markdownHref } from "../lib/guide";

/**
 * The guide's table of contents for a language model, per llmstxt.org: what Luxe is, then every
 * page as a markdown link with its summary, under its group.
 */
export const GET: APIRoute = async ({ site }) => {
  const origin = site?.origin ?? "https://luxe.dev";
  const sections = grouped(await guidePages()).map(
    ({ group, pages }) =>
      `## ${group}\n\n` +
      pages
        .map((page) => `- [${page.data.title}](${origin}${markdownHref(page.id)}): ${page.data.summary}`)
        .join("\n"),
  );
  const body = [
    "# Luxe",
    "",
    "> A coding agent for your terminal: write, review and ship code in one workflow, with Guided Review, Git and GitHub, and any model.",
    "",
    `Every page is plain markdown. The whole guide in one file: ${origin}/llms-full.txt`,
    "",
    sections.join("\n\n"),
    "",
  ].join("\n");
  return new Response(body, { headers: { "Content-Type": "text/plain; charset=utf-8" } });
};
