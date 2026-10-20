---
title: "Mentions"
summary: "Point the agent at a file, skill, issue, MCP server or your own tracker with `@`."
order: 5
group: "Using luxe"
---

Type `@` at the start of a word in the composer to mention something. The picker opens on your
project's files; `tab` and `shift+tab` move between its tabs, typing filters, `↑`/`↓` move,
`enter` inserts the mention, and `esc` or a space closes it.

![The @ picker on its Files tab, changed files first](images/mentions-picker.svg)

| Tab | Lists |
| --- | --- |
| **Files** | Changed files first, then the rest of the project, 50 rows at most; type to narrow them. Respects `.gitignore`, and never lists `.git`, `target`, `node_modules` or `.cache`. |
| **Skills** | The [skills](skills.md) you have installed |
| **GitHub** | The pull requests and issues `/github` lists, through `gh` |
| **MCP** | The [MCP servers](mcp.md) this session can use, when there are any |
| Your own | Any [mention source](mention-sources.md) you add: Jira, Asana, a runbook index |

Dotfiles and files under dot-folders, such as `.gitignore` and `.github/`, are left off the first
list but show up when you type. Start with `.` to rank them with everything else.

The file list is read from disk each time the picker opens, every few seconds while the Files tab
shows, and when the agent changes files, so a file made or moved a moment ago is there. `ctrl+r` reads it
again on the spot.

## What the agent receives

A mention is a pointer, not a copy. Nothing is pasted into your message:

| Mention | The agent gets |
| --- | --- |
| `@[file:docs/plan.md]` | The path, to read when it needs to |
| `@[skill:gh-stack]` | The skill's name, to load with the `skill` tool |
| `@[file:src/app.rs:40-52]` | The path and those lines, from **ask about lines** in the [preview](interface.md#preview) |
| `@[github:acme/todo/issues/42]` | Its number, title, state and address, and the `git` tool call that reads it in full |
| `@[mcp:asana]` | The server to use, with the names of its tools the first time it is signed in and listing them |
| `@[jira:PROJ-12]` | The text your mention source gave for that row |

A mention typed by hand works too. The title and state of an issue, and a mention source's text,
come from a row you picked this session; for one you typed, the agent gets the address or
`Jira PROJ-12` and looks it up itself. A file or skill that does not exist is reported as missing.

So a large file never floods the conversation, and a file the agent then edits is never in it twice
in two versions. *Review @[file:docs/plan.md] and draft a plan* is enough: the agent opens the file
itself.

In the composer, a mention is one chip. The cursor steps over it, and one `backspace` deletes it.
In `/github`, `c` inserts a mention of the pull request or issue under the cursor.
