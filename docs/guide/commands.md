---
title: "Commands and keys"
summary: "The Command Palette, slash commands, Help, every default key, and how to rebind them."
order: 4
group: "Using luxe"
---

## Command Palette and slash commands

Every command is in two places:

- **Command Palette** (`ctrl+k`) opens over whatever you are doing, anywhere, with your draft left
  where it was.
- **Slash commands**: type `/` at the start of an empty composer and the same commands open above
  it.

![The Command Palette, with the commands used last at the top](images/commands-palette.svg)

Both list only what can run where you are, with the five you used last at the top. Type to filter
by name, slash command or a few words each is known by: `exit` finds **Quit**. A pane beside the
list says what the command does and the arguments it takes.

`enter` runs a command. In the slash list, `tab` puts it in the composer so you can add arguments:
`/diff pr 142`, `/dock left`, `/compact the API decisions`.

Past the command, the list shows the arguments it takes, such as `/diff turn` or
`/diff pr <number or URL>`, and narrows as you type. `tab` fills in the one under the cursor.
`enter` runs it when nothing is missing, and fills it in when it still needs a value. `esc` puts the
list away for that command. In the composer, a command luxe knows is highlighted, and so are the
words of it you have typed, like `pr` in `/diff pr 142`. A command that stays plain is misspelled.

## Slash commands

| Command | Does |
| --- | --- |
| `/help` | Every key for this screen, as you have bound them |
| `/model` | Change the model; the session carries on |
| `/thinking` | How hard the model reasons, from Off to Ultra |
| `/mode` | Switch between build and [plan mode](agent.md#plan-then-build): `/mode`, `/mode plan`, `/mode build` (`shift+tab`) |
| `/review` | A [second opinion](agent.md#a-second-opinion) on the uncommitted changes: `/review`, `/review error handling` |
| `/new` | Start a new session. The current one is kept. |
| `/sessions` | Find another session in this project and pick it up; see [Sessions](sessions.md) |
| `/context` | What fills the context window, largest first |
| `/compact` | Summarize the conversation now. Add what to keep: `/compact the API decisions`. |
| `/cost` | What this session has cost, by model and kind of token |
| `/search` | Find text in the conversation |
| `/diff` | Open a [Guided Review](guided-review.md): `/diff`, `/diff turn`, `/diff base main`, `/diff commit <sha>`, `/diff pr 142` |
| `/preview` | [Preview](interface.md#preview) a file, whole or as its diff: `/preview`, `/preview src/app.rs` |
| `/editor` | Open the project in your editor, or a file: `/editor`, `/editor src/app.rs`, `/editor src/app.rs:42` |
| `/pr` | This branch's pull request: status, reviewers and conversation |
| `/github` | Pull requests and issues on GitHub |
| `/stack` | Restack a stack of pull requests |
| `/worktree` | Where this session works. `/worktree remove` ends it and removes its worktree. |
| `/dock` | Show or hide the dock: `show`, `hide`, `auto`, `left`, `right` |
| `/statusline` | Show or hide the statusline: `show`, `hide`, `auto` |
| `/theme` | Preview and choose a theme |
| `/mouse` | Mouse capture `on` or `off` |
| `/images` | What this terminal can draw, and why |
| `/permissions` | Approve every tool call that would ask you: `on`, `off`. Deny rules still apply. |
| `/tools` | Every tool the agent can use this run, and the ones it cannot |
| `/lsp` | Language servers: which started, and what they are doing |
| `/debug` | Debug adapters, and whether each can be reached |
| `/mcp` | MCP servers, and the tools each offers |
| `/config` | Providers, credentials, settings and config files |
| `/settings` | [Settings](configuration.md#settings) as controls: appearance, the conversation, the dock, tools and the agent |
| `/login`, `/logout` | Add or remove a provider's credential |
| `/auth` | Which providers you are signed in to, and how |
| `/reload` | Re-read your config and keys without restarting |
| `/quit` | Leave luxe. The session is already saved. |
| `/update` | Leave luxe and update it, as `luxe update` does |

The Command Palette also lists commands with no slash form, such as **Copy message**, **Swap the
sidebar's side** and, in a review, **Send comments**.

## Help

**Help** (`/help`) shows every key for the screen you are on, grouped and drawn from your own
bindings. In Guided Review it shows the review's keys. Help has no default key, because the obvious
ones do not reach a terminal program reliably. To give it one:

```json title="~/.config/luxe/keybindings.json"
{ "tui.app.toggle_help": ["f1"] }
```

## Default keys

The name in the last column is what `keybindings.json` uses.

**Compose**

| Key | Does | Name |
| --- | --- | --- |
| `enter` | Send, or steer a running turn | |
| `shift+enter` | New line | `tui.editor.new_line` |
| `alt+q` | Queue the message for the next turn | `tui.editor.queue_message` |
| `alt+↑` | Bring waiting messages back | `tui.editor.dequeue_message` |
| `ctrl+esc` | Stop the turn | `tui.editor.cancel` |
| `ctrl+v`, `shift+insert` | Paste | `tui.editor.paste` |
| `alt+a` | Select the whole draft | `tui.editor.select_all` |

`enter` and `alt+enter` are fixed. `ctrl+enter` also sends, as `tui.editor.submit`.

**Read**

| Key | Does | Name |
| --- | --- | --- |
| `ctrl+↑`, `ctrl+↓` | Scroll | `tui.convo.scroll_up`, `tui.convo.scroll_down` |
| `pageup`, `pagedown` | Scroll a page | `tui.convo.page_up`, `tui.convo.page_down` |
| `ctrl+f` | Search the conversation | `tui.convo.search` |
| `alt+k`, `alt+j` | Select the previous or next message | `tui.convo.select_block_up`, `tui.convo.select_block_down` |
| `ctrl+o` | Full tool output | `tui.convo.toggle_tool_output` |
| `alt+i` | View image | `tui.convo.open_image` |
| `alt+y` | Copy message | `tui.convo.copy_message` |
| `ctrl+insert`, `ctrl+shift+c` | Copy selection | `tui.convo.copy_selection` |

**Conversation**

| Key | Does | Name |
| --- | --- | --- |
| `alt+e` | Edit and resend your last message | `tui.convo.edit_resend` |
| `alt+t` | Ask again | `tui.convo.retry` |
| `alt+g` | Replace the last answer | `tui.convo.regenerate` |

An edit replaces your last message and the answer to it, and **Replace the last answer** drops the
old answer, so the model sees only the new version. **Ask again** sends your last message once
more, after the answer if there was one. The replaced messages stay in the session file but leave
the conversation. All three wait for a running turn to finish.

**Workspace**

| Key | Does | Name |
| --- | --- | --- |
| `ctrl+k` | Command Palette | `tui.app.open_palette` |
| `shift+tab` | Switch between build and plan mode, from the composer or the start screen | `tui.app.cycle_mode` |
| `alt+d` | Show or hide the dock | `tui.workspace.toggle_dock` |
| `alt+s` | Show or hide the statusline | `tui.statusline.toggle` |
| `ctrl+p` | Preview | `tui.preview.toggle` |
| `alt+n`, `alt+p` | Next or previous changed file, in the preview | `tui.preview.file_next`, `tui.preview.file_prev` |
| `alt+w` | Wrap long lines, in the preview | `tui.preview.toggle_word_wrap` |
| `alt+o` | Open the file in your editor, in the preview | `tui.preview.open_in_editor` |
| `alt+m` | Show a changed file whole, or its diff, in the preview | `tui.preview.toggle_view` |
| `alt+l` | Ask about the lines you picked, in the preview | `tui.preview.ask_about_selection` |
| `ctrl+t` | Move the keyboard into the dock's file tree, then into the file showing | `tui.workspace.focus_files` |
| `ctrl+g` | Guided Review | `tui.review.open` |
| `ctrl+shift+r` | Reload config and keys | `tui.app.reload` |
| `ctrl+c` | Quit | `tui.app.quit` |

**Guided Review**

| Key | Does | Name |
| --- | --- | --- |
| `c` | Comment on this line | `tui.review.add_finding` |
| `a` | Ask the agent | `tui.review.ask` |
| `alt+enter` | Finish review | `tui.review.finish` |
| `alt+u` | Send comments | `tui.review.send` |
| `alt+c` | All comments | `tui.review.findings` |
| `e` | Edit the selected comment | `tui.review.edit_finding` |
| `x` | Delete the selected unsent comment | `tui.review.delete_finding` |
| `r` | Resolve or reopen the selected comment | `tui.review.resolve_finding` |
| `alt+r` | Reanchor the selected stale comment | `tui.review.reanchor_finding` |
| `]`, `[` | Next or previous chapter | `tui.review.chapter_next`, `tui.review.chapter_prev` |
| `n`, `shift+n` | Next or previous hunk | `tui.review.hunk_next`, `tui.review.hunk_prev` |
| `enter` | Open what the cursor is on | `tui.review.open_selection` |
| `home` | Back to column zero | `tui.review.line_start` |
| `g`, `alt+v` | Show or hide the guide | `tui.review.toggle_guide` |
| `f` | Open or fold collapsed churn | `tui.review.toggle_noise` |
| `ctrl+b` | Side by side, or one column | `tui.review.toggle_split` |
| `w` | Wrap long lines | `tui.review.toggle_word_wrap` |
| `s` | Change what is reviewed | `tui.review.source` |
| `ctrl+r` | Refresh the review | `tui.review.refresh` |
| `o` | Open on GitHub | `tui.review.open_in_browser` |
| `shift+e` | Open the file in your editor, at the cursor's line | `tui.review.open_in_editor` |
| `alt+←` | Back to the conversation | `tui.review.return_chat` |
| `?` | Help | `tui.review.help` |

## Rebind keys

Put your bindings in `~/.config/luxe/keybindings.json`, or open it from **Configuration** with
**Edit configuration files**. Each entry is a name from the tables above and a list of keys:

```json title="~/.config/luxe/keybindings.json"
{
  "tui.app.open_palette": ["ctrl+p"],
  "tui.preview.toggle": ["alt+p"],
  "tui.app.toggle_help": ["f1"]
}
```

- A binding replaces that action's default keys rather than adding to them.
- A key you bind is taken from any action that had it by default.
- A key is modifiers and one key joined by `+`: `ctrl`, `shift`, `alt`, then a letter, a digit,
  `enter`, `esc`, `tab`, `space`, `backspace`, `delete`, `insert`, an arrow (`up`, `down`, `left`,
  `right`), `home`, `end`, `pageup`, `pagedown`, or `f1` to `f12`.
- An unknown name or key is skipped with a warning in the log, and the rest still load. An action
  whose keys are all invalid is left with none, rather than its defaults.
- A project's `.luxe/keybindings.json` applies over yours.

`/reload` or `ctrl+shift+r` applies a change without restarting.

## Alt keys on a Mac

A Mac has no Alt key. Its Option key types characters instead: `option+o` is `ø`, `option+p` is
`π`. Most of luxe's `alt` keys need your terminal to send Option as Alt:

| Terminal | Setting |
| --- | --- |
| Ghostty | `macos-option-as-alt = true` (or `left`) |
| iTerm2 | **Profiles › Keys › Left Option key: Esc+** |
| Terminal | **Settings › Profiles › Keyboard › Use Option as Meta key** |
| kitty | `macos_option_as_alt yes` |
| WezTerm | Left Option already sends Alt; for the right one, `send_composed_key_when_right_alt_is_pressed = false` |
| Alacritty | `option_as_alt = "Both"` under `[window]` |

Ghostty already does this on the "U.S." keyboard layouts. On the others it sends the character and
marks it as an Alt key. On "ABC", the Mac's default English layout, luxe reads that back as the key
you pressed, so `option+o` opens a file from the preview without any setting. On other layouts, set
the option above.
