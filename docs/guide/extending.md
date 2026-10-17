---
title: "Extending Luxe"
summary: "Which kind of extension you need, how to install one, and what stays stable across updates."
order: 1
group: "Extend"
---

Luxe extends through files and programs, never through code loaded into Luxe itself. A file carries
instructions or settings; a program carries behavior, and runs with the same access you have.
Installing an extension is copying a file.

## Which one you need

| To | Use | Runs |
| --- | --- | --- |
| Change how Luxe looks, or which tools exist | [Configuration](configuration.md) and [themes](appearance.md#make-a-theme) | Nothing |
| Teach the agent a procedure or house style | [Skills](skills.md) and [`AGENTS.md`](project-context.md) | Nothing |
| Point the agent at your tracker or runbooks with `@` | [Mention sources](mention-sources.md) | Your program, when its tab opens |
| Give the agent new tools | [An MCP server](mcp.md) | Your program, when the agent calls it |
| Block, rewrite, audit or react to what the agent does | [Hooks](hooks.md) | Your program, at points in a turn |
| Use an endpoint or model Luxe does not know | [Custom providers](custom-providers.md) | Nothing, or your `!command` for a key |
| Drive Luxe from your own program, or build a new front end: conversation, Guided Review and GitHub | [The RPC protocol](rpc.md) | Your program, driving Luxe |
| Open another terminal tool with a key | [Launchers](configuration.md#launchers) | Your program, on a key |

The rows are in order of effort; the first that fits is usually right. [Recipes](recipes.md) puts
several together: Asana in the `@` picker, subagents, a classifier guard, running in Herdr, and a
local model in LM Studio.

## Where extensions live

```text
~/.config/luxe/
├── config.json           settings, launchers, mention sources
├── keybindings.json      keys
├── models.json           endpoints and models
├── permissions.json      what the agent may do
├── agents.md             instructions for every project
├── skills/
│   └── <name>/           a skill
├── hooks.json            hooks
├── mcp.json              MCP servers
├── themes/
│   └── <name>.json       a theme
├── lsp-servers.json      language servers
└── debug-adapters.json   debug adapters

<project>/
├── .agents/skills/       skills committed with the repository
├── .mcp.json             MCP servers
└── .luxe/
    ├── config.json       settings, less what a project cannot change
    ├── keybindings.json
    ├── skills/
    │   └── <name>/
    ├── mcp.json
    ├── hooks.json        only with --trust-project
    └── permissions.json  can only tighten yours, unless --trust-project
```

What a project's `config.json` cannot change is listed in
[Configuration](configuration.md#what-a-project-cannot-change). On Windows, your own folder is
`%APPDATA%\luxe`, and `LUXE_CONFIG_DIR` moves it anywhere. These paths do not change between
releases.

`.luxe/` writes its own `.gitignore`, which keeps everything in it out of Git except
`permissions.json`, `mcp.json`, `hooks.json` and `skills/`. To share more with your team, such as the
project's `config.json`, write that `.gitignore` yourself: Luxe rewrites its own copy, but leaves
alone one it did not write.

## Trust

**A repository does not choose what runs on your machine.** Its hooks and its loop
`gate.commands` run only with `--trust-project`, and its permission rules can only make yours
stricter. MCP servers, language servers and formatters it brings start, by default, only in an
interactive session or with `--trust-project`; `lsp.project_servers` in your own config changes
that, see [Tools](tools.md#a-server-the-project-installed). This is about which programs Luxe starts. A
language server itself may run project code, as rust-analyzer does with `build.rs`, so open an
unfamiliar repository in Luxe the way you would build it.

**A repository can still write instructions.** Its `AGENTS.md` and skills reach the model. Luxe
removes text a reviewer cannot see, but plain text is still instructions, so read an unfamiliar
repository's `AGENTS.md` as you would its `Makefile`.

## What stays stable across updates

| | Across updates |
| --- | --- |
| `config.json`, `models.json`, `keybindings.json`, `lsp-servers.json`, `debug-adapters.json` | **Only added to.** An old file always loads. An unknown key is ignored, and no key changes meaning. |
| Sessions in `.luxe/sessions/` | A newer Luxe reads every older session. An older Luxe skips what it does not know in a newer one. |
| The RPC protocol | Adding to it changes nothing. Removing or renaming anything changes `protocol_version`. |
| Hook points | Added, never renamed. An unknown point skips that hook alone, with a warning in the log. |
| Tool arguments | A renamed argument still accepts its old name. |
| Skills and `AGENTS.md` | Plain markdown |
| The paths above | Do not move |

Before 1.0, a minor release may change the RPC protocol and the hook points, and its release notes
will say so. Config files and sessions keep the stronger promise regardless, since they are the two
you cannot regenerate.

## What Luxe will not add

- **Plugins loaded into Luxe.** A plugin would have all of Luxe's access, however it was sandboxed.
- **A network listener for RPC.** It would turn a local program into a service that needs
  authentication.
