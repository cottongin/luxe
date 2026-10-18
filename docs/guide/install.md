---
title: "Install"
summary: "Install Luxe, check your setup, and pick a font."
order: 1
group: "Get started"
---

## Install

On macOS and Linux, the install script puts `luxe` in `~/.local/bin`, or in `$LUXE_INSTALL_DIR`
when it is set, and adds that to your `PATH`:

```bash
curl -fsSL https://luxe.dev/install.sh | sh
```

That address redirects to the script on the [latest
release](https://github.com/evanwhittaker/luxe/releases/latest), the one it built and attested. To
read it before running it, drop the `| sh`.

With Homebrew, on macOS or Linux:

```bash
brew install evanwhittaker/tap/luxe
```

With Cargo, which builds it, so you need Rust 1.89 or newer ([rustup](https://rustup.rs) installs
one):

```bash
cargo install --locked luxe-cli
```

On Windows, in PowerShell:

```powershell
powershell -c "irm https://luxe.dev/install.ps1 | iex"
```

[That script](https://luxe.dev/install.ps1) is two lines: it lets this PowerShell window run the
release's installer, which Windows' default execution policy would stop, then runs it. Nothing
about your policy is saved.

Any of them installs the `luxe` command; `luxe --version` checks it worked.

To update, run `luxe update`, or `/update` inside Luxe. Luxe checks once a day for a newer release
and says so on the start screen; see [Updates](configuration.md#updates) for how it updates and how
to turn the check off.

### Check a download

Every file on a release carries a build attestation, so you can confirm it came from this
repository's release workflow and nowhere else:

```bash
gh attestation verify luxe-cli-x86_64-unknown-linux-gnu.tar.xz --repo evanwhittaker/luxe
```

## Build from source

```bash
git clone https://github.com/evanwhittaker/luxe.git
cd luxe
cargo build --release
./target/release/luxe --version
```

## Requirements

Luxe runs natively on Linux, macOS and Windows. Only a terminal and a model are required.

| | |
| --- | --- |
| **Terminal** | Any current terminal with UTF-8. On Windows, use Windows Terminal. |
| **Model** | An API key, a ChatGPT, Copilot or OpenRouter subscription, or a local server such as LM Studio or Ollama. See [Providers](providers.md). |
| **Git** | For reviewing changes and working in worktrees. Without a repository, Luxe runs with nothing to review. |
| **GitHub CLI** | Optional. `gh`, signed in, for issues and pull requests. |
| **A Nerd Font** | Optional, for icons. See [Fonts](#fonts). |

Luxe also uses these when they are installed:

- `ast-grep`, for structural search.
- A language server per language, for code intelligence. See [Code
  intelligence](tools.md#code-intelligence).
- A debug adapter per language, for the debugger. See [Debugger](tools.md#debugger).

## Check your setup

```bash
luxe doctor
```

`luxe doctor` lists the tools, language servers and debug adapters Luxe found, which ones work, and
how to install what is missing:

```
External programs
  ok    git        git
  ok    gh         git (pull requests)

Language servers
  ok    rust         rust-analyzer
  none  go           tried: gopls
        go install golang.org/x/tools/gopls@latest
```

## Fonts

Luxe draws its icons from a [Nerd Font](https://www.nerdfonts.com/) when your terminal has one.
Without one, it falls back to plain characters on its own, and every icon has a word beside it, so
nothing becomes unreadable.

The screenshots use **JetBrains Mono Nerd Font Mono**:

```bash
brew install --cask font-jetbrains-mono-nerd-font   # macOS
sudo pacman -S ttf-jetbrains-mono-nerd               # Arch
scoop install nerd-fonts/JetBrainsMono-NF-Mono       # Windows, after `scoop bucket add nerd-fonts`
```

Use the **Mono** build of a Nerd Font. The other build draws icons wider than one cell, so they
overlap the character after them. Set it in your terminal by its short family name:

```toml title="alacritty.toml"
[font]
normal = { family = "JetBrainsMono NFM" }
```

The short name matters on Linux desktops that pick their monospace font with a fontconfig rule,
such as Omarchy. Asking for the full name, `JetBrainsMono Nerd Font Mono`, can hand you the
desktop's font instead. To check what a name resolves to:

```bash
fc-match -f '%{file}\n' "JetBrainsMono NFM"
```

JetBrains Mono lacks six symbols Luxe draws (`ℹ︎ ↳︎ ↺︎ ⏸︎ ✔︎ ✖︎`), and your system fills them in
from another font. For every character from one face, use **IosevkaTerm** or **Adwaita Mono**:

```bash
brew install --cask font-iosevka-term-nerd-font    # "IosevkaTerm NFM"
brew install --cask font-adwaita-mono-nerd-font    # "AdwaitaMono Nerd Font Mono"
sudo pacman -S ttf-iosevka-nerd ttf-adwaitamono-nerd
scoop install nerd-fonts/IosevkaTerm-NF-Mono nerd-fonts/AdwaitaMono-NF-Mono
```

To use the wider icons anyway, set `"glyphs": "nerd_wide"` in
[`config.json`](configuration.md), and Luxe gives each icon two cells.

## On Windows

Install [Git for Windows](https://gitforwindows.org/). Luxe finds its `bash` even though it is not
on `PATH`, and the agent needs a POSIX shell for most commands. Without one, commands run through
`cmd.exe`, which handles `cargo test` or `npm run build` but not much else.

If WSL is installed, Luxe skips the `bash.exe` in `System32`, which would run commands inside Linux
against a different filesystem.

Python on Windows is `py` or `python`. `python3` is usually the Microsoft Store's placeholder, so
Luxe tries `py -3` first. For the Python debugger, run `py -m pip install debugpy`. The agent's
commands run Python in UTF-8 mode (`PYTHONUTF8=1`) unless you have set it yourself, so output with
characters outside your code page doesn't crash them.

Windows Terminal keeps a column at the right edge for its scrollbar. To give it to Luxe, set
**Scrollbar visibility** to **Hidden** in your profile's **Appearance** settings.

## Where Luxe keeps files

| | Linux and macOS | Windows |
| --- | --- | --- |
| Configuration and credentials | `~/.config/luxe` | `%APPDATA%\luxe` |
| State | `~/.local/state/luxe` | `%LOCALAPPDATA%\luxe` |
| Cache | `~/.cache/luxe` | `%LOCALAPPDATA%\luxe` |

An `XDG_CONFIG_HOME`, `XDG_STATE_HOME` or `XDG_CACHE_HOME` you have set wins on every platform.
`LUXE_CONFIG_DIR` moves every configuration file, API key and provider sign-in. `LUXE_STATE_DIR`
moves the state Luxe keeps for you: the model you last chose, MCP sign-ins, and the logs of
commands that only report, such as `luxe auth`.

Inside a project, Luxe keeps its sessions, logs and caches in `.luxe/`, with a `.gitignore` that
keeps all of it out of Git except the files a team may want to share: `permissions.json`,
`mcp.json`, `hooks.json` and `skills/`.
