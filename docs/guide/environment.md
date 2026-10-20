---
title: "Environment variables"
summary: "The environment variables you can set, and the ones Luxe sets."
order: 3
group: "Reference"
---

An environment variable wins over the same setting in a config file, and a command-line flag wins
over both. A variable that turns something off, such as `LUXE_HERDR=0`, cannot turn back on what a
config file turned off.

## Model and provider

| Variable | Default | Does |
| --- | --- | --- |
| `LUXE_MODEL` | | The model, as an ID or `provider/id` |
| `LUXE_PROVIDER` | | The provider |
| `LUXE_API_KEY` | | A key for the provider the session starts on |
| `LUXE_BASE_URL` | | That provider's endpoint, including its version |
| `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, … | | Each provider's own key; see [Models and providers](providers.md#providers) |
| `LUXE_STREAM_START_TIMEOUT` | `60` | Seconds to wait for a reply to start. `0` waits forever. |
| `LUXE_STREAM_IDLE_TIMEOUT` | `180` | Seconds a reply may go quiet. `0` waits forever. |
| `LUXE_PROVIDER_RETRIES` | `3` | Tries after the first when a provider fails before answering |
| `LUXE_COPILOT_PLAN_USAGE` | `1` | `0` stops reading how much of your GitHub Copilot plan is used |

## Files

| Variable | Does |
| --- | --- |
| `LUXE_CONFIG_DIR` | Where configuration and credentials live, instead of `~/.config/luxe` |
| `LUXE_STATE_DIR` | Where Luxe keeps its state, such as the model you last chose, MCP sign-ins and the logs of commands that only report, instead of `~/.local/state/luxe` |
| `XDG_CONFIG_HOME`, `XDG_STATE_HOME`, `XDG_CACHE_HOME` | Honored on every platform; see [Install](install.md#where-luxe-keeps-files) |

When Luxe works out where its files go, a path in one of these, or in `HOME`, that is not valid
UTF-8 is ignored, as if the variable were unset.

## Model catalog

| Variable | Default | Does |
| --- | --- | --- |
| `LUXE_MODELS_DEV_REFRESH` | | `0` never refreshes, and the built-in catalog still works. `1` refreshes at every start. Unset refreshes once the copy is older than `LUXE_MODELS_DEV_TTL`. |
| `LUXE_MODELS_DEV_URL` | `https://models.dev/api.json` | Refresh from a mirror, as a full URL |
| `LUXE_MODELS_DEV_PATH` | | Keep the catalog in this file. The refresh writes there too, so pair it with `LUXE_MODELS_DEV_REFRESH=0` to pin a file of your own. |
| `LUXE_MODELS_DEV_TTL` | `21600` | Seconds between refreshes |

## Tools and turns

| Variable | Default | Does |
| --- | --- | --- |
| `LUXE_TOOL_TIMEOUT_SECS` | | Replace the time limit of every tool that has one |
| `LUXE_MAX_TOOL_ITERATIONS` | | Most rounds of tool calls in one turn. `0` lifts a configured cap. |
| `LUXE_COMPRESS` | `1` | `0` stops trimming long command output, though output past the size limit is still cut |
| `LUXE_LSP_ON_EDIT` | `1` | `0` stops adding language server errors, and notes about missing tests, after edits |
| `LUXE_SIZE_CHECK` | `1` | `0` stops telling the agent when an edit takes a file past 500 lines or a function past 20 |
| `LUXE_DEBUG_NUDGE` | `1` | `0` stops suggesting the debugger after repeated test failures |
| `LUXE_PLAN_NUDGE` | `1` | `0` stops reminding the agent to update its checklist |
| `LUXE_NESTED_INSTRUCTIONS` | `1` | `0` stops reading the [`AGENTS.md` of folders](project-context.md#folders-the-agent-works-in) the agent works in |
| `LUXE_LAZY_TOOLS` | `1` | `0` sends every tool's description with every request |
| `LUXE_CORE_TOOLS` | | The tools sent with every request, comma-separated; the rest load when needed |
| `LUXE_DAP_STOP_TIMEOUT_SECS` | `30` | How long the debugger waits for a program to stop |
| `LUXE_MAX_DEPTH` | `2` | How deeply Luxe may run inside itself, as with [subagents](recipes.md#subagents) |
| `LUXE_WEB_ALLOW_PRIVATE` | | `1` lets `web_fetch` and `screenshot` reach your local network and files |
| `LUXE_WEB_TIMEOUT_SECS` | `30` | How long one fetch may take. Past 60, raise `LUXE_TOOL_TIMEOUT_SECS` too. |
| `BRAVE_API_KEY`, `TAVILY_API_KEY`, `EXA_API_KEY`, `FIRECRAWL_API_KEY` | | Search providers, and Firecrawl for pages built with JavaScript; see [Tools](tools.md#web) |

## Integrations

| Variable | Does |
| --- | --- |
| `LUXE_EDITOR`, then `VISUAL`, then `EDITOR` | The editor Luxe opens files in |
| `LUXE_FILE_MANAGER`, then `FILE_MANAGER` | The file manager `O` in the dock's file tree opens a folder in. Without either, the first of yazi, lf, ranger and nnn that is installed, then your desktop's own. |
| `LUXE_HERDR` | `0` stops reporting state to [Herdr](recipes.md#run-luxe-in-herdr) |
| `LUXE_UPDATE_CHECK` | `0` turns off the update check |
| `LUXE_JOINABLE` | `0` stops other programs [joining](configuration.md#joining-a-session) this run's session |
| `LUXE_RUNTIME_DIR` | Where a running Luxe keeps the socket and record that let a program join it, instead of `luxe` in `$XDG_RUNTIME_DIR` on Linux or `$TMPDIR` on macOS, or `run` in the state folder |

## Diagnostics

| Variable | Does |
| --- | --- |
| `LUXE_LOG` | What goes in the log, such as `debug` or `luxe_core=debug`. `RUST_LOG` works too. See [Logs](troubleshooting.md#logs). |
| `LUXE_WIRE_LOG` | A file to record every request sent to the model; see [Troubleshooting](troubleshooting.md#see-what-is-sent-to-the-model) |

## Set by Luxe

| Variable | Is |
| --- | --- |
| `LUXE_DEPTH` | How deeply this run is nested inside another Luxe |
| `LUXE_REVIEW_GATE_ACTIVE` | Set for a loop's [gate commands](loop.md#check-each-phase), so a Luxe they start does not run a gate of its own |
| `LUXE_HANDOFF` | For a [launcher](configuration.md#launchers): a file whose contents go in the composer when the program exits |
