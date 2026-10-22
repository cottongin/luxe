---
title: "Security and permissions"
summary: "What the agent may do, the rules and prompts that decide it, secrets, and what luxe sends over the network."
order: 1
group: "Reference"
---

The agent reads and writes files and runs commands on your machine. luxe lets ordinary work through
without asking, keeps a few guardrails on at all times, and gives you rules to tighten the rest.
For real isolation, run it in a [container](#isolation).

## What is allowed by default

Every tool call is **allowed**, **asked** about, or **denied**. With no configuration:

| | |
| --- | --- |
| **Denied** | Reading `.env` and `.env.*` files (`.env.example` is fine). Catastrophic commands: wiping `/`, your home folder or a system folder, `mkfs`, `dd` to a device, fork bombs. Running a command as root; see [below](#commands-that-need-root). The same call with the same arguments a third time in a row: the agent is told it already has the result. |
| **Asked** | Anything outside the project folder. `git push --force`. Committing. Writes to GitHub: opening, merging or commenting on pull requests and issues, and re-running CI. Capturing your screen. |
| **Allowed** | Everything else |

Commands are matched piece by piece, so `cd /tmp && rm -rf /` is caught. These are guardrails, not
a security boundary: a model set on getting around a pattern can. Committing asks because a commit
moves what Guided Review compares against; answer **Allow always** once and it stops asking.

Recursive `rm` has its own check, covering `-r`, `-R`, combined flags, `--recursive` and an
end-of-options `--`. It catches `/`, `/*`, whole-home targets such as `~`, `$HOME` and `${HOME}`,
and protected system folders and their contents. Ordinary build-folder cleanup stays allowed.
`/etc-backup/` isn't `/etc`, so its name alone doesn't trigger this guard.
A Bash allowance or session approval cannot waive this check; `--yolo` disables the default guard.

## Permission modes

```bash
luxe --permission-mode strict
```

| Mode | |
| --- | --- |
| `default` | The defaults above |
| `strict` | Ask before every change: `write`, `edit`, `bash`, `git`, `debug`, a `test` with its own `command`, and an applied `ast_grep` or `lsp_navigation` change |
| `read-only` | Deny every change, and a `test` with its own `command`. Reading, search, code intelligence and detected tests still work. Also `--read-only`. |
| `yolo` | Allow everything, guardrails off. Also `--yolo`. Your own `deny` rules still apply. |

[Plan mode](agent.md#plan-then-build) is a different thing: the session's mode, not a permission
mode. It refuses every change but a write to the plan file, before the permission rules are asked,
so a rule that allows a command does not let plan mode run it. A [review](agent.md#a-second-opinion)
runs in a mode that refuses every change.

## Rules

```json title="~/.config/luxe/permissions.json"
{
  "mode": "default",
  "permission": {
    "bash": { "*": "allow", "rm *": "ask", "git push * --force *": "deny" },
    "git": { "commit*": "allow" },
    "read": { "*": "allow", "*.env": "deny", "*.env.example": "allow" },
    "external_directory": { "~/scratch/**": "allow" },
    "mcp": { "github/*": "ask" }
  }
}
```

- **Keys** are tool names (`bash`, `read`, `edit`, which also covers `write`, `glob`, `grep`, `git`,
  `webfetch`, `websearch`, `screenshot`, `mcp`), plus `external_directory` for paths outside the
  project, `doom_loop` for repeated calls, and `hook_ask` for [hooks](hooks.md) that ask.
- **Values** are `allow`, `ask` or `deny`, or a map from pattern to one of them.
- **Patterns**: `*` matches anything, `?` one character, and `~` your home folder. `git *` also
  matches `git` alone. A `bash` pattern matches the command; a `read` or `edit` pattern, the path;
  an `mcp` pattern, `server/tool`.
- **The most specific pattern wins.**

Bash chains are also checked segment by segment. An allowance for `git *` does not allow
`git status && curl example.com`: the `curl` segment needs its own allowance. A denied segment
blocks the call. Otherwise, if any segment asks, the call asks too. Every segment must be allowed
for the whole chain to run without asking. The matcher is best-effort, not a shell parser.

Tools that run a program, change files or search a folder are judged by the same rules:

| Tool | Also checked as |
| --- | --- |
| `test` with a `command` | `bash`, segment by segment, with its `filter`; a filter that hides a second command is checked too |
| `debug` `launch` | `bash`, as the program and its arguments; a shell's `-c` script is checked as a command of its own |
| `ast_grep` replace with `apply: true` | `edit`, for each path |
| `lsp_navigation` rename or code action with `apply: true` | `edit`, for the file |
| `grep`, `glob`, `ast_grep` | `read`, for the folder or file searched |
| `module_report`, `read_symbol`, `lsp_navigation`, `lsp_diagnostics` | `read`, for each file opened |

Each tool's own rules still apply alongside. A folder outside the project asks, and a named `.env`
is denied even with `all: true`. **Always** on these prompts remembers only what was asked: the
exact command for `test` and `debug`, the folder for one search outside the project, and nothing
for an applied change, since its files aren't known until it runs.

`grep` and `ast_grep` also check every file they find before reading it, so a search of the whole
project skips a nested `.env`; `glob` checks every name before listing it. A file your rules deny is
skipped. One that would ask is read when the run approves asks on its own (headless by default, or
with auto-approve on) and skipped otherwise, since a search can't stop to ask. The result says how
many were skipped; `read` the file to be asked. A walk over a huge tree stops at 100,000 files and
says so.

An applied rename or code action is refused whole if the language server's edit names a file
outside the project or inside a fetched repository. These checks cover what a tool is asked to
open. They don't sandbox a language server, which can index other files and runs the project's own
code.

A project's `.luxe/permissions.json` can only make your rules stricter, never looser, unless you
start luxe with `--trust-project`.

## The permission prompt

When a call needs approval, a prompt opens above the composer:

![A commit waiting for approval, offering to remember git commit*](images/security-permission-prompt.svg)

| Choice | Does |
| --- | --- |
| **Allow once** | Run this call |
| **Allow for this session** | Allow calls like it until you quit |
| **Allow always** | Write the rule to your `~/.config/luxe/permissions.json` |
| **Reject** | Refuse; the agent is told and carries on |

The prompt shows the rule an **Allow always** would write: a command family, such as `cargo *`, or
the whole folder for a path outside the project. A hook's prompt or a chained Bash command offers
only **Allow once** and **Reject**: one command family cannot cover a chain. Choose with the
arrows and `enter`, or a number; `esc` rejects. For a moment after it opens, the prompt ignores
every key but `esc`, so typing meant for the composer cannot answer it.

## When you step away

**Auto-approve** (`/permissions on`, or `--auto-approve`) allows every call that would ask, while
your `deny` rules still hold. The statusline says `auto-approve` for as long as it is on.

**A run with nobody to answer**, such as `--headless`, `luxe loop` or a phase reviewer, allows
every `ask` by default. To refuse them instead, and tell the agent why:

```bash
luxe --headless --on-ask deny -p "…"
```

or `"agent": { "on_ask": "deny" }` in `config.json`. A project can set `deny` but not `allow`.

**Four GitHub actions need a person**: merging a pull request, closing one, submitting a review, and
closing an issue. Auto-approve and unattended runs refuse them. To let an unattended run do one,
allow it in your own rules, such as `"bash": { "gh pr merge *": "allow" }`.

## Joining a session

A session open in the interface can be [joined](rpc.md#join-a-running-session) by another program,
which then sees the conversation and can answer its prompts. That makes who may join a security
question:

- **Only your user account.** The socket sits in a folder only you can open, only you can use it,
  and luxe checks the account of whatever connects. On Windows the named pipe admits your account
  alone, and refuses remote clients.
  Nothing listens on the network.
- **Not the agent.** A program the agent started, at any depth, is turned away, so it can't
  approve its own prompts. luxe checks the joining process's ancestry. It can't see past a program
  that detaches from its parent, such as one started with `setsid` and left running after its
  parent exits, so treat this as a tripwire, not a wall. A command that would do that should be
  something your rules ask about.
- **A guest can't do everything.** A joined client can't switch or end the session, and can't
  publish to GitHub.

Anything running as you can already read your sessions on disk and your credentials, so joining
adds a way to steer, not a way to read. To turn it off, set
[`joinable`](configuration.md#joining-a-session) to `false`.

## luxe remote

[`luxe remote`](remote.md) is the one part of luxe that answers network requests, and it is off
until you run it. It puts your sessions in reach of your phone, so it takes two things to get in:

- **Your Tailscale login.** The bridge listens on this machine only (`127.0.0.1`) and refuses to
  listen anywhere else. Tailscale Serve carries it to your tailnet over HTTPS and adds the login of
  whoever is asking, replacing any the request brought. A request without one didn't come through
  Serve and is refused, and so is a login other than yours or one you `--allow`. luxe never uses
  Tailscale Funnel, which would publish it to the internet.
- **A paired device.** Any program on this machine can reach a local port and claim a login, so
  every session also needs the credential a device gets when it pairs. An invitation works once,
  within ten minutes. luxe keeps only a hash of each invitation and credential, in
  `~/.config/luxe/remote.json`. The device keeps its credential in a cookie scripts can't read,
  sent only over HTTPS and never with a request from another site; a request a page on another
  site makes is refused too.

Revoking a device disconnects it within a few seconds. A device joins a session as a guest, with
the same limits as any [joined client](#joining-a-session), and the interface names it when it joins
and when it answers a prompt. The name is the device's own label: whether a client may answer
prompts at all is still decided by the process checks above.

Notifications are off until you turn them on in the app, one device at a time, and they never
answer anything: a notification says what is waiting and where, such as "Approve bash" over the
folder's name, never the command, and answering takes the app. While any device has them on, the
bridge joins each running session to watch for prompts. It joins watch-only: the terminal doesn't
announce it, and luxe drops anything it sends, so a client nobody sees can't act. Each
notification is sealed so only that device can read it (RFC 8291) and posted to the push service
the device's browser picked; the push service sees when, not what. The bridge posts only to
Apple's, Google's, Mozilla's and Microsoft's push services, so a paired device can't aim it at
anything else on your network. It signs with a key of its own, kept in `remote.json`, and revoking
a device ends its notifications.

The bridge starts luxe only in a folder a device asks for, as you, in a herdr pane when herdr is
already running and as its own `luxe --rpc` otherwise. A packager can build luxe without the
`remote` feature, which leaves out the bridge entirely.

A session is open in one luxe at a time. A second `luxe --resume` of a session the interface
already has, or a resume from the session list, is refused and says which process has it. The lock
is the operating system's, so a crash never leaves a session stuck.

## Secrets

- Credentials are stored in `~/.config/luxe/auth.json`, readable only by you. luxe warns if
  `auth.json` or `models.json` can be read by others.
- A key is found in this order: `--api-key` or `LUXE_API_KEY`, then what you signed in with, then
  the provider's own variable, then a provider block in `auth.json` or `models.json`. For a provider
  luxe does not know, the block comes before `OPENAI_API_KEY`. The order is the same for the
  provider a session starts on and for one you switch to in `/model`.
- A key can be `$VAR` or `!command` rather than the value, so it never sits in a file. A command
  runs when a key is needed, so treat these files like your shell's startup file. It runs from your
  home folder and is stopped after 30 seconds, so a password manager that asks for a passphrase
  needs a graphical prompt.
- Anything that looks like a secret (API keys, tokens, private keys, passwords in URLs) is removed
  from tool output before the model, the transcript, hooks or the log see it. So is every key and
  token luxe itself has used or loaded, whatever it looks like, including `$VAR` and `!command`
  keys and MCP tokens, as long as it is at least 8 characters.
- MCP sign-in tokens are kept in `~/.local/state/luxe/mcp-auth.json`, outside your config folder,
  so a dotfiles repository never carries them.
- An MCP server's `auth` token or OAuth sign-in only travels over `https`, or plain `http` to
  this machine; a server or sign-in endpoint elsewhere on plain `http` is refused. A key you put in
  a server's `headers` is not checked, so use `https` for one. See [MCP](mcp.md#sign-in).
- Anthropic and Google take API keys only, since their terms do not allow their subscriptions in
  other programs.

## Commands that need root

The agent does not run commands as root: `sudo`, `doas`, `su`, `pkexec`, `run0`, and `gsudo` or
`runas` on Windows. A command that needs root needs your attention and consent, and typing it into
your own terminal is how you give both. So when a step needs one, the agent puts the exact command
in a code block, you copy it into another terminal and run it, then tell the agent it is done.

The same goes for a command that waits on you at a terminal, such as a password, a confirmation or
an interactive installer. Commands the agent runs have no terminal and nothing to read from, so one
of those fails or carries on with empty input, rather than waiting where nobody can answer. Most,
such as `sudo`, say what they wanted; a shell's `read` just gets an empty answer.

To allow it anyway, decide ahead of time in your global file. It only helps where no password is
needed, such as a passwordless `sudo` rule for one program:

```json title="~/.config/luxe/permissions.json"
{ "permission": { "bash": { "sudo pacman *": "allow" } } }
```

`--yolo` allows it too. A project's `.luxe/permissions.json` cannot.

## Screen capture

The `screenshot` tool's `url` mode renders a web page and captures only that. Its `fullscreen`,
`region` and `window` modes capture whatever is on your screen, and the image goes to your model
provider, so each **asks** first, and runs with nobody watching refuse them. To turn screen capture
off entirely:

```json title="~/.config/luxe/permissions.json"
{ "permission": { "screenshot": { "fullscreen": "deny", "region": "deny", "window": "deny" } } }
```

## Untrusted repositories

Opening a repository should not run its code. So, without `--trust-project`:

- Its `.luxe/hooks.json` is ignored.
- Its `gate.commands` in `.luxe/config.json` are ignored with a warning. The loop uses your global
  commands instead; a project list needs `--trust-project`.
- Its `.luxe/permissions.json` can only tighten your rules.
- Its MCP servers, language servers and formatters start only in an interactive session; see
  [Tools](tools.md#a-server-the-project-installed).

Its `AGENTS.md` and skills, including the ones it commits in `.agents/skills/`, still reach the
model, because your own project's are the point of the feature. luxe removes text a reviewer
cannot see, comments and invisible Unicode, and leaves out a file it cannot finish checking; see
[What luxe removes](project-context.md#what-luxe-removes). Plain text is still instructions, so read
an unfamiliar repository's `AGENTS.md` as you would its `Makefile`.
Text from GitHub, such as issues and comments, is filtered the same way and given to the agent as
someone else's words, not as instructions. So are MCP servers' descriptions and results.

A language server can run project code itself: rust-analyzer runs `build.rs`. Treat opening a
repository in luxe like building it.

A repository the agent fetches with `web_fetch` is different: you never chose to open it. luxe
clones it into `.luxe/gh-cache/` with symbolic links flattened to plain files, and starts no
language server or formatter there; see [Tools](tools.md#github-repositories).

### Isolation

Rules are a policy, not a sandbox. For hard isolation, run luxe in a container that mounts only the
project:

```bash
docker run --rm -it -v "$PWD":/work -w /work -e ANTHROPIC_API_KEY <image-with-luxe> \
  luxe --headless -p "…"
```

Do not mount your home folder or SSH keys. `--network none` works for everything but the model
provider, the web tools, the catalog refresh and the update check.

## What luxe sends over the network

There is no telemetry. luxe connects only to:

1. The model provider you configured, including the cache refreshes described in
   [Keeping the cache warm](sessions.md#keeping-the-cache-warm) (`"agent": { "cache_warming": "off" }`
   stops them). For GitHub Copilot that includes reading your plan's usage from GitHub every few
   minutes while you work (`LUXE_COPILOT_PLAN_USAGE=0` stops it). For an Azure resource it includes
   listing what the resource has deployed when you open `/model` (`"discover_deployments": false`
   on the provider stops it)
2. Search providers and web pages, when the agent uses `web_search` or `web_fetch`, and GitHub
   when `web_fetch` clones a repository (`"tools": { "clone_repositories": false }` stops it)
3. The models.dev catalog, to refresh model details (`LUXE_MODELS_DEV_REFRESH=0` stops it)
4. The update check, once a day (`"update": { "check": false }` stops it), and when you run
   `luxe update` or `/update`, which then runs your package manager's own update, or for the
   install script, downloads the new release's script from GitHub and runs it
5. GitHub, through your own `gh`, when you, the agent or an [RPC client](rpc.md#github-writes) work
   with pull requests and issues. A client can write there only if you start it with
   `--github-writes`.
6. Your device's push service (Apple's, for an iPhone), when you turn on notifications in the
   [`luxe remote`](remote.md#notifications) app, sealed so only that device can read them
   (turning them off in the app stops it)

None of them carries anything about you or your machine. The update check sends no version,
platform or identifier; it downloads a public file and compares versions locally.

One local report stays on your machine: inside a [herdr](configuration.md#herdr) pane, luxe runs
herdr's own command to tell it whether the agent is working, idle or waiting on you, and the id of
the session for herdr to resume. Nothing is sent outside herdr, and `"herdr": false` stops it.
And a running session takes connections on a local socket only you can open, so another program can
[join it](#joining-a-session); `"joinable": false` stops that.

Apart from notifications, `luxe remote` sends nothing of its own. It answers your paired devices
through Tailscale Serve and runs your own `tailscale` and `herdr` commands; see
[luxe remote](#luxe-remote).

What you post to GitHub is yours alone: reviews, replies and pull requests are posted from a
preview, with nothing added, and no agent tool can post them.

## Supply chain

luxe is written in Rust with `unsafe` code denied throughout, apart from a few small, reviewed
modules that call the operating system directly. One starts each command in a session of its own,
so nothing the agent runs can reach your terminal. On Windows, one keeps the programs luxe starts
off the stdin `--rpc` reads, one ends a command's whole process tree on a timeout, and one locks
the [session pipe](#joining-a-session) to your account. CI checks
every dependency's security advisories and license with `cargo-deny`, builds with `--locked`, and
pins its GitHub Actions to exact commits.
