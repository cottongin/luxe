---
title: "Tools"
summary: "What the agent can use: files, the shell, tests, Git, code intelligence, a debugger and the web."
order: 10
group: "Using luxe"
---

## What the agent can use

| Tool | Does |
| --- | --- |
| `read` | Read files, several at once, with numbered lines. A long file comes back a page at a time. An image is read as a picture for models that can see. |
| `write`, `edit` | Write or change a file. A file must be read before it is edited, and the card shows the diff, after [formatting](#format-on-save). |
| `bash` | Run a shell command. Long output is trimmed, and the untouched output saved under `.luxe/artifacts/`: each stream whole up to 8 MB, or its first and last 4 MB past that. |
| `test` | Run the project's tests and report each failure with its `file:line`, and which tests your change broke. Output is saved as `bash`'s is. |
| `grep`, `glob` | Search file contents and names, with capped output. `grep` names each file once with its matching lines under it, and can say which function or type each match sits in. |
| `ast_grep` | Search and replace by code structure. Needs [ast-grep](https://ast-grep.github.io). |
| `git` | Status, diffs, history, commits, branches, and GitHub pull requests, issues and CI checks through `gh` |
| `todo` | The agent's checklist, shown in the dock's **PLAN** |
| `question` | Ask you a question with choices, in a picker |
| `skill` | Load a [skill](skills.md) |
| `symbol_search`, `read_symbol`, `module_report` | Find symbols across the project and read one without the whole file |
| `impact` | What else a change to a symbol or file affects: callers, dependents and covering tests |
| `lsp_diagnostics`, `lsp_navigation` | Errors and warnings, definitions, references and types, from a [language server](#code-intelligence) |
| `debug` | Drive a [debugger](#debugger): breakpoints, stepping, inspecting variables. Offered when a debug adapter is installed. |
| `web_search`, `web_fetch` | Search the web and read pages; see [Web](#web) |
| `screenshot` | Capture a web page or your own dev server, with a Chromium-based browser, or with your approval, your screen; see [Screen capture](security.md#screen-capture) |
| `mcp` | Call tools on the [MCP servers](mcp.md) you configured, when there are any |

Each tool has to beat the shell command it replaces. [luxe.dev/tools](https://luxe.dev/tools)
shows how, measured.

A call with an argument the tool does not know still runs, and its result names the argument it
ignored, with the one the agent probably meant.

`/tools` lists what the agent can use in the current session, and why any tool is missing. Some tools are sent with every
request; the rest are listed by name and loaded when the agent first needs one, which keeps every
request smaller.

The `test` tool runs tests the way the project does: through its own `.venv` or the package
manager its lockfile names (`uv`, `poetry`, `pnpm`, `yarn` and so on), not whatever happens to be
on `PATH`.

## Turn tools off

The Tools page of [`/settings`](configuration.md#settings) turns each one on or off, or set it in
the file:

```json title="~/.config/luxe/config.json"
{ "tools": { "disabled": ["web_search", "screenshot"] } }
```

A change applies the next time luxe starts, since the tools the agent is offered are fixed for a
run.

For one session, `luxe --no-tool web_search` turns a tool off and `luxe --with-tool web_search`
turns one back on. `read`, `write`, `edit` and `bash` cannot be turned off.

## Format on save

Each file the agent writes or edits goes through its formatter, and the result shows the file as
the formatter left it. That includes a file changed by an applied rename, code action or `ast_grep`
rewrite. rustfmt and gofmt always run. The others run only where the project has
their config, in the file's directory or one above it:

| Language | Formatter | Runs when the project has |
| --- | --- | --- |
| Rust | rustfmt, at the project's edition | always |
| Go | gofmt | always |
| Python | ruff | `ruff.toml`, `.ruff.toml`, or `[tool.ruff]` in `pyproject.toml` |
| TypeScript, JavaScript | biome | `biome.json` or `biome.jsonc` |
| TOML | taplo | `taplo.toml` or `.taplo.toml` |

A formatter that is not installed is skipped. When luxe pipes a file through one (taplo), an empty
answer, or a file that changed again while it ran, leaves the file as it was. To turn formatting
off:

```json title="~/.config/luxe/config.json"
{ "tools": { "format_on_save": false } }
```

## Time limits

Every tool call has a time limit, after which it is stopped: 120 seconds, or 60 for tools that wait
on the network. `bash` defaults to 30 seconds and `test` to 300, and the agent can ask for longer,
up to an hour. `question` waits for you as long as it takes. `LUXE_TOOL_TIMEOUT_SECS` replaces the
120- and 60-second limits; `bash` and `test` keep their own.

`ctrl+esc` stops the whole batch of calls, not just the one running. A call that never started is
reported as having had no effect; one stopped partway says so, since it may have done half its work.

A command that starts something and leaves it running, such as a dev server, returns when the
command itself ends. The server keeps running.

Tools work from the directory luxe was started in. A `cd` inside one `bash` call does not carry over
to the next, so start luxe in the project you want to work on.

## Code intelligence

After every edit, luxe asks the file's language server for errors and warnings and adds the ones
the edit introduced to the result the agent sees, so it can fix what it broke before moving on.
Problems that were already in the file are counted, not listed. The result also names changed code
that no test covers. Set `LUXE_LSP_ON_EDIT=0` to turn both off.

The agent is held to small files and functions: 500 lines and 20 lines. When an edit takes a file
or a function past one of those for the first time, the result says so, so the agent can split it
while the change is fresh. A file that was already long says nothing more. Functions are measured
in Rust, Python, Go, TypeScript and JavaScript; files in any language. Set `LUXE_SIZE_CHECK=0` to turn this off, for a project whose own
conventions differ.

luxe launches these language servers when they are installed:

| Language | Server | Install |
| --- | --- | --- |
| Rust | rust-analyzer | `rustup component add rust-analyzer` |
| TypeScript, JavaScript | typescript-language-server, `tsgo` for TypeScript 7, or `tsc --lsp` | `npm i -g typescript-language-server typescript` |
| Python | pyright, or basedpyright | `npm i -g pyright` |
| Go | gopls | `go install golang.org/x/tools/gopls@latest` |
| C, C++ | clangd | `apt install clangd`, `brew install llvm` |
| Bash | bash-language-server | `npm i -g bash-language-server` |
| Ruby | ruby-lsp, or solargraph | `gem install ruby-lsp` |
| Lua | lua-language-server | `mise use -g lua-language-server` |
| Svelte | svelte-language-server | `npm i -g svelte-language-server` |
| JSON | vscode-json-language-server | `npm i -g vscode-langservers-extracted` |
| YAML | yaml-language-server | `npm i -g yaml-language-server` |

A rename or code action the agent applies writes only inside the project. An edit naming a file
anywhere else, or inside a [fetched repository](#github-repositories), is refused whole. Otherwise
every file is written or none is, and the result says which edits didn't fit their file and which
file operations, such as creating or renaming one, it left undone.

`/lsp` shows which servers are running, and `luxe doctor` starts each one to check it works. luxe
never installs a server itself. When one is missing, the result tells the agent the command that
installs it, which runs through `bash` under your rules like any other.

### A server the project installed

luxe prefers the copy a project installed for itself, in its `node_modules/.bin` or virtual
environment, since that version matches the project. The same goes for formatters such as `ruff`
and `biome`, and for MCP servers a project declares. Runs with nobody watching, such as
`--headless` and the loop, use only what you installed yourself unless you pass `--trust-project`.
To decide for every run, in your own config:

```json title="~/.config/luxe/config.json"
{ "lsp": { "project_servers": "always" } }
```

`auto` (the default) uses a project's servers when you are watching, `always` uses them every time,
and `never` uses only yours. A project's own config cannot change this.

A language server runs your project's build tooling. rust-analyzer runs `build.rs` and procedural
macros, and a TypeScript server loads `tsconfig.json` plugins. For a repository you do not trust,
use a container rather than relying on this setting.

### Add a language server

```json title="~/.config/luxe/lsp-servers.json"
{
  "ruby": {
    "command": "ruby-lsp",
    "extensions": ["rb"],
    "root_markers": ["Gemfile"],
    "install_hint": "gem install ruby-lsp"
  }
}
```

An entry for a language luxe already knows changes only what it names, so
`{ "rust": { "command": "rust-analyzer-nightly" } }` swaps the binary and keeps the rest.
`alternatives` lists other servers to try, in order, when the first will not start.

## Debugger

The `debug` tool drives a real debugger, so the agent can set breakpoints, step through code and
inspect variables rather than add print statements. After three failing test runs in a row, luxe
suggests it. `LUXE_DEBUG_NUDGE=0` turns that suggestion off.

| Language | Adapter |
| --- | --- |
| Python | debugpy (`pip install debugpy`) |
| Rust, C, C++ | CodeLLDB, lldb-dap or gdb |
| Go | Delve (`dlv`) |

`/debug` shows which adapters are installed and any running sessions. To add one, reuse the launch
style of a built-in adapter (`debugpy`, `lldb`, `gdb` or `delve`). This one debugs Zig programs
with `lldb-dap`:

```json title="~/.config/luxe/debug-adapters.json"
[
  {
    "id": "zig",
    "command": ["lldb-dap"],
    "extensions": ["zig"],
    "dialect": "lldb"
  }
]
```

`transport` is `stdio` (the default) or `tcp_dial_in`, for an adapter that connects back to luxe:
put `${DAP_ADDR}` in its `command` where it takes the address, as Delve's
`dlv dap --client-addr ${DAP_ADDR}` does, and luxe replaces it with `host:port`.
`addr_placeholder` names a different placeholder.

A breakpoint the adapter could not set is listed under the launch, with the adapter's reason. An
adapter that fails to start says why, with what it printed.

## Web

`web_search` works without a key. To use a particular search provider, set its key; the first one
set is used:

| Key | Provider |
| --- | --- |
| none | Exa, through its public endpoint |
| `BRAVE_API_KEY` | Brave |
| `TAVILY_API_KEY` | Tavily |
| `EXA_API_KEY` | Exa, on your own account |
| `FIRECRAWL_API_KEY` | Firecrawl |

`web_fetch` reads a page as markdown. If a page refuses, it tries again as a browser, and then, with
`FIRECRAWL_API_KEY` set, in a real browser for pages built with JavaScript.

`web_fetch` and `screenshot` reach the internet and your own machine (`localhost`), so your dev
server works. Other machines on your network, cloud metadata addresses and `file://` are refused,
including through an IPv6 address that carries one of them. A page elsewhere can't redirect a fetch
to your machine, and a fetch reads at most 10 MB. `screenshot` loads only the host you asked for, so
a redirect, frame or image from any other host fails to load, and it runs the browser on a fresh
profile, sandboxed unless luxe runs as root or in a container. `LUXE_WEB_ALLOW_PRIVATE=1` lifts the
address checks, allows `file://` in `screenshot`, and lets it load other hosts. A fetch gives up
after 30 seconds, or `LUXE_WEB_TIMEOUT_SECS`, within the call's 60-second limit.

A search provider's other features, such as crawling a site, are available through its MCP
server; see [MCP](mcp.md).

### GitHub repositories

Given a repository address, such as `https://github.com/owner/repo`, `web_fetch` clones it instead
of reading the page, so the agent can search the real files. The clone is shallow, the latest commit
only, and goes into `.luxe/gh-cache/owner/repo` in your project, which git ignores. Reading it never
asks permission, because it is inside the project.

A repository you did not write sits inside one you did, so luxe keeps it inert:

- Symbolic links are checked out as plain files holding the link's target. A link cannot point the
  agent at your `.env`, or anywhere else outside the clone.
- Git never prompts. A private repository needs your git credentials for github.com, for example
  from `gh auth setup-git`.
- Git LFS files, submodules and tags are not downloaded.
- Language servers and format-on-save do not start there, because rust-analyzer would run the
  repository's `build.rs`.
- Nothing in it runs unless the agent runs it with `bash`, under your permission rules as usual.

A clone over 500 MB on disk, or one that takes longer than 45 seconds, is stopped and removed.
Fetching a clone more than a day old clones it again, and a clone nobody has fetched in 14 days is
deleted. To clear them all, delete `.luxe/gh-cache/`.

The files are still text the agent reads, and text can carry instructions, so treat a clone like any
other page from the web. To read repositories as web pages instead:

```json title="~/.config/luxe/config.json"
{ "tools": { "clone_repositories": false } }
```
