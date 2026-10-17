<script lang="ts">
  /**
   * Full-text search over the manual, via Pagefind.
   *
   * Pagefind indexes the *built* site rather than the source, so what it searches is exactly what a
   * reader sees. The index is fetched on first keystroke, not on page load — it is a few hundred
   * kilobytes, and most visits never search.
   */
  type Result = { url: string; meta: { title?: string }; excerpt: string };

  let query = $state("");
  let results = $state<Result[]>([]);
  let active = $state(false);
  let pagefind: { search(q: string): Promise<{ results: { data(): Promise<Result> }[] }> } | null =
    null;

  async function load() {
    if (pagefind) return pagefind;
    // Built into /pagefind/ *after* this bundle, by indexing the output it belongs to. The path is
    // assembled at runtime so neither Vite nor TypeScript tries to resolve a file that cannot exist
    // until the build that produced them both has finished.
    const url = ["", "pagefind", "pagefind.js"].join("/");
    pagefind = await import(/* @vite-ignore */ url);
    return pagefind;
  }

  async function run() {
    if (query.trim().length < 2) {
      results = [];
      return;
    }
    try {
      const engine = await load();
      const found = await engine!.search(query);
      results = await Promise.all(found.results.slice(0, 8).map((r) => r.data()));
    } catch {
      // No index in `astro dev` — the search box is inert there rather than throwing.
      results = [];
    }
  }

  function onKey(event: KeyboardEvent) {
    if (event.key === "Escape") {
      query = "";
      results = [];
      (event.target as HTMLInputElement).blur();
    }
  }
</script>

<svelte:window
  onkeydown={(e) => {
    if (e.key === "/" && document.activeElement?.tagName !== "INPUT") {
      e.preventDefault();
      document.getElementById("docs-search")?.focus();
    }
  }}
/>

<div class="search" class:is-open={active && results.length > 0}>
  <input
    id="docs-search"
    class="search__input"
    type="search"
    placeholder="Search  /"
    aria-label="Search documentation"
    bind:value={query}
    oninput={run}
    onfocus={() => (active = true)}
    onblur={() => setTimeout(() => (active = false), 150)}
    onkeydown={onKey}
  />

  {#if active && results.length > 0}
    <ol class="search__results plate">
      {#each results as result (result.url)}
        <li>
          <a class="search__hit" href={result.url}>
            <span class="search__title">{result.meta.title ?? "Untitled"}</span>
            <span class="search__excerpt">{@html result.excerpt}</span>
          </a>
        </li>
      {/each}
    </ol>
  {/if}
</div>

<style>
  .search {
    position: relative;
  }

  .search__input {
    width: 100%;
    font: inherit;
    font-size: var(--step--1);
    padding: 0.55em 0.8em;
    border: 0;
    background: var(--luxe-user-msg-bg);
    color: var(--luxe-text);
  }

  .search__input::placeholder {
    color: var(--luxe-dim);
  }

  .search__results {
    position: absolute;
    z-index: 6;
    top: calc(100% + 0.4rem);
    left: 0;
    width: max(100%, 22rem);
    max-height: 26rem;
    overflow-y: auto;
    list-style: none;
    margin: 0;
    padding: 1px;
  }

  .search__hit {
    display: grid;
    gap: 0.2rem;
    padding: 0.6rem 0.8rem;
    text-decoration: none;
    background: var(--luxe-user-msg-bg);
  }

  .search__hit:hover {
    background: var(--luxe-selected-bg);
  }

  .search__title {
    color: var(--luxe-accent);
    font-size: var(--step--1);
  }

  .search__excerpt {
    color: var(--luxe-muted);
    font-size: var(--step--1);
    line-height: 1.5;
  }

  .search__excerpt :global(mark) {
    background: none;
    color: var(--luxe-text);
    font-weight: 700;
  }
</style>
