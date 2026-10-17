---
title: "Overview"
summary: "What Luxe is, and where to find what you need."
order: 0
group: "Get started"
---

Luxe is a coding agent for your terminal. It works in your project with the model you choose,
then walks you through what it changed so you can review it and open a pull request without leaving
the terminal.

New to Luxe? [Install](install.md) it, then follow the [Quickstart](quickstart.md). The rest of the
guide is reference, for when you need it.

## Find what you need

| To | Read |
| --- | --- |
| Connect a model, or switch between them | [Models and providers](providers.md) |
| Find your way around the screen | [The interface](interface.md) |
| Find a command or a key, or rebind one | [Commands and keys](commands.md) |
| Point the agent at a file, issue or skill | [Mentions](mentions.md) |
| Review what the agent changed | [Guided Review](guided-review.md) |
| Work from an issue, open or merge a pull request | [Git and GitHub](github.md) |
| Resume a session, or see what fills the context | [Sessions and context](sessions.md) |
| See what the agent can use | [Tools](tools.md) |
| Let the agent work through a plan on its own | [Autonomous runs](loop.md) |
| Change the theme, keys or statusline | [Configuration](configuration.md) |
| Give the agent your project's conventions | [Project context](project-context.md) |
| Add tools, skills or your own commands | [Extending Luxe](extending.md), [Recipes](recipes.md) |
| Control what the agent may do | [Security and permissions](security.md) |
| Look up a command, flag or environment variable | [Command line](cli.md), [Environment variables](environment.md) |
| Fix something that is not working | [Troubleshooting](troubleshooting.md) |

## Help

- **Help** (`/help`) lists every key for the screen you are on, with any you have rebound.
- `luxe doctor` checks which language servers, debuggers and tools Luxe can use on your machine.
- [Troubleshooting](troubleshooting.md) covers common problems and how to report a bug.
- Problems and questions: [GitHub issues](https://github.com/evanwhittaker/luxe/issues).

## Ask Luxe

This guide is built into Luxe, for the version you have installed. Ask the agent how to do
something in Luxe, such as *how do I add an MCP server?*, and it reads the answer from the guide
rather than guessing. You can read it yourself too:

```bash
luxe docs                  # every page
luxe docs hooks            # one page
luxe docs --search "on_ask"
```

## For agents

Every page is also plain markdown, at the same address with `.md` on the end:
`https://luxe.dev/docs/install.md`. [`llms.txt`](https://luxe.dev/llms.txt) lists every page with a
one-line summary, and [`llms-full.txt`](https://luxe.dev/llms-full.txt) is the whole guide in one
file. Inside Luxe, `luxe docs` serves the same pages offline.
