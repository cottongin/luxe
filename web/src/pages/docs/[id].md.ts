import type { APIRoute, GetStaticPaths } from "astro";
import { guidePages, asMarkdown, type GuidePage } from "../../lib/guide";

/** Every page of the guide as plain markdown, at `/docs/<page>.md`, for agents and copying. */
export const getStaticPaths = (async () =>
  (await guidePages()).map((page) => ({ params: { id: page.id }, props: { page } }))) satisfies GetStaticPaths;

export const GET: APIRoute = ({ props }) =>
  new Response(asMarkdown((props as { page: GuidePage }).page), {
    headers: { "Content-Type": "text/markdown; charset=utf-8" },
  });
