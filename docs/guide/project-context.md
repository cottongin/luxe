---
title: "Project context"
summary: "Give the agent your project's conventions in AGENTS.md, and how Luxe finds the project."
order: 3
group: "Customize"
---

## AGENTS.md

Put your project's conventions in an `AGENTS.md` at its root, and the agent reads it at the start of
every session. In a project folder a `CLAUDE.md` works too; when both exist, `AGENTS.md` is used.
The global files below are read only by those names, so `~/.claude/CLAUDE.md` is not.

Luxe reads, in order:

1. `~/.agents/AGENTS.md`, the file many people keep for every agent they use
2. `~/.config/luxe/agents.md`, for every project in Luxe
3. `AGENTS.md` at the top of the repository
4. One `AGENTS.md` per folder down to where you started Luxe

So a monorepo can keep shared rules at the root and package rules beside the code, and `luxe`
started in `packages/app` reads both. Nothing above the repository is read. Outside a repository,
only the folder you started in is, so an `AGENTS.md` in a parent folder never reaches a project it
was not written for.

The files are read once, when a session sends its first message, and every later request in the
session repeats that same text, which is what lets the provider reuse its cache. An edit reaches
the agent in the next session: `/new`, or resuming one. If you switch Git branches mid-session, the
agent is told with your next message.

### Folders the agent works in

An `AGENTS.md` in a folder below the repository's top, but not on the way to where you started,
joins when the agent first opens, writes or searches a path in that folder. It's added to that
tool result, once, and Luxe shows **Following** and the file's path. The system prompt doesn't
change, so the cache holds. Ignored folders, such as `node_modules`, are skipped. If a rewind or a
compaction drops the file, it joins again the next time the agent goes there.
`LUXE_NESTED_INSTRUCTIONS=0` turns this off.

**Project context** lists every file a turn reads, and says whose rules each one carries:
**global**, **project** or **folder** context. Type `context` on the start screen and open it.
`enter` opens a file in your editor, or creates one from a template. The template's guidance is in
comments, which Luxe removes and reports, so replace them with your own lines.

### Duplicates

A file the walk reaches twice is read once. That covers a symbolic link, such as
`~/.config/luxe/agents.md` pointing at `~/.agents/AGENTS.md`, and a file that is a byte-for-byte
copy of an earlier one. The context list shows the copy as *read once*.

Two files that say mostly the same thing are both read, because only you can say which sentence
should stay. Keep rules that apply everywhere in one global file, and the project's own rules in its
`AGENTS.md`.

```markdown title="AGENTS.md"
# AGENTS.md

- Run `pnpm test` before calling work done; `pnpm lint --fix` formats.
- Database changes go through a migration in `db/migrations/`, never by hand.
- The API returns errors as `{ "error": { "code", "message" } }`. Keep it that way.
- The current spec is in `docs/SPEC.md`. Read it before planning; skip it for small fixes.
```

### Keep it short

The file is part of every request, and a long one is followed less carefully, not more. For each
line, ask: would a capable model get this wrong without being told, and would that cost something?
Commands, pitfalls and conventions that differ from the usual pass. Directory layouts, dependency
lists and architecture overviews do not; the agent reads those from the code.

- A rule for one part of the tree goes in an `AGENTS.md` beside that code.
- A procedure the agent needs only sometimes goes in a [skill](skills.md).
- A plan, spec or design document stays a file. Name it in `AGENTS.md`, as above, and the agent
  reads it when the work calls for it.

An `AGENTS.md` over 24,000 characters is cut off, with a note telling the agent to read the rest of
the file itself.

### What Luxe removes

Two kinds of text are removed from `AGENTS.md` and skill files before the agent sees them, because
neither is visible to someone reviewing the file:

- Comments outside code blocks: HTML comments, `<!-- … -->`, and link definitions used as
  comments, such as `[//]: # (…)`
- Invisible Unicode: tag characters, supplementary variation selectors, bidirectional overrides,
  and soft hyphens and other marks that draw nothing, written as characters or as references
  such as `&#x202E;`

A file that repeats one link label so often that Luxe cannot check every definition is left out
entirely. For an `AGENTS.md`, Luxe says what it removed or left out when it starts; a skill file is
cleaned without a notice.

## Where the project begins

Luxe works on one project, found by walking up from where you started it:

1. Inside a repository (`.git`, `.jj` or `.hg`), the nearest folder with a `.luxe/` in it, up to
   the top of the repository
2. Otherwise, the top of the repository
3. Outside any repository, the folder you are in

So `luxe` started in `crates/parser/src` opens the whole repository, with its sessions, plan and
reviews. The walk stops at your home folder. A Git worktree is its own project.

To keep a package of a monorepo separate, give it a `.luxe/`:

```bash
mkdir -p packages/app/.luxe
```

The package keeps its own sessions and still reads the `AGENTS.md` at the top of the repository.

Outside a repository, a `.luxe/` claims only its own folder. Luxe writes one wherever it runs, so
starting it once in `~/.config` does not make `~/.config/herdr` part of that project.
