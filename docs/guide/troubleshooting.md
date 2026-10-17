---
title: "Troubleshooting"
summary: "Common problems, where Luxe logs, and what to include in a bug report."
order: 4
group: "Reference"
---

## Start with `luxe doctor`

```bash
luxe doctor
```

It lists the tools, language servers and debug adapters Luxe found, starts each language server to
check that it works, and shows how to install or fix what does not.

## Common problems

| Problem | Fix |
| --- | --- |
| **No model selected**, or `--headless` refuses to start | Choose one with `/model`, or pass `--model provider/id`. Luxe never picks one for you. |
| The model picker is empty | No provider is set up. Open **Configuration** (`c`), or set a provider's key in the environment. |
| A provider keeps failing with `retrying (3/3)` | It is overloaded or unreachable. See [When a provider stalls](providers.md#when-a-provider-stalls) for the time limits. |
| A setting does nothing | Luxe ignores keys it does not know, so check the spelling. `luxe init` and `/settings` write a `config.schema.json` your editor can check against. A project's file cannot change some settings; see [Configuration](configuration.md#what-a-project-cannot-change). |
| Icons show as boxes, or sit flush against their words | See [Fonts](install.md#fonts), and the `glyphs` setting in [Appearance](appearance.md#icons-and-fonts). |
| Your terminal's copy key does nothing | The terminal keeps it. Hold `shift` while dragging, or let the key pass through; see [Mouse and clipboard](interface.md#mouse-and-clipboard). |
| Images do not appear | Your terminal may not draw them. `/images` says why; see [Images](appearance.md#images). |
| A language server is not used | `luxe doctor` says whether it starts. A server a project installed is used only when you are watching; see [Tools](tools.md#a-server-the-project-installed). |
| A project's hooks or MCP servers do not run | They need `--trust-project`, or an interactive session for MCP servers. See [Security](security.md#untrusted-repositories). |
| Luxe opened the wrong folder as the project | Inside a repository, a `.luxe/` between you and its top scopes the project there. Remove it, or start Luxe somewhere else. See [Where the project begins](project-context.md#where-the-project-begins). |
| `/pr` or `/github` says GitHub is unavailable | Install the GitHub CLI and sign in: `gh auth login`. A private repository also needs `gh auth setup-git`. |
| A key binding does not work | Your terminal may not send that key. Rebind it; see [Commands and keys](commands.md#rebind-keys). |
| On a Mac, some `alt` keys type a character such as `ø` or `π` | Your terminal treats Option as a key that types characters. Set it to act as Alt; see [Alt keys on a Mac](commands.md#alt-keys-on-a-mac). |

## Logs

A session logs to `.luxe/logs/luxe.log` in the project. Commands that only report, such as
`luxe auth`, `luxe doctor` and `luxe models`, log to `logs/luxe.log` in the state directory:
`~/.local/state/luxe/` (`%LOCALAPPDATA%\luxe\` on Windows), or wherever `LUXE_STATE_DIR` or
`XDG_STATE_HOME` puts it. On macOS and Linux, a log is readable only by you. One past 5 MB is moved
to `luxe.log.1` the next time Luxe starts, replacing the one before.

The log shows Luxe's own activity at `info` and everything else at `warn`. For more:

```bash
LUXE_LOG=debug luxe                        # everything
LUXE_LOG=luxe_core::hooks=debug luxe       # one part, in detail
```

## See what is sent to the model

`LUXE_WIRE_LOG` records every request Luxe sends and every reply, with credentials removed.
`luxe wire` summarizes it: what fills each request, whether the cached part stays the same between
turns, and whether anything is sent twice.

```bash
LUXE_WIRE_LOG=/tmp/wire.jsonl luxe --headless -p "what does src/main.rs do?"
luxe wire /tmp/wire.jsonl
luxe wire /tmp/wire.jsonl --tools    # with each tool's share
```

The log stays on your machine and is off unless the variable is set.

## Report a bug

Open an issue at [github.com/evanwhittaker/luxe](https://github.com/evanwhittaker/luxe/issues) with:

- `luxe --version`, your operating system and terminal
- What you did, what you expected, and what happened
- The relevant part of `.luxe/logs/luxe.log`

If the interface itself draws wrongly, such as stray characters or a half-drawn screen, record the
raw output, which shows what Luxe actually wrote:

```bash
script -f /tmp/luxe.tty -c luxe       # replay with `cat` or `scriptreplay`
asciinema rec /tmp/luxe.cast -c luxe
```

Attach the recording and the log from the same time.
