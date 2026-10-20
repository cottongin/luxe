---
title: "Command line"
summary: "Every luxe command and flag."
order: 2
group: "Reference"
---

`luxe` with no command opens the interface. `luxe --help`, and `--help` after any command, prints
the same reference. `luxe --version` prints the version.

## Starting a session

| Flag | Does |
| --- | --- |
| `-c`, `--continue` | Resume the most recent session in this project |
| `--resume <id>` | Resume a session by ID; see `luxe sessions list` |
| `--from-pr <number>` | Resume the session that worked on a pull request |
| `--issue <number>` | Start on a GitHub issue, in a worktree of its own |
| `--pr <number>` | Check out a pull request in a worktree and start a session there |
| `--worktree [<branch>]` | Work in a worktree of its own, on a new or named branch |
| `--plan` | Start in [plan mode](agent.md#plan-then-build): read, ask, and write only the plan |

See [Git and GitHub](github.md) and [Sessions and context](sessions.md).

## Model

| Flag | Does |
| --- | --- |
| `--model <id>` | The model, as an ID or `provider/id`, which also picks the provider |
| `--provider <id>` | The provider |
| `--thinking <level>` | `off`, `minimal`, `low`, `medium`, `high`, `xhigh` or `ultra`. An unknown level is ignored with a warning. |
| `--api-key <key>` | The key, or `$VAR`, or `!command`. A key typed here is visible to other users of the machine, so prefer the other two. |
| `--base-url <url>` | The endpoint, including its version, such as `/v1` |

These win over the environment and your config for this run only. See
[Models and providers](providers.md).

## Permissions and tools

| Flag | Does |
| --- | --- |
| `--permission-mode <mode>` | `default`, `strict`, `read-only` or `yolo` |
| `--read-only` | The same as `--permission-mode read-only` |
| `--yolo` | Allow everything, with the guardrails off |
| `--auto-approve` | Start with every prompt approved. Interface and `--rpc` only. |
| `--on-ask <allow\|deny>` | What a prompt becomes with nobody to answer: `allow` by default. Headless and loop only. |
| `--trust-project` | Let a project's hooks, permission rules and servers apply |
| `--no-tool <name>` | Turn a tool off for this run |
| `--with-tool <name>` | Turn on a tool your config turned off |

An unknown `--permission-mode` stops the run with an error. It never falls back to a
more permissive mode.

See [Security and permissions](security.md) and [Tools](tools.md).

## Without the interface

| Flag | Does |
| --- | --- |
| `--headless` | Run one prompt and exit |
| `-p`, `--prompt <text>` | The prompt, for `--headless` |
| `--format <text\|json>` | `text` streams the reply to stdout and tool activity to stderr. `json` writes one event per line, with token usage. |
| `--rpc` | Speak the [RPC protocol](rpc.md) on stdin and stdout |
| `--github-writes` | Let an `--rpc` client publish to GitHub: comments, reviews, thread answers, and opening, merging or restacking pull requests. Refused under `--read-only`. |

```bash
luxe --headless -p "list the TODO comments in src/" --format text
```

A headless run needs `-p` and a model: `--model`, `LUXE_MODEL`, your last choice or `default_model`
in `models.json`. It refuses to start without either. It exits with status 1 when the turn fails or Luxe stops before the turn ends, so a
script can tell a failure from an answer without reading the output.

## Commands

| Command | Does |
| --- | --- |
| `luxe auth login [provider]` | Sign in. `--method api-key` or `subscription` when a provider has both, `--stdin` to read a key from a pipe, `--api-key <key>` to store one given here, `--no-verify` to skip checking it. |
| `luxe auth list` | Every provider and how it is configured |
| `luxe auth status [provider]` | A credential's type, where it came from, and when it expires |
| `luxe auth logout <provider>` | Remove a stored credential |
| `luxe models list` | Models you can use. `--provider <id>` filters, `--configured` lists only the models `models.json` or `auth.json` names. |
| `luxe models show <provider/id>` | What Luxe knows about a model, and where each value came from. A bare ID uses the active provider. |
| `luxe models refresh` | Refresh the model catalog. `--force` even if it is fresh. |
| `luxe sessions list` | Sessions in this project, newest first |
| `luxe join [id]` | Join a session another Luxe is running, speaking the [RPC protocol](rpc.md#join-a-running-session) on stdin and stdout. The id can be its start, or the Luxe's process id; without one, it joins the session running in this folder. |
| `luxe join --list` | Sessions running on this machine that can be joined |
| `luxe remote` | Serve this machine's sessions to your own phone or tablet. Listens on this machine only, for Tailscale Serve to carry to your devices, and lets in this machine's tailnet owner from a paired device. |
| `luxe remote --port <n>` | The port it listens on (default 4519) |
| `luxe remote --allow <login>` | Also let this tailnet login in, as Tailscale names it (`alex@github`). Repeatable. |
| `luxe remote setup` | Check Tailscale, offer to publish the bridge on your tailnet with Tailscale Serve, offer to run it at login as a user service, then show a QR code to pair a device. Each step that changes something outside Luxe asks first. |
| `luxe remote pair` | Show a QR code and link that pair one phone or tablet. They work once, within ten minutes. |
| `luxe remote devices` | The paired devices |
| `luxe remote devices revoke <id>` | Unpair a device. Its open sessions close within a few seconds. |
| `luxe remote service remove` | Stop the bridge's user service and stop it starting at login |
| `luxe mcp list` | Configured MCP servers |
| `luxe mcp add --id <id> --name <name>` | Add a server to the project's `.luxe/mcp.json`, with `--command "<program> <args>"` or `--url <url>`. It counts as a [project's server](mcp.md#servers-a-project-brings). |
| `luxe mcp remove --id <id>` | Remove a server from the project's `.luxe/mcp.json` |
| `luxe mcp tools` | Every server's tools |
| `luxe mcp login --id <id>` | Sign in to a server, in a browser |
| `luxe loop start` | Start or continue an [autonomous run](loop.md). `--max-turns`, `--compact-percent`, `--pause-percent`, `--quiet`. |
| `luxe loop status` | The run's iteration, phase and cost |
| `luxe loop pause`, `luxe loop resume`, `luxe loop stop` | Pause, clear a pause, or stop the run |
| `luxe loop phase` | The current phase. `--set <n>` changes it. |
| `luxe loop fresh` | Reset the run's turn count; see [Autonomous runs](loop.md#start-a-run) |
| `luxe docs [page]` | Read this guide, offline and for the installed version. `--search <words>` finds lines across every page. |
| `luxe doctor` | Check which tools, language servers and debug adapters work |
| `luxe update` | Update Luxe with the command for however it was installed; see [Updates](configuration.md#updates). `--check` only reports. |
| `luxe init` | Set up a theme, a provider, the tools and the start screen, and write them to `~/.config/luxe/config.json`. `--defaults` skips the questions, `--force` starts from an empty file; see [Editing](configuration.md#editing). |
| `luxe wire <log>` | Summarize a `LUXE_WIRE_LOG` capture; see [Troubleshooting](troubleshooting.md#see-what-is-sent-to-the-model). `--tools` breaks the tool schemas' size down per tool. |

Every command also takes the model and permission flags above.
