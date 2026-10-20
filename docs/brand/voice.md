# luxe's voice

How luxe sounds anywhere a person reads it: the TUI, the phone app, the site, the guide, `--help`,
errors and release notes. How the agent talks is set in `luxe-core::context`, not here.

**In one line:** plain, direct and warm. luxe sounds confident because it's good, not because it
says so.

## Principles

- **Say it plainly.** Short words, short sentences. "Start", not "initiate". If a word can go, it
  goes. Read it out loud: if you wouldn't say it to a friend at their desk, rewrite it.
- **Lead with what matters.** The point first, then the detail.
- **Talk to one person.** "You". Never "we" or "users".
- **Show, don't boast.** A fact beats an adjective: "Works with over 8,000 models", not "Works
  with all the best models". CI fails on the marketing words listed in `scripts/voice-check.py`.
- **Use the real name.** Feature names, git terms and key names are what people search for. Explain
  one if you have to; don't swap it for a cuter word.
- **Don't oversell the model.** luxe drives models; it isn't one. Talk about what luxe does: what
  the model sees, what it can touch, how you review what it did.
- **Have some fun, in the right places.** Wordplay belongs in headlines and empty states: "As good
  as it gits.", "Get excited." Never in errors, buttons, permission prompts or anything read when
  something's gone wrong. One per screen at most.

## UI text

The TUI and the phone app use labels, not sentences. People glance at UI; they don't read it.

| What | Do | Don't |
| --- | --- | --- |
| Button | `Review changes` | `Click here to review your changes` |
| Key hint | `ctrl+g to review` | `Press ctrl+g to open a review of what changed.` |
| Status | `Compacting` | `luxe is now compacting your conversation…` |
| Empty state | `No sessions yet. Run luxe in a project to start one.` | `Nothing to see here!` |
| Error | `No shell found. Install Git for Windows and try again.` | `Oops! Something went wrong.` |

- **Buttons start with a verb:** Send, Allow once, Review.
- **Sentence case** for titles and buttons: `Command palette`.
- **Keys in lowercase,** the way the TUI draws them: `ctrl+p`, `alt+enter`, `esc`. A capital means
  Shift, so `G` is Shift+G.
- **No period** on labels, hints or status. A full sentence in a message gets one.
- **Errors say what happened, then what to do.** No "oops", no "sorry", no blame.
- **Empty screens say what to do next.**
- **One word per thing.** Once something has a name, use it everywhere.

## The site

- **Headlines are short statements, with a period:** "A better way to build."
- **Body copy is two or three sentences,** then show it. A screenshot beats a paragraph.
- **Numbers come from the source** (`facts.json`), never typed by hand.

## The guide

People read `docs/guide/` mid-task, and so do the agents they point at it.

- **Answer the question:** how do I, what does, where is. History goes in commit messages.
- **Headings name the task:** "Add an MCP server".
- **A table for options, a list for steps,** prose only for what neither can carry.
- **Name the feature, then its default key:** the **Command Palette** (`ctrl+k`). Keys can be
  rebound; names can't.
- **Every config example names its file** in the fence:
  ` ```json title="~/.config/luxe/config.json" `. A test loads it as that file.
- **Links name the file,** `configuration.md#github`, so they work on GitHub, the site and in the
  terminal. No "see above": link the heading.

`scripts/docs-check.py` checks the links, the frontmatter and a short list of words.

## Words

- **luxe** in prose, `luxe` for the command. Never all caps.
- **Model,** not "AI". **Coding agent,** not "platform" or "solution".
- **American spelling.** `scripts/voice-check.py` checks it, comments included.

## Before you ship it

Read it out loud. If another tool could put its name on the sentence, make it specific. If it sounds
like an ad, cut it.
