<script lang="ts">
  /**
   * A recorded terminal session, played rather than described.
   *
   * The recordings are asciicast, not video: a few hundred kilobytes each, sharp at any DPI, and
   * the text inside stays selectable — you can copy a command out of a demo. The player is ~180 KB
   * of JavaScript, which is why this island is `client:visible` and its CSS is imported here rather
   * than in the page's critical path.
   *
   * **Two recordings per demo, and the reason is arithmetic.** A cast is drawn at a fixed size per
   * character, so the column count *is* the type size. The 150 columns that make the workspace
   * legible on a laptop come out at 2.7px a character on a phone, which is the same as not being
   * there; the ~72 that read on a phone waste two thirds of a desktop page. So each script is shot
   * at both sizes and the media query picks one. The two takes are separate sessions against a real
   * model, so they do not say identical things — each is honest about the same feature.
   *
   * **One theme.** A recording bakes its colors in, and they are the colors the page is set in.
   */
  import { onMount, onDestroy } from "svelte";
  import "asciinema-player/dist/bundle/asciinema-player.css";

  interface Props {
    /** URL of the wide recording. */
    src: string;
    /** Columns it was recorded at, which sets the type size. */
    cols: number;
    /** Rows, used to reserve the right box before the player loads. */
    rows: number;
    /** URL of the narrow recording, for phones. */
    mobile?: string;
    /** Columns the narrow recording was shot at. */
    mobileCols?: number;
    mobileRows?: number;
    /** Caption under the frame. */
    caption: string;
  }
  const { src, cols, rows, mobile, mobileCols = 72, mobileRows = 24, caption }: Props = $props();

  /**
   * The row pitch, and it is a measurement rather than a preference.
   *
   * Ghostty draws box-drawing and block characters itself, so a rule runs unbroken down a column no
   * matter how the rows are spaced. A browser cannot: it typesets the glyph, and the glyph is only
   * as tall as the font drew it. JetBrains Mono's `█` measures exactly 1.32em, so any line height
   * above that opens a seam under every row — which is what put hairlines through every border,
   * every composer and every slab in the demos while the same session looked perfect in a real
   * terminal.
   *
   * Set to the glyph, the rules tile. Descenders are not clipped by it because 1.32em is the face's
   * own full extent, ascender to descender, not a tightened version of it.
   */
  const LINE_HEIGHT = 1.32;

  /**
   * JetBrains Mono's advance, as a fraction of its font size. Measured, not guessed: the player
   * sets the terminal to `150ch` and a `ch` here comes out at 0.6 of the font size.
   *
   * It matters because the width below is what *sets* the type size, and a wrong figure reserves
   * the wrong box: the demo then stands taller or shorter than the player draws it and the page
   * jumps when it hydrates.
   */
  const ADVANCE = 0.6;

  /**
   * The largest a cell is allowed to get.
   *
   * Past this a demo stops reading as a terminal on a page and starts reading as an application
   * somebody embedded. The size actually used is usually smaller — the interesting constraint is
   * the `--cell` sum in the stylesheet.
   */
  const MAX_CELL = 10;

  /**
   * How many cell-widths tall a row is: the advance is 0.6 of the type size, and a row is 1.32 of
   * that type size.
   *
   * Written down because the stylesheet solves for a cell that makes the whole demo fit the screen,
   * and a second copy of this arithmetic that disagreed would reserve the wrong box and make the
   * page jump when the player arrives.
   */
  const ROW_CELLS = LINE_HEIGHT / ADVANCE;


  /** Below this the wide cut is unreadable, so the narrow one plays instead. */
  const NARROW_BELOW = 720;

  /**
   * How much of a demo has to be on screen before it starts.
   *
   * `client:visible` hydrates the island the moment its first pixel crosses the viewport, which for
   * something this tall means the demo begins while it is still almost entirely below the fold —
   * the reader arrives partway through a take that is already explaining itself. Waiting for most
   * of it to be in view costs nothing and means the first frame is the first thing seen.
   */
  const PLAY_WHEN_VISIBLE = 0.55;

  let mount: HTMLDivElement;
  let player: { dispose(): void; addEventListener?(event: string, fn: () => void): void } | null =
    null;
  let figure: HTMLElement;
  let started = false;
  let narrow = $state(false);
  let ended = $state(false);
  let query: MediaQueryList | null = null;

  const active = $derived(narrow && mobile ? mobile : src);
  const activeCols = $derived(narrow && mobile ? mobileCols : cols);
  const activeRows = $derived(narrow && mobile ? mobileRows : rows);

  async function create() {
    const { create: createPlayer } = await import("asciinema-player");
    // The player sizes the terminal by *measuring* a character, so measuring before the webfont
    // arrives measures the fallback — and the terminal then sits at the wrong scale for the rest of
    // its life. `font-display: swap` guarantees that race unless we wait it out. Asked for by name:
    // `fonts.ready` only waits for faces already loading, and nothing has drawn the terminal's yet,
    // so on a cold load it resolved at once and Firefox sized the first demo to the fallback.
    await Promise.all(
      ["1em", "bold 1em"].map((style) =>
        document.fonts.load(`${style} "JetBrainsMono Nerd Font Mono"`).catch(() => []),
      ),
    );
    await document.fonts.ready;
    player?.dispose();
    mount.replaceChildren();
    ended = false;
    player = createPlayer(active, mount, {
      // Plays once and stops on its last frame. It used to loop, on the theory that a demo halted
      // on a still reads as a broken image — but these end somewhere worth looking at, and a loop
      // restarting under a reader mid-sentence is worse than a stop. Replay is one button.
      autoPlay: true,
      loop: false,
      controls: false,
      fit: "width",
      terminalFontFamily:
        "'JetBrainsMono Nerd Font Mono', 'AdwaitaMono Nerd Font Mono', 'JetBrains Mono', ui-monospace, monospace",
      terminalLineHeight: LINE_HEIGHT,
    });
    player?.addEventListener?.("ended", () => {
      ended = true;
    });

    // The player measures its wrapper once on create. Our width override lands in the same frame,
    // so nudge it to measure again rather than living with whatever it saw first.
    requestAnimationFrame(() => window.dispatchEvent(new Event("resize")));
  }

  onMount(() => {
    query = window.matchMedia(`(max-width: ${NARROW_BELOW - 1}px)`);
    narrow = query.matches;
    // Re-created rather than re-sourced: the player owns a terminal sized to the old cast, and
    // handing it a different geometry mid-life leaves it drawing at the previous scale.
    const swap = () => {
      const next = query!.matches;
      if (next === narrow) return;
      narrow = next;
      if (started) create();
    };
    query.addEventListener("change", swap);

    // Started when it is actually being looked at, not when the island hydrates. Once only: a demo
    // that restarts every time it scrolls back into view is a demo nobody can leave and return to.
    const watcher = new IntersectionObserver(
      ([entry]) => {
        if (!entry.isIntersecting || started) return;
        started = true;
        watcher.disconnect();
        create();
      },
      { threshold: PLAY_WHEN_VISIBLE },
    );
    watcher.observe(figure);

    return () => {
      query?.removeEventListener("change", swap);
      watcher.disconnect();
    };
  });

  onDestroy(() => player?.dispose());
</script>

<figure
  bind:this={figure}
  class="demo"
  style="--cols: {activeCols}; --rows: {activeRows}; --max-cell: {MAX_CELL}px; --row-cells: {ROW_CELLS}"
>
  <div class="demo__plate plate">
    <div class="demo__frame" style="--aspect: {activeCols * ADVANCE} / {activeRows * LINE_HEIGHT}">
      <div class="demo__mount" bind:this={mount}></div>
    </div>
  </div>
  <figcaption class="demo__caption muted">
    <span>{caption}</span>
    <!-- Only after it has ended: a control offering to restart something already playing is one
         more thing to read past. -->
    {#if ended}
      <button class="demo__replay" type="button" onclick={create}>
        <span aria-hidden="true">↺</span> Replay
      </button>
    {/if}
  </figcaption>
</figure>

<style>
  /* Wide enough for its own columns, even when that is wider than the measure the prose is set to.
     A three-pane surface squeezed into a reading column reads as a diff viewer, which is the one
     thing it is not. Centered with `translate` rather than `transform`, because the section's
     scroll-driven reveal owns `transform` and would overwrite it. */
  /* As large as fits, which turns out to be a question about the screen's *height*.

     One size per character cannot be right twice. Seven pixels left a 150-column demo sitting in
     the middle third of a 2560px display looking like a thumbnail; nine filled that display and
     made the same demo 826px tall, which does not fit a 1366x768 laptop along with its own caption.
     The binding constraint is vertical — rows times the row pitch — so that is what the cell is
     solved for, and the width follows from it.

     The 8.75rem is everything that has to share the screen with the terminal: the plate's padding,
     the caption under it, and air above and below so it is not wedged against the fold. */
  .demo {
    --cell: min(var(--max-cell), calc((100svh - 8.75rem) / (var(--rows) * var(--row-cells))));
    /* Never wider than whatever is holding it.

       This used to cap at `--page` and center itself with `left: 50%; translate: -50%`, which is
       the full-bleed trick and only works when the parent *is* the page. Inside a grid track it
       measured the page, came out wider than its column, and hung off the left edge of the screen.
       A plain `100%` cap and auto margins let the same demo sit in the hero, in a section's media
       track, or in a single stacked column without knowing which. */
    width: min(calc(var(--cols) * var(--cell) + 2 * var(--space-3)), 100%);
    margin-inline: auto;
    position: relative;
  }

  /* The terminal's own background, not the site's raised surface.

     Luxe paints a background into most of its cells rather than leaving them default, so a plate
     in any other color splits the picture in two: painted cells become dark rectangles and
     unpainted ones show the plate through. It reads as a corrupted capture, and it is the reason
     these looked broken in a browser while the recording itself was clean. `--luxe-capture-bg` is
     generated from the dark theme and does not follow the page's mode, because the recording does
     not either. */
  .demo__frame {
    /* Reserve the box before the player exists, or the page jumps when it arrives — but only as a
       minimum: once the player is up it owns its own height, and a hard aspect box would either
       crop it or leave a band of dead canvas under it. */
    min-height: 0;
    aspect-ratio: var(--aspect);
    padding: var(--space-3);
    background: var(--luxe-capture-bg);
  }

  :global(.demo__frame:has(.ap-player)) {
    aspect-ratio: auto;
  }

  .demo__mount {
    width: 100%;
  }

  .demo__caption {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: var(--space-3);
    font-size: var(--step--1);
    margin-top: var(--space-3);
  }

  /* Sits in the caption rather than over the demo: an overlay would cover the last frame, which is
     the one the demo spent half a minute earning. */
  .demo__replay {
    flex: none;
    font: inherit;
    color: var(--luxe-accent);
    background: none;
    border: var(--border);
    border-color: currentcolor;
    padding: 0.15em 0.7em;
    cursor: pointer;
    transition: background-color 0.2s var(--ease);
  }

  .demo__replay:hover,
  .demo__replay:focus-visible {
    background: var(--luxe-user-msg-bg);
  }

  /* The player brings its own palette; point it at the recording's. */
  .demo__frame :global(.ap-player) {
    --term-color-background: transparent;
    --term-color-foreground: var(--luxe-capture-fg);
  }

  /* `.ap-term`, not `.ap-terminal` — the latter is not a class the player has ever defined, so the
     rule silently did nothing and the player's own background sat inside our plate as a second,
     slightly wrong rectangle. */
  .demo__frame :global(.ap-term) {
    background: transparent !important;
    padding: 0;
  }

  /* Fractional advances. Chrome otherwise rounds each glyph's advance to a whole pixel at these
     sizes, so a 120-column line is 120 times the rounding error off the width the player sized it
     for: a demo at 10.86px drew each 6.52px cell as 7 and ran 7% off the right of its frame,
     cutting the model's name at the statusline's edge, while one at 8.68px came up 4% short. Which
     one a reader got depended on their window. */
  .demo__frame :global(.ap-term),
  .demo__frame :global(.ap-term *) {
    text-rendering: geometricPrecision;
  }

  /* `fit: "width"` scales the terminal to its *wrapper*, and the wrapper is content-sized by
     default — so without this the demo renders at whatever the font's natural size happens to be
     and leaves the frame half empty. */
  .demo__frame :global(.ap-wrapper),
  .demo__frame :global(.ap-player) {
    width: 100%;
  }
</style>
