<script lang="ts">
  /**
   * Every built-in theme, previewed on the real UI, the way `/theme` previews them.
   *
   * A palette is not something you can judge from its name, which is why the terminal's picker
   * repaints the whole app as the cursor moves. The page does the same: pointing at a theme, or
   * tabbing onto it, repaints the dashboard in that palette at once, and clicking pins it so you can
   * move the mouse away and keep looking.
   *
   * The picture arrives as the slot, drawn once at build time by `fixture_svg --palette`: every
   * color in it is `var(--pN, …)`, and `palette` says what each `N` is in each theme. So this sets
   * a dozen custom properties on the frame and the browser repaints the rest. It used to be an HTML
   * mock of the app, which showed the colors and nothing of how Luxe actually uses them.
   *
   * With `cycle`, the gallery takes turns on its own every few seconds, so a reader who never
   * touches it still sees what the themes are. Pointing at a chip holds that theme, pointing at the
   * picture holds whichever is up so it can be read, and a click pins one and ends the turns. It
   * turns only while it is on screen, and never for a reader who asked for reduced motion.
   */
  import { onMount, type Snippet } from "svelte";

  interface Props {
    /** Resolved tokens per theme, generated from `luxe-core`'s theme files. */
    themes: Record<string, Record<string, string>>;
    /** What each of the preview's variables is, per theme (`theme-preview.json`). */
    palette: Record<string, Record<string, string>>;
    /** The variable the preview paints its canvas with, so the frame around it matches. */
    canvas: string;
    /** Take turns through the themes on a timer. */
    cycle?: boolean;
    /** Put the theme chips under the picture rather than over it. */
    chipsBelow?: boolean;
    /** The preview itself. */
    children?: Snippet;
  }
  const {
    themes,
    palette,
    canvas,
    cycle = false,
    chipsBelow = false,
    children,
  }: Props = $props();

  /** How long each theme stays up before the next takes its turn. */
  const TURN_MS = 3500;
  /** How long a theme someone pointed at stays up after the pointer leaves it. */
  const LINGER_MS = 1200;

  // `$derived`, like every other read of the props below: a plain `const` would take the key list
  // once and keep serving it after the prop changed.
  const names = $derived(Object.keys(themes).filter((name) => name in palette));
  let pinned = $state("luxe");
  let pointed = $state<string | null>(null);
  // Held while the pointer is over the picture, and for good once a chip has been clicked
  let reading = $state(false);
  let chosen = $state(false);
  let gallery: HTMLDivElement;

  // Leaving a chip keeps its theme up a moment: a preview that snapped back the instant the pointer
  // left took the picture away while it was still being looked at. Pointing at another chip cancels
  // the wait, and the rotation holds while it runs, since `pointed` is still set.
  let lingering: ReturnType<typeof setTimeout> | undefined;
  const point = (name: string) => {
    clearTimeout(lingering);
    pointed = name;
  };
  const release = () => {
    clearTimeout(lingering);
    lingering = setTimeout(() => (pointed = null), LINGER_MS);
  };

  onMount(() => {
    if (!cycle || matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    let visible = false;
    const watcher = new IntersectionObserver(([entry]) => (visible = entry.isIntersecting), {
      threshold: 0.4,
    });
    watcher.observe(gallery);
    const timer = setInterval(() => {
      if (!visible || document.hidden || pointed || reading || chosen) return;
      const at = names.indexOf(pinned);
      pinned = names[(at + 1) % names.length];
    }, TURN_MS);
    return () => {
      clearInterval(timer);
      clearTimeout(lingering);
      watcher.disconnect();
    };
  });

  const showing = $derived(pointed ?? pinned);

  /** The preview's palette, as inline custom properties on its frame. */
  const style = $derived(
    Object.entries(palette[showing] ?? {})
      .map(([name, hex]) => `--${name}: ${hex}`)
      .concat(`--preview-canvas: var(--${canvas})`)
      .join("; "),
  );
</script>

<!-- Defined once and placed above or below the picture, in document order, so tabbing through the
     chips follows what the eye sees rather than a CSS reordering of it. -->
{#snippet chips()}
  <div class="gallery__chips" role="group" aria-label="Preview a theme">
    {#each names as name (name)}
      <button
        type="button"
        class="chip"
        class:chip--showing={showing === name}
        aria-pressed={pinned === name}
        style={`--chip-bg: ${themes[name].background}; --chip-ink: ${themes[name].accent}`}
        onmouseenter={() => point(name)}
        onmouseleave={release}
        onfocus={() => point(name)}
        onblur={release}
        onclick={() => {
          // Pin *and* point: the pointer is still resting on this chip, and `pointed` outranks
          // `pinned`, so setting only the pin would leave a click that visibly did nothing.
          pinned = name;
          point(name);
          chosen = true;
        }}
      >
        <span class="chip__dot" aria-hidden="true"></span>
        {name}
      </button>
    {/each}
  </div>
{/snippet}

<div class="gallery" bind:this={gallery}>
  {#if !chipsBelow}
    {@render chips()}
  {/if}

  <!-- `aria-live` would announce every hover; the label names the theme on screen instead. -->
  <figure
    class="preview plate"
    onmouseenter={() => (reading = true)}
    onmouseleave={() => (reading = false)}
  >
    <div class="preview__frame" {style} role="img" aria-label="The Luxe dashboard in {showing}">
      {@render children?.()}
    </div>
  </figure>

  {#if chipsBelow}
    {@render chips()}
  {/if}

</div>

<style>
  .gallery {
    display: grid;
    gap: var(--space-4);
  }

  .gallery__chips {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2);
  }

  .chip {
    display: inline-flex;
    align-items: center;
    gap: 0.5em;
    padding: 0.35em 0.8em;
    font: inherit;
    font-size: var(--step--1);
    color: var(--luxe-text);
    background: var(--luxe-user-msg-bg);
    border: 1px solid var(--luxe-border);
    border-radius: 999px;
    cursor: pointer;
    transition:
      border-color 120ms ease,
      transform 120ms ease;
  }

  .chip:hover,
  .chip:focus-visible {
    border-color: var(--luxe-accent);
    transform: translateY(-1px);
  }

  /* Whichever theme is on screen, so a pointed-at chip is the only one lit rather than one of two */
  .chip--showing {
    border-color: var(--luxe-accent);
    color: var(--luxe-accent);
  }

  /* The chip wears the theme it names: its own background, its own accent. */
  .chip__dot {
    width: 0.85em;
    height: 0.85em;
    border-radius: 50%;
    background: var(--chip-bg);
    box-shadow: inset 0 0 0 2px var(--chip-ink);
  }

  .preview {
    margin: 0;
  }

  .preview__frame {
    padding: var(--space-3);
    background: var(--preview-canvas);
    transition: background-color 180ms ease;
  }

  /* `:global`, because the picture arrives as markup this component did not author. Easing the
     fills stops the page flickering as the pointer crosses the row of chips. */
  .preview__frame :global(svg) {
    display: block;
    width: 100%;
    height: auto;
  }

  .preview__frame :global(svg *) {
    transition:
      fill 180ms ease,
      stroke 180ms ease;
  }

  @media (prefers-reduced-motion: reduce) {
    .chip,
    .preview__frame,
    .preview__frame :global(svg *) {
      transition: none;
    }
  }
</style>
