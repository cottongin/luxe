import type { APIRoute, GetStaticPaths } from "astro";
import { logos, type Logo } from "../../lib/press";

/** One press logo per file, its colors written in at build time. */
export const getStaticPaths = (() =>
  logos.map((logo) => ({ params: { file: logo.file }, props: { logo } }))) satisfies GetStaticPaths;

export const GET: APIRoute<{ logo: Logo }> = ({ props }) =>
  new Response(props.logo.svg(), { headers: { "Content-Type": "image/svg+xml" } });
