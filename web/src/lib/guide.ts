import { getCollection, type CollectionEntry } from "astro:content";
import { GROUPS } from "../content.config";

/** A page of the guide. */
export type GuidePage = CollectionEntry<"docs">;

/** Every page, in reading order: group by group, then by `order` within one. */
export async function guidePages(): Promise<GuidePage[]> {
  const rank = (page: GuidePage) => GROUPS.indexOf(page.data.group);
  return (await getCollection("docs")).sort(
    (a, b) => rank(a) - rank(b) || a.data.order - b.data.order,
  );
}

/** The pages under each group, in order, leaving out a group with none. */
export function grouped(pages: GuidePage[]): { group: string; pages: GuidePage[] }[] {
  return GROUPS.map((group) => ({
    group,
    pages: pages.filter((page) => page.data.group === group),
  })).filter((section) => section.pages.length > 0);
}

/** A page's route on the site. */
export const pageHref = (id: string) => (id === "index" ? "/docs" : `/docs/${id}`);

/** A page as plain markdown, for an agent or a copy. */
export const markdownHref = (id: string) => `/docs/${id}.md`;

/**
 * A page as one markdown document: its title as a heading, its summary, then its body.
 *
 * The body's links stay as written, `configuration.md#github`, which is exactly right here: read
 * from `/docs/<page>.md`, a relative link resolves to the other page's markdown.
 */
export function asMarkdown(page: GuidePage): string {
  return `# ${page.data.title}\n\n> ${page.data.summary}\n\n${(page.body ?? "").trim()}\n`;
}
