<script lang="ts">
  /** The install command, per channel, with a copy button. */

  type Channel = { id: string; label: string; command: string; note?: string };

  const channels: Channel[] = [
    { id: "cargo", label: "cargo", command: "cargo install luxe-cli" },
    {
      id: "source",
      label: "source",
      command: "git clone https://github.com/evanwhittaker/luxe && cd luxe && cargo build --release",
    },
  ];

  let active = $state(channels[0]);
  let copied = $state(false);
  let timer: ReturnType<typeof setTimeout>;

  async function copy() {
    await navigator.clipboard.writeText(active.command);
    copied = true;
    clearTimeout(timer);
    timer = setTimeout(() => (copied = false), 1600);
  }
</script>

<div class="install plate">
  <div class="install__inner">
    <div class="install__tabs" role="tablist" aria-label="Install method">
      {#each channels as channel (channel.id)}
        <button
          role="tab"
          aria-selected={active.id === channel.id}
          class="install__tab"
          class:is-active={active.id === channel.id}
          onclick={() => (active = channel)}
        >
          {channel.label}
        </button>
      {/each}
    </div>

    <div class="install__row">
      <code class="install__command"><span class="install__prompt" aria-hidden="true">$</span>{active.command}</code>
      <button class="install__copy" onclick={copy} aria-label="Copy install command">
        {copied ? "copied" : "copy"}
      </button>
    </div>
  </div>
</div>

<style>
  .install {
    width: 100%;
  }

  .install__inner {
    padding: 0;
  }

  .install__tabs {
    display: flex;
    gap: 0;
    border-bottom: 1px solid var(--luxe-border-muted);
  }

  .install__tab {
    font: inherit;
    font-size: var(--step--1);
    letter-spacing: 0.1em;
    padding: 0.75em 1.2em;
    background: none;
    border: 0;
    border-bottom: 2px solid transparent;
    color: var(--luxe-muted);
    cursor: pointer;
    transition: color 150ms var(--ease), border-color 150ms var(--ease);
  }

  .install__tab:hover {
    color: var(--luxe-text);
  }

  .install__tab.is-active {
    color: var(--luxe-accent);
    border-bottom-color: var(--luxe-accent);
  }

  .install__row {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    padding: var(--space-3) var(--space-3) var(--space-3) var(--space-4);
  }

  .install__command {
    flex: 1;
    color: var(--luxe-text);
    font-size: var(--step-0);
    overflow-x: auto;
    white-space: nowrap;
    scrollbar-width: thin;
  }

  .install__prompt {
    color: var(--luxe-dim);
    margin-right: 0.8em;
    user-select: none;
  }

  .install__copy {
    flex: none;
    font: inherit;
    font-size: var(--step--1);
    letter-spacing: 0.1em;
    padding: 0.5em 1em;
    border: 0;
    cursor: pointer;
    background: var(--luxe-selected-bg);
    color: var(--luxe-muted);
    transition: color 150ms var(--ease);
  }

  .install__copy:hover {
    color: var(--luxe-accent);
  }
</style>
