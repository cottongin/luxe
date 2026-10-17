---
title: "Appearance"
summary: "Themes, the statusline, icons and fonts, the conversation's layout, diffs and images."
order: 2
group: "Customize"
---

Every setting on this page goes in `~/.config/luxe/config.json`, or a project's `.luxe/config.json`.
`/reload` applies a change without restarting, except to `images`.

## Themes

`/theme` previews each theme as you move through the list. `enter` keeps one and saves it to your
config; `esc` puts back the theme you had.

Luxe's own theme is `luxe`, a warm graphite with a brass accent. Every accent clears a 4.5:1
contrast ratio against the background, and body text 7:1. The others are dark versions of
community favorites and of Omarchy's themes:

| | |
| --- | --- |
| Luxe | `luxe` |
| Editors | `jetbrains` (Darcula), `vscode` (Visual Studio 2017 Dark), `xcode` (Default Dark) |
| Community | `aura`, `ayu`, `catppuccin`, `dracula`, `everforest`, `github` (Dark Dimmed), `gruvbox`, `kanagawa`, `monokai`, `noctis-bordo`, `nord`, `one-dark`, `panda`, `rose-pine`, `tokyo-night`, `vesper`, `zenburn` |
| Omarchy | `ethereal`, `hackerman`, `last-horizon`, `lumon`, `matte-black`, `miasma`, `osaka-jade`, `retro-82`, `ristretto`, `solitude`, `vantablack` |

```json title="~/.config/luxe/config.json"
{ "theme": "nord" }
```

An unknown name falls back to `luxe`, and Luxe says so when the session opens. See them all on
[luxe.dev](https://luxe.dev/#craft).

Code is colored by the theme too: in answers, tool cards, diffs and the preview, keywords take the
theme's `accentSecondary`, strings its `success`, numbers its `warning`, functions its `accent`,
types its `info`, and comments and punctuation its `muted`. Markdown wears the colors it does in an
answer. A theme of your own, or your Omarchy theme, colors code with no extra setup, and the
website's code blocks are colored the same way.

### Change a few colors

`colors` overrides single colors on top of any theme. A value is `#rrggbb`, an ANSI color number
(`0` to `255`), or `""` for the terminal's own default.

```json title="~/.config/luxe/config.json"
{
  "colors": {
    "accent": "#7aa2f7",
    "diffAddBg": "#12331f",
    "diffDelBg": "#3a1620"
  }
}
```

The names are the ones a theme file uses; [`luxe.json`](https://github.com/evanwhittaker/luxe/blob/HEAD/crates/luxe-core/src/themes/luxe.json)
lists every one.

One more name is optional: `modePlan`, the color [plan mode](agent.md#plan-then-build) wears on the
composer and the menus and questions around it. Without it, Luxe picks the first of the theme's
other colors that reads as different from its accent.

### Make a theme

A theme is a JSON file in `~/.config/luxe/themes/`, named for the file: `themes/harbor.json` is
`"theme": "harbor"`, and it appears in `/theme` with the built-ins. Its `colors` need only what it
changes; everything else comes from `luxe`. `vars` names colors for the rest of the file to use.

```json title="~/.config/luxe/themes/harbor.json"
{
  "vars": { "harbor": "#2f7d8c" },
  "colors": { "accent": "harbor", "borderAccent": "harbor" }
}
```

This is also how to have a light theme. Start from a copy of
[`luxe.json`](https://github.com/evanwhittaker/luxe/blob/HEAD/crates/luxe-core/src/themes/luxe.json)
for a complete one.

### On Omarchy

Leave `theme` unset, or set it to `omarchy`, and Luxe wears your desktop's theme and changes with
it, within a couple of seconds, in a running session. It uses, in order:

1. A `luxe.json` in the current Omarchy theme. To give every theme one, write a template at
   `~/.config/omarchy/themed/luxe.json.tpl` and Omarchy renders it from each theme's colors.
2. Luxe's built-in theme of the same name, when both are dark.
3. A theme made on the spot from the Omarchy theme's `colors.toml`, with text lifted until it is
   readable. This is how a light Omarchy theme comes out.

A theme you choose with `/theme` stays, whatever the desktop does, unless you choose `omarchy`.
`/theme` saves to your own config, so a theme a project's `.luxe/config.json` sets still wins in
that project.

## Statusline

The statusline is the bar under the composer. Its modules are listed in the order they are drawn:
the left half from the edge inward, the right half from the middle outward.

```json title="~/.config/luxe/config.json"
{
  "statusline": {
    "visibility": "auto",
    "style": "slab",
    "left": ["project", "branch", "pr", "todo", "changes"],
    "right": ["context", "cost", "tokens", "thinking", "model"],
    "thinking_colors": false
  }
}
```

Those are the defaults. Reorder a list to move modules, and leave one out to hide it. `project`,
`branch`, `pr`, `todo`, `changes`, `cwd` and `activity` belong on the left, the rest on the right,
and a module listed on the other side is not drawn. While auto-approve is on, an `auto-approve`
badge sits on the right whatever the list says.

| Module | Shows |
| --- | --- |
| `project` | The project's name |
| `branch` | The Git branch, with `*` when there are uncommitted changes |
| `pr` | The branch's pull request: `#138 · 1 failing · 2 threads`. Shown only when there is one. |
| `todo` | The agent's checklist: `4/9 · Refine the workspace overview` |
| `changes` | How many files changed, and the key for the [preview](interface.md#preview). Click it to open. |
| `cwd` | The current directory, shortened |
| `activity` | What the turn is doing, and for how long |
| `context` | How full the context window is. Yellow past 75%, red past 90%. |
| `cost` | What the session has cost, or on a subscription, what is left of the plan |
| `tokens` | Tokens sent and received, and the input read from the cache |
| `thinking` | The thinking level |
| `model` | The model, or `no model` until you choose one |

| Key | Values |
| --- | --- |
| `visibility` | `auto` (shown when the dock is not beside the conversation), `shown`, `hidden` |
| `style` | `slab` (each module on its own block) or `flat` (plain text). The dock's headings follow it. |
| `thinking_colors` | `true` colors the thinking level from muted (off) to magenta (ultra) |

When the bar runs out of room, it drops modules in this order: `cwd`, `tokens`, `activity`, `cost`,
`todo`, `changes`, `pr`, `branch`, `project`, then `context` and `thinking`. The model goes last.

## Icons and fonts

`glyphs` sets which icons Luxe draws. Every icon has a word beside it, so nothing is lost at any
setting.

| `glyphs` | Draws | Needs |
| --- | --- | --- |
| `auto` | `nerd` when Luxe can confirm your font has every icon, otherwise `unicode` | |
| `nerd` | Nerd Font icons | A Nerd Font |
| `nerd_wide` | The same icons, each given two cells | A Nerd Font whose icons are drawn wide |
| `unicode` | Plain shapes: `■ ► ▪ ○ ●` | UTF-8 |
| `ascii` | Plain punctuation: `/ + ~ = x` | UTF-8, for the rest of the screen |

```json title="~/.config/luxe/config.json"
{ "glyphs": "nerd" }
```

`auto` asks fontconfig (`fc-list`) whether your fonts have every icon, and picks `nerd` in kitty,
which brings its own. On macOS and Windows without fontconfig, it picks `unicode`; set `nerd` if you
have a Nerd Font. If icons sit flush against the word beside them, you have the wide build of your Nerd
Font: switch to its `Mono` build (see [Fonts](install.md#fonts)), or set `nerd_wide`.

Fonts that draw every character Luxe uses, measured against the font files:

| Font | Missing | Width |
| --- | --- | --- |
| JetBrains Mono | `ℹ︎ ↳︎ ↺︎ ⏸︎ ✔︎ ✖︎`, filled in by your system | 0.60 em |
| IosevkaTerm | nothing | 0.50 em |
| Adwaita Mono | nothing | 0.60 em |
| Monaspace | `⏎︎`, used only by `luxe wire` | 0.62 em |

Use IosevkaTerm's **Term** build: plain Iosevka lets some symbols overlap the next character.

## The conversation

```json title="~/.config/luxe/config.json"
{
  "conversation": {
    "alignment": "full",
    "growth": "bottom",
    "thinking": "shown",
    "tool_runs": "fold",
    "stream_reveal": "smooth",
    "activity_bar": "bars"
  }
}
```

| Key | Default | Values |
| --- | --- | --- |
| `alignment` | `full` | `full` uses the whole width. `left` or `center` cap the width at `prose_max_width` for text and `wide_block_max_width` for code and tables. |
| `prose_max_width` | `120` | 72 to 120 columns, for `left` and `center` |
| `wide_block_max_width` | `140` | 72 to 140 columns, for `left` and `center` |
| `growth` | `bottom` | `bottom` grows up from the composer, like a chat. `top` grows down from the top. |
| `thinking` | `shown` | How the model's reasoning appears: `shown`, `collapsed` to one line, or `hidden` |
| `tool_runs` | `fold` | `fold` draws a run of lookups as one line. `cards` gives each call a card. |
| `tool_details` | `errors` | Which tool cards start open: `errors`, `all` or `collapsed` |
| `stream_reveal` | `smooth` | `smooth` paces streaming text evenly. `off` draws text as soon as it comes in. |
| `activity_bar` | `bars` | The running-turn indicator: `bars`, `dots` or `off` |
| `scroll_lines` | `3` | Lines per mouse wheel notch, 1 to 20 |
| `image_max_rows` | `24` | The tallest an image is drawn, 3 to 200 rows |

## Diffs

Tool cards, the preview and Guided Review draw diffs the same way:

```json title="~/.config/luxe/config.json"
{ "diff": { "layout": "auto", "word_wrap": false } }
```

| `layout` | Side by side when each column fits |
| --- | --- |
| `auto` | 44 columns of code |
| `split` | 28 columns |
| `unified` | Never |

`ctrl+b` switches the preview or Guided Review for the session. A change that only adds or only
removes lines is always one column. Within a changed line, the words that changed are bold.

`word_wrap` wraps long lines in Guided Review instead of scrolling sideways, and `w` switches it
there. Tool cards always wrap, and the preview starts wrapped; `alt+w` switches it.

## Images

Screenshots and images the agent reads are drawn in the conversation where the terminal can:

| `images` | Draws | Needs |
| --- | --- | --- |
| `auto` | `pixels` in kitty and Ghostty, `off` elsewhere and inside tmux or screen | |
| `pixels` | The picture | A terminal with the kitty graphics protocol |
| `blocks` | A rough preview in colored blocks | Any terminal with color |
| `off` | A line naming the image, with `open ↗` to view it | |

```json title="~/.config/luxe/config.json"
{ "images": "auto" }
```

Double-click an image, or select its message (`alt+k`) and press `alt+i`, to view it full-window.
`/images` says which setting is in effect and why. A change to `images` applies the next time Luxe
starts.

## The start screen

The start screen's logotype is drawn in your theme's accent, with a slow shine across it.
`"banner_animation": false` holds it still. See [The start screen](interface.md#the-start-screen)
for its styles, and for skipping it.
