---
title: "Skills"
summary: "Teach the agent a procedure it loads only when it needs it."
order: 2
group: "Extend"
---

A skill is a playbook in a `SKILL.md` file: how your team reviews a migration, cuts a release or
writes a changelog entry. The agent sees each skill's name and description, and loads the full text
only when a task calls for it, so a long skill costs nothing until it is used.

## Write a skill

One folder per skill, with a `SKILL.md` in it:

| Folder | For |
| --- | --- |
| `~/.agents/skills/<name>/` | Every project, and every agent that reads it |
| `~/.config/luxe/skills/<name>/` | Every project (`%APPDATA%\luxe\skills` on Windows) |
| `.agents/skills/<name>/` | This repository, committed with it |
| `.luxe/skills/<name>/` | This project. Git sees it too, since `.luxe/` keeps `skills/` out of what it ignores. |

`.agents/skills/` is where Codex, OpenCode, Pi, GitHub Copilot and the `npx skills` installer put
skills, so one installed for any of them works here too. Luxe reads a repository's `.agents/skills/`
from the top of the repository down to the project. When two skills share a name, whatever its case,
the one lower in the table wins, so a project's skill replaces your own, and between nested
`.agents/skills/` folders the deeper one wins. A skill reached twice through a symbolic link counts
once.

```markdown title=".luxe/skills/release-notes/SKILL.md"
---
name: release-notes
description: >
  Writes the CHANGELOG entry for a user-visible change. Use when a change
  adds, removes or alters something a user would notice.
---

1. Read the diff and the issue it closes.
2. Add one line under `## Unreleased` in CHANGELOG.md, in the past tense,
   naming what changed for the user, not how.
3. Link the pull request at the end of the line.
```

Write the description for the moment the agent decides whether to load it: say when to use it, not
what it contains. Without one, the agent sees only the name. `name` is optional and defaults to the
folder's, and the `---` block has to start on the first line.

A skill folder can also hold reference files. A loaded skill tells the agent its folder, so a
skill that says *see `references/schema.md`* gets read. A skill over 40,000 characters is cut off,
with a note telling the agent to read the rest of the file.

## Use a skill

The agent loads a skill when its description fits the task. To ask for one yourself, mention it:
`@`, `tab` to **Skills**, and `enter`, as in *split this into a stack with @[skill:gh-stack]*.

**Project context** lists the skills Luxe found: type `skills` on the start screen and open it. A
`SKILL.md` placed directly in `skills/`, without its own folder, is skipped, and Luxe names it at
startup. The list in the agent's instructions is read with a session's first message, and again
when you resume one. The `@` picker and the `skill` tool look again every time, so a skill added
mid-session can be mentioned or loaded at once.

## Install a skill

Skills written for other agents use the same format and work as they are. Installing one is copying
its folder:

```bash
git clone --depth 1 https://github.com/github/gh-stack /tmp/gh-stack
mkdir -p ~/.config/luxe/skills
cp -r /tmp/gh-stack/skills/gh-stack ~/.config/luxe/skills/
```

Create `skills/` first, as above. Without it, `cp -r` copies the skill's contents to `skills/`
itself, one level too high.

A skill's text reaches the model as instructions, so read a skill before installing it, as you
would a script. The same goes for a repository you clone: its `.agents/skills/` and
`.luxe/skills/` reach the model without `--trust-project`. Luxe removes comments and invisible
characters from skill files, as it does from [`AGENTS.md`](project-context.md#what-luxe-removes),
but without a notice.
