/**
 * The press kit's files: logos with their colors written in, and the screenshots offered as PNGs.
 *
 * The site's own `wordmark.svg` takes its colors from page CSS, so on its own, in a slide deck or
 * an article, it draws in black. These are the same generated drawings with the theme's colors
 * filled in from the same generated tokens, so a download can't drift from what the site shows.
 */
import wordmark from "../assets/generated/wordmark.svg?raw";
import mark from "../assets/generated/mark.svg?raw";
import tokens from "../assets/generated/tokens.css?raw";
import counted from "../assets/generated/facts.json";

/**
 * `n` rounded down to a multiple of `step`, for a figure nobody has to keep current.
 *
 * The catalog counts come from `facts.json`, which `scripts/site-assets.sh` counts from the
 * models.dev snapshot, so they move when the snapshot does. Rounded because the point is the
 * breadth, and "8,279" invites a reader to check the last digit.
 */
const atLeast = (n: number, step: number) =>
  (Math.floor(n / step) * step).toLocaleString("en-US");

/** How many models and providers the catalog knows, rounded down. */
export const catalog = {
  models: atLeast(counted.models, 1000),
  providers: atLeast(counted.providers, 100),
};

/** One theme color from the generated tokens, by its name without the `--luxe-` prefix. */
export function token(name: string): string {
  const found = tokens.match(new RegExp(`--luxe-${name}:\\s*(#[0-9a-f]{6})`, "i"));
  if (!found) throw new Error(`tokens.css has no --luxe-${name} color`);
  return found[1];
}

/** `svg` with each `var(--luxe-wordmark-N, currentColor)` fill replaced by `fill(N)`. */
function filled(svg: string, fill: (step: number) => string): string {
  return svg.replace(/var\(--luxe-wordmark-(\d), currentColor\)/g, (_, step) => fill(Number(step)));
}

/** A downloadable logo: its file name, what it's for, and how its preview is shown. */
export interface Logo {
  file: string;
  name: string;
  use: string;
  /** Shown on a light plate rather than the dark one */
  light?: boolean;
  svg: () => string;
}

export const logos: Logo[] = [
  {
    file: "wordmark-brass.svg",
    name: "Wordmark",
    use: "The full logo, in brass. For dark backgrounds.",
    svg: () => filled(wordmark, (step) => token(`wordmark-${step}`)),
  },
  {
    file: "wordmark-light.svg",
    name: "Wordmark, light",
    use: "One color, for dark backgrounds.",
    svg: () => filled(wordmark, () => token("text")),
  },
  {
    file: "wordmark-dark.svg",
    name: "Wordmark, dark",
    use: "One color, for light backgrounds.",
    light: true,
    svg: () => filled(wordmark, () => token("background")),
  },
  {
    file: "mark-brass.svg",
    name: "Mark",
    use: "The L on its own, for small spaces.",
    svg: () => filled(mark, () => token("accent")),
  },
];

/** A screenshot offered as a PNG: the still it's rendered from, and what it shows. */
export interface Screenshot {
  name: string;
  from: "captures" | "generated";
  title: string;
  caption: string;
}

export const screenshots: Screenshot[] = [
  {
    name: "guided-review-pull-request",
    from: "generated",
    title: "Guided Review",
    caption: "A pull request explained one chapter at a time, with its GitHub discussion alongside.",
  },
  {
    name: "peek-beside",
    from: "generated",
    title: "Diff preview",
    caption: "The agent's changes open beside the conversation while it keeps working.",
  },
  {
    name: "github-overview",
    from: "generated",
    title: "GitHub",
    caption: "What a pull request needs before it can merge, with shortcuts to each step.",
  },
  {
    name: "palette",
    from: "captures",
    title: "Command Palette",
    caption: "Every command, searchable, with what each one does.",
  },
];

/** Where a screenshot's PNG is served from. `scripts/press-shots.sh` writes them. */
export const screenshotHref = (shot: Screenshot) => `/press/${shot.name}.png`;
