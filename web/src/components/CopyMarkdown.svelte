<script lang="ts">
  /**
   * A page of the guide as markdown: copied, for pasting into an agent, or opened as a file.
   *
   * Copies the served `.md` rather than the page's text, so a prompt gets the source
   * with its code blocks and links intact, not the rendered page flattened.
   */
  let { href }: { href: string } = $props();

  let state = $state<"idle" | "copied" | "failed">("idle");
  let timer: ReturnType<typeof setTimeout>;

  async function copy() {
    try {
      const response = await fetch(href);
      if (!response.ok) throw new Error(String(response.status));
      await navigator.clipboard.writeText(await response.text());
      state = "copied";
    } catch {
      state = "failed";
    }
    clearTimeout(timer);
    timer = setTimeout(() => (state = "idle"), 1600);
  }
</script>

<div class="markdown">
  <button class="markdown__copy" onclick={copy}>
    {state === "copied" ? "Copied" : state === "failed" ? "Copy failed" : "Copy as Markdown"}
  </button>
  <a class="markdown__view" {href}>View as Markdown</a>
</div>

<style>
  .markdown {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    margin-top: var(--space-3);
    font-size: var(--step--1);
  }

  .markdown__copy {
    font: inherit;
    letter-spacing: 0.06em;
    padding: 0.4em 0.9em;
    border: 0;
    cursor: pointer;
    background: var(--luxe-selected-bg);
    color: var(--luxe-muted);
    transition: color 150ms var(--ease);
  }

  .markdown__copy:hover {
    color: var(--luxe-accent);
  }

  .markdown__view {
    color: var(--luxe-muted);
  }
</style>
