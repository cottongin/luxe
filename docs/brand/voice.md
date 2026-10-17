# Luxe — voice

How Luxe writes, everywhere a person reads it: the site, the README, `--help`, tool descriptions,
error messages, release notes.

This is not the agent's runtime communication contract — that is product behavior and lives in
`luxe-core::context` (`SystemPromptBuilder::build_base_harness`). The two agree, but they are
enforced in different places: that one by the prompt, this one by whoever is writing.

---

## The register

Luxe already has a voice. It is in `AGENTS.md` and `PROGRESS.md`, and it sounds like this:

> Windows is a gate, not a hope.
>
> A test that names a program has named a platform.
>
> The blocker was never the version.
>
> A check that only ever sees correct input proves nothing.
>
> A promotion is a claim, and a wrong one renders as tofu.

Declarative. Specific. Willing to say what it cost. No adjective stacks, no throat-clearing. The job
is to carry that outward, not to invent something friendlier for the front page — a product that
writes one way in its journal and another way in its marketing is telling you which one is true.

---

## Rules

### 1. A claim carries its evidence

Numbers, or nothing. The evidence is what separates a fact from a boast, and Luxe has the facts.

> **Yes** — 2481 tests. The whole suite cross-compiled and run against Windows in twenty-two seconds.
> **No** — Thoroughly tested. Blazingly fast CI.

If a claim has no number behind it, either find one or cut the sentence.

### 2. Name the trade-off

This is the rule that makes the others believable, and it is the least-copied thing about how this
project writes. Every real engineering decision cost something. Say so.

> **Yes** — The chamfer ships in three grades because a terminal either draws the triangle at exact
> cell metrics or invents its own shape. A kitty user sees a one-step bevel until someone runs the
> probe, and never sees a broken glyph.
> **No** — Beautiful, pixel-perfect rendering on every terminal.

A page with no cost on it reads as a page that hasn't been used.

### 3. No marketing adjectives

Banned outright, on top of the runtime list (*leverage, delve, robust, seamless, intricate, pivotal,
realm*): **effortless, magical, blazingly, revolutionary, game-changing, powerful, simply, just,
supercharged, next-generation, unleash, elevate**.

Note *simply* and *just*. They almost always precede something that isn't simple, and they tell the
reader that if it's hard for them, that's their fault.

### 4. Vary sentence length

A paragraph of same-length sentences reads as generated, because it usually is. Land the point on a
short one.

### 5. Second person

"You." Never "we", never "users", never "developers everywhere". Luxe is talking to the one person
reading it.

> **Yes** — Your files live where your platform keeps them.
> **No** — We store user configuration in platform-appropriate locations.

### 6. Headlines are statements, with a full stop

> **Yes** — It runs where you already work.
> **No** — Runs everywhere! · Cross-Platform Support

### 7. Never oversell the model

Luxe drives models; it is not one. Output quality belongs to the model. What Luxe can claim is what
it does with one: what reaches it, what it is allowed to touch, what happens to the result.

> **Yes** — The system prompt stays byte-identical between turns, so the cache prefix holds. You can
> check: `LUXE_WIRE_LOG=… luxe wire`.
> **No** — Luxe writes better code.

### 8. Say the real word, then gloss it

Never swap a precise term for a fuzzy one — the precise term is what the reader will search for.
Keep it and explain it in the same breath.

> **Yes** — A review gate: a command that has to pass before a turn counts as done.
> **No** — A special checkpoint thingy that makes sure everything's okay.

---

## Applying it

**Nouns.** *Harness*, not "platform" or "solution". *Turn*, not "interaction". *Tool*, not
"capability". *Model*, not "AI".

**The name.** "Luxe" in prose, `luxe` in a command. Never "LUXE" outside the logotype.

**Lists.** Prose by default. A list is for genuinely parallel items, and it earns itself when the
items are scannable — a list of three sentences is a paragraph wearing a costume.

**Errors** get the most care, because they are read at the worst moment. State what happened, what
it means, and the one thing to do next. No apology, no blame.

> **Yes** — No POSIX shell found. `bash` needs one for the pipelines a model writes. Install Git for
> Windows and Luxe will find it — it doesn't need to be on `PATH`.
> **No** — Error: shell not found. Please ensure your environment is configured correctly.

**Release notes** lead with what changed for the reader, not with the internal refactor that caused
it.

---

## The guide

`docs/guide/` is read by people using Luxe and by the agents they point at it, usually while they
are in the middle of something. Most of the rules above still hold. These come first.

- **Answer the reader's question.** How do I, what does, where is. Rationale stays only where it
  changes what the reader does; history and internals go in commit messages.
- **Short.** Lead with the task. One example per idea. If a sentence can go, it goes.
- **Labels over prose.** Headings name the task or the thing: *Add an MCP server*, *Statusline
  modules*. A table for options, a list for steps, prose for what neither can carry.
- **Features by name.** Keys can be rebound, so name the feature and give the default key after it:
  **Fix with agent** (`f`), the **Command Palette** (`ctrl+k`).
- **Keys in lower case**, the way a keybindings file spells them and the TUI draws them: `ctrl+p`,
  `alt+enter`, `esc`. A capital means Shift, so `G` is Shift+G and `ctrl+p` never reads as
  Ctrl+Shift+P.
- **Every config example names its file**, in the fence: ` ```json title="~/.config/luxe/config.json" `.
  The site shows it above the block, and a test loads it as that file. A fragment that will not
  load does not get a title, and should rarely exist.
- **Written to be followed by an agent.** Each page opens with what it covers. Headings are tasks.
  Blocks are complete enough to paste. No "see above": link the heading.
- **Links name the file**, `configuration.md#github`, so they work on GitHub, on the site and in the
  terminal.
- **Plain words.** No *lands*, *arrives* or *invariant*; few em dashes. `scripts/docs-check.py`
  holds both, along with the links.

---

## The check

Read it aloud. If a sentence sounds like a landing page, cut it. If a paragraph could describe any
other coding agent with the nouns swapped, it says nothing — replace it with the specific thing Luxe
does that the others don't.
