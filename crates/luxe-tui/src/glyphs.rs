//! glyphs — the icon vocabulary, at three fidelities.
//!
//! Every icon in the TUI is decoration: the words beside it carry the meaning, and a reader whose
//! font has none of these should lose shine, not information, and never meet a `□` for an icon.
//!
//! So each icon is declared once in the `icons!` table below, by *meaning*, with all three of its
//! renderings beside it:
//!
//! - [`GlyphTier::Nerd`] — Material Design icons from a patched font, the look Luxe ships with.
//! - [`GlyphTier::Unicode`] — Geometric Shapes, Arrows and two Dingbats: no private use anywhere,
//!   nothing newer than Unicode 1.1 outside a couple of shapes, and every entry one cell wide.
//! - [`GlyphTier::Ascii`] — plain punctuation, for SSH into a minimal box, a CI log, or a terminal
//!   whose `LANG` is not UTF-8.
//!
//! # Some icons simply go away
//!
//! A fallback shape is only worth drawing where it still tells a reader something. On a tool card it
//! does not: the tool's name is right there in bold, and the card's color and wash already say how the
//! call went — the icon is the bonus, so below the nerd tier it is **omitted** rather than approximated.
//! An empty rendering means exactly that, and [`Icon::prefix`] drops the separator with it so the name
//! sits where it should instead of behind a stray space.
//!
//! # Why the other fallbacks share shapes
//!
//! The nerd tier gives every surface its own silhouette. There is no honest way to reproduce that
//! with shapes a bare font is guaranteed to have, so the fallbacks group by *family* instead — one
//! shape for browsers, one for documents, one for attention, one per tool category. A reader still
//! sorts "file operation" from "search" from "web" at a glance, and the label settles the rest.
//!
//! # Width
//!
//! The trap is not availability, it is width. A private-use icon is one cell; the obvious substitutes
//! often are not — `⚙` and much of Miscellaneous Symbols take emoji presentation in many fonts
//! and render two cells wide, which shears every row they sit in. Hence the narrow palette, and hence
//! the test that measures all three renderings of every icon.
//!
//! Advance width is not the whole story, though. A Nerd Font ships two cuts of every face: the
//! `Mono` one rescales each icon into a single cell, while the other leaves it at its drawn size
//! (up to twice the cell) behind the same one-cell advance, so the icon paints over the space after
//! it. Nothing a terminal reports tells the two apart, so [`GlyphTier::NerdWide`] is the reader's
//! answer: the same shapes, given the second cell they were drawn for. [`icon_gap`] is the one
//! place that decides how much room that is, and every surface goes through it.
//!
//! It applies to **this vocabulary only**. The rest of what the TUI draws — a `✓` on a finished
//! todo, the `▸` on a folded run, the arrows in a token count, the half blocks the slabs are cut
//! with — is ordinary text at ordinary width, and widening it would only push it away from what it
//! labels. A row that reaches for [`icon_prefix`] with a marker of its own is the mistake to watch
//! for.
//!
//! # The spinner asks a different question
//!
//! [`spinner`] sits outside the table on purpose. The table asks *does the terminal's own face
//! contain this*, which is what a mark other columns align to has to answer. A spinner is one cell
//! with nothing aligned to it, so it may come from a fallback font, and braille usually does. So it
//! is measured rather than assumed (`braille_available`) and draws the ten-frame sweep where a font
//! on the machine has it, block elements where none does, and ASCII where the tier says to assume
//! nothing. Three drawings is too many for anything the reader has to *read*; this one they only
//! have to see move.

mod detect;
mod format;
mod spinner;
mod state;

pub use format::{configured_glyph, icon_cells, icon_gap, icon_label, icon_prefix};
pub use spinner::spinner;
#[cfg(test)]
pub(crate) use spinner::spinner_ticks_per_frame;
pub use state::{braille, resolve_vocabulary, set_braille, set_tier, tier};
use unicode_width::UnicodeWidthStr;
#[cfg(test)]
use {
    detect::braille_drawable,
    luxe_core::config::GlyphStyle,
    spinner::{SPINNER_ASCII, SPINNER_BLOCKS, SPINNER_BLOCK_TICKS, SPINNER_BRAILLE},
};

/// How much glyph fidelity the terminal and font can be asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GlyphTier {
    /// Material Design icons from a Nerd Font.
    #[default]
    Nerd,
    /// The same icons, from a face that draws them two cells wide.
    NerdWide,
    /// Geometric Shapes and Arrows; no private-use codepoints.
    Unicode,
    /// Plain ASCII punctuation.
    Ascii,
}

/// Declares the icon vocabulary: what each icon means, then its three drawings.
///
/// The enum, [`Icon::renderings`] and [`EVERY_ICON`] all come out of this one list, which is what
/// makes the claim above true — an icon really is declared once, by meaning, with its renderings
/// beside it, rather than spelled out in three places that have to agree.
///
/// They were three places, and a hand-kept list can only fail in one direction: *removing* an icon
/// breaks the enum, the match and the array, so the compiler walks you through all three, while
/// *adding* one breaks only two. `Changes` went in that way and sat outside [`EVERY_ICON`] — and so
/// outside every check that its fallbacks are one cell wide, free of private-use codepoints and
/// actually ASCII — for its whole existence.
macro_rules! icons {
    ($( $(#[$meaning:meta])* $name:ident => [$nerd:expr, $unicode:expr, $ascii:expr] ),+ $(,)?) => {
        /// Every icon the TUI can draw, named for what it means rather than what it looks like.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum Icon {
            $( $(#[$meaning])* $name, )+
        }

        impl Icon {
            /// This icon's three renderings: `[nerd, unicode, ascii]`.
            ///
            /// One exhaustive match, so a new icon cannot be added without deciding what a bare
            /// terminal shows for it — the declaration asks.
            const fn renderings(self) -> [&'static str; 3] {
                match self {
                    $( Self::$name => [$nerd, $unicode, $ascii], )+
                }
            }
        }

        /// Every icon, for the table tests and any surface that wants to enumerate them.
        ///
        /// Generated from the list above, so it cannot fall behind it.
        pub const EVERY_ICON: &[Icon] = &[$( Icon::$name, )+];
    };
}

icons! {
    // === Surfaces (a bordered panel or overlay) ===
    /// Fuzzy command search — the command palette
    Palette => ["\u{f0349}", "■", "*"],
    /// Slash-command completion above the composer
    SlashMenu => ["\u{f07b7}", "►", ">"],
    /// `@`-mention file completion, and a file generally
    FileMenu => ["\u{f0214}", "▪", "#"],
    /// Session history browser
    Session => ["\u{f02da}", "■", "~"],
    /// Find-in-transcript bar
    Search => ["\u{f13b8}", "○", "/"],
    /// Keybinding cheat sheet
    Help => ["\u{f030c}", "■", "?"],
    /// Review chapter outline
    Outline => ["\u{f0279}", "≡", "="],
    /// Review findings list
    Findings => ["\u{f023b}", "▲", "!"],
    /// Review finding detail / evidence
    Evidence => ["\u{f02fc}", "●", "i"],
    /// Review finding composer
    Compose => ["\u{f03eb}", "►", "+"],
    /// Review source (working tree, turn, base, commit)
    Source => ["\u{f0718}", "└", "^"],
    /// Finish/accept a review
    Finish => ["\u{f012c}", "■", "v"],
    // === Attention (something is waiting on the reader) ===
    /// Tool-permission request
    Permission => ["\u{f0483}", "▲", "!"],
    /// Agent question addressed to the user
    Question => ["\u{f0625}", "▲", "?"],
    /// Provider sign-in (the login wizard)
    Key => ["\u{f0306}", "●", "@"],
    /// A submission handed to the running turn
    Steering => ["\u{f0054}", "→", ">"],
    /// A submission waiting for a turn of its own
    Queued => ["\u{f03e4}", "▪", "."],
    /// A submission that never ran
    NeverSent => ["\u{f0156}", "×", "x"],
    // === The model ===
    /// A language model (catalog, statusline, dock)
    Model => ["\u{f06a9}", "●", "%"],
    /// Model reasoning
    Thinking => ["\u{f0335}", "●", "~"],
    // === Dashboard menu ===
    Find => ["\u{f021e}", "○", "/"],
    NewSession => ["\u{f0214}", "►", "+"],
    Grep => ["\u{f0284}", "○", "/"],
    Config => ["\u{f0493}", "■", "="],
    Quit => ["\u{f05fc}", "×", "x"],
    /// The project's context as a whole, and the list that shows all of it
    Loaded => ["\u{f03d6}", "▪", "#"],
    ContextFile => ["\u{f0219}", "▪", "#"],
    /// Codicons' own MCP mark, rather than a puzzle piece `activate_tools` already wears
    Mcp => ["\u{ec47}", "□", "&"],
    Skill => ["\u{f140b}", "§", "*"],
    SessionEntry => ["\u{f028c}", "■", "~"],
    // === Statusline ===
    Branch => ["\u{f062c}", "└", "^"],
    /// The branch's pull request
    PullRequest => ["\u{f04c2}", "↑", "^"],
    /// An issue, beside its number in `/github` (`oct-issue_opened`)
    Issue => ["\u{f41b}", "○", "o"],
    /// `/github`'s pull requests waiting on your review (`md-eye_outline`)
    ReviewRequested => ["\u{f06d0}", "○", "o"],
    /// `/github`'s pull requests you opened (`md-account_outline`)
    Mine => ["\u{f0013}", "●", "@"],
    /// A pull request's or an issue's Overview tab (`oct-info`)
    Overview => ["\u{f449}", "≡", "="],
    /// A pull request's or an issue's Conversation tab (`md-comment_multiple_outline`)
    Conversation => ["\u{f0181}", "»", "\""],
    /// A review that only comments, as GitHub's own review dialog draws it (`oct-comment`)
    ReviewComment => ["\u{f41f}", "»", "\""],
    /// A review that approves (`oct-check`)
    ReviewApprove => ["\u{f42e}", "•", "+"],
    /// A review that requests changes (`oct-file_diff`)
    ReviewRequestChanges => ["\u{f4d2}", "±", "!"],
    /// GitHub itself: `/github`, and the surfaces that post to it, open on it or read from it
    /// (`md-github`)
    GitHub => ["\u{f02a4}", "●", "*"],
    Folder => ["\u{f024b}", "■", "/"],
    Context => ["\u{f0e4}", "▪", "#"],
    /// How far the session's checklist has got
    Plan => ["\u{f08a8}", "≡", "="],
    /// Files this session has changed
    Changes => ["\u{f440}", "±", "+"],
    // === Modes, on the composer's title ===
    /// Plan mode: reads, asks, writes only the plan (`md-clipboard_text_outline`). Hollow where
    /// build is solid, for a terminal with no icon font: a sketch, then the thing.
    ModePlan => ["\u{f0a38}", "○", ""],
    /// Build mode, the default: changes files and runs commands (`md-hammer_wrench`)
    ModeBuild => ["\u{f1323}", "●", ""],
    // === Inline in the transcript and composer ===
    /// The `@` picker, which references files and GitHub (`md-at`)
    Mention => ["\u{f0065}", "@", "@"],
    /// A mention source of the reader's own, when it names no icon (`md-tag_outline`)
    MentionSource => ["\u{f04fc}", "▪", "-"],
    /// An attached file on a submission
    Attachment => ["\u{f0c6}", "▪", "#"],
    /// An attached image
    Image => ["\u{f03e}", "▫", "&"],
    /// The copy affordance on a code block
    Copy => ["\u{f0c5}", "▪", "c"],
    /// A fenced code block whose language has no mark of its own
    Code => ["\u{f0174}", "", ""],

    // === Markdown alerts (`> [!NOTE]` and its four siblings) ===
    //
    // Five silhouettes a reader can tell apart at a terminal's glyph size, which rules out the
    // ornate ones: an `i` in a circle, a bulb, and the exclamation mark in a circle, a triangle
    // and an octagon — the shapes road signs already spend on the same three severities.
    // Codepoints confirmed by glyph *name* in IosevkaTermNerdFontMono (`md-alert_octagon`, not a
    // plausible-looking neighbor) at one cell of advance.
    //
    // Below the nerd tier they collapse into the two families the rest of the table uses — `●`/`○`
    // for something worth knowing, `▲` for something worth stopping at — because the alert's word
    // is written out beside every one of them and carries the distinction on its own.
    /// A `> [!NOTE]` alert
    AlertNote => ["\u{f02fd}", "●", "i"],
    /// A `> [!TIP]` alert
    AlertTip => ["\u{f0336}", "○", "*"],
    /// A `> [!IMPORTANT]` alert
    AlertImportant => ["\u{f0028}", "▲", "!"],
    /// A `> [!WARNING]` alert
    AlertWarning => ["\u{f0026}", "▲", "!"],
    /// A `> [!CAUTION]` alert
    AlertCaution => ["\u{f0029}", "▲", "!"],

    // === Languages (a fenced code block's own mark) ===
    //
    // Brand marks, one per language, drawn only at the nerd tier: the language's *name* is written
    // beside them on the same rule, so a terminal without a patched font loses the shine and keeps
    // the information. Substituting a geometric shape would be worse than nothing — twenty-seven
    // languages cannot have twenty-seven honest shapes in a bare font, and one `▪` for all of them
    // says less than the word already does.
    //
    // Every codepoint below was verified present in IosevkaTermNerdFontMono and confirmed by its
    // glyph *name* (`dev-rust`, `seti-typescript`, …) rather than by memory: `U+E60B` looks like a
    // plausible JSON mark and is `seti-less`.
    LangRust => ["\u{e7a8}", "", ""],
    LangPython => ["\u{e73c}", "", ""],
    LangTypeScript => ["\u{e628}", "", ""],
    LangJavaScript => ["\u{e781}", "", ""],
    LangGo => ["\u{e626}", "", ""],
    // `seti-c` and `seti-cpp` rather than the `custom-*` pair: those two are both a letter in a
    // circle and at a terminal's glyph size they are the same picture, which defeats the only
    // reason to spend a cell on a mark. These read as `C` and `C+`.
    LangC => ["\u{e649}", "", ""],
    LangCpp => ["\u{e646}", "", ""],
    LangShell => ["\u{e760}", "", ""],
    LangRuby => ["\u{e739}", "", ""],
    LangLua => ["\u{e826}", "", ""],
    LangSvelte => ["\u{e8b7}", "", ""],
    LangJson => ["\u{e80b}", "", ""],
    LangYaml => ["\u{e8eb}", "", ""],
    LangToml => ["\u{e6b2}", "", ""],
    LangHtml => ["\u{e736}", "", ""],
    LangCss => ["\u{e749}", "", ""],
    LangSql => ["\u{e706}", "", ""],
    LangMarkdown => ["\u{e73e}", "", ""],
    LangJava => ["\u{e738}", "", ""],
    LangPhp => ["\u{e73d}", "", ""],
    LangSwift => ["\u{e755}", "", ""],
    LangKotlin => ["\u{e81b}", "", ""],
    LangHaskell => ["\u{e777}", "", ""],
    LangZig => ["\u{e8ef}", "", ""],
    LangNix => ["\u{e843}", "", ""],
    LangDocker => ["\u{e7b0}", "", ""],
    LangDiff => ["\u{eae1}", "", ""],
    /// A tool's raw output, in the transcript
    ToolOutput => ["\u{f07e}", "▎", "|"],
    /// A system notice in the transcript
    SystemLine => ["\u{f120}", "└", "^"],
    /// An answer the reader gave the agent, on the card that asked for it. Drawn at every tier
    /// rather than omitted like a tool card's icon, because this row is aligned against the header
    /// above it — whose tool is `question`, which wears [`Icon::Question`] and keeps its fallbacks.
    /// Drop these and the two labels shear apart on any terminal without the Material shapes.
    Answer => ["\u{f012c}", "■", "v"],
    // === Tools: file operations ===
    ToolRead => ["\u{edfa}", "", ""],
    ToolWrite => ["\u{f0193}", "", ""],
    ToolEdit => ["\u{f440}", "", ""],
    // === Tools: the shell ===
    ToolBash => ["\u{f07b7}", "", ""],
    // === Tools: search ===
    ToolGrep => ["\u{f0451}", "", ""],
    ToolGlob => ["\u{f021e}", "", ""],
    ToolAstGrep => ["\u{f0169}", "", ""],
    ToolSymbolSearch => ["\u{f1907}", "", ""],
    // === Tools: code intelligence ===
    ToolDiagnostics => ["\u{ea6c}", "", ""],
    ToolNavigation => ["\u{f018b}", "", ""],
    ToolModuleReport => ["\u{f04aa}", "", ""],
    ToolReadSymbol => ["\u{f0295}", "", ""],
    ToolImpact => ["\u{f1049}", "", ""],
    ToolDebug => ["\u{f00e4}", "", ""],
    // === Tools: the project ===
    ToolGit => ["\u{f02a2}", "", ""],
    ToolTest => ["\u{f0668}", "", ""],
    // === Tools: the web ===
    ToolWebFetch => ["\u{f059f}", "", ""],
    ToolWebSearch => ["\u{f070f}", "", ""],
    // === Tools: session and loop ===
    ToolTodo => ["\u{f0135}", "", ""],
    ToolSkill => ["\u{f51b}", "", ""],
    ToolScreenshot => ["\u{f0100}", "", ""],
    ToolLoopControl => ["\u{f0456}", "", ""],
    ToolMcp => ["\u{f06a5}", "", ""],
    ToolActivate => ["\u{f0431}", "", ""],
    /// Any tool with no icon of its own
    ToolGeneric => ["\u{f05b7}", "", ""],
}

/// Declares the structural marks: the glyphs a row is *built* from rather than labeled with.
///
/// Same three renderings as [`icons!`], one rule different — a mark may never be empty. An icon is
/// decoration and can stand down, which is why two dozen of them do; a fold arrow, a cursor or a
/// tree corner is the thing the row is saying, and dropping it would cost meaning rather than
/// shine.
macro_rules! marks {
    ($( $(#[$meaning:meta])* $name:ident => [$nerd:expr, $unicode:expr, $ascii:expr] ),+ $(,)?) => {
        /// A structural glyph: part of a row's grammar, not an icon beside its label.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum Mark {
            $( $(#[$meaning])* $name, )+
        }

        impl Mark {
            /// This mark's three renderings: `[nerd, unicode, ascii]`.
            const fn renderings(self) -> [&'static str; 3] {
                match self {
                    $( Self::$name => [$nerd, $unicode, $ascii], )+
                }
            }
        }

        /// Every mark, for the table tests.
        pub const EVERY_MARK: &[Mark] = &[$( Mark::$name, )+];
    };
}

marks! {
    /// The row under the cursor, in a menu or a picker
    Cursor => ["\u{276f}", "\u{203a}", ">"],
    /// A run that is folded shut
    FoldClosed => ["\u{25b8}", "\u{25ba}", ">"],
    /// A run that is open
    FoldOpen => ["\u{25be}", "\u{25bc}", "v"],
    /// A row wrapped from the one above it
    Continuation => ["\u{21b3}", "\u{2514}", "\\"],
    /// Lines the card is not showing
    Elision => ["\u{22ef}", "\u{2026}", "."],
    /// Input served from the prompt cache
    CacheReuse => ["\u{21ba}", "\u{2190}", "<"],
    /// Work that stopped before it finished — a cancelled call, a blocked task
    Halted => ["\u{2298}", "\u{25cb}", "-"],
    /// Work under way, where a spinner would be too loud
    Underway => ["\u{25c9}", "\u{25cf}", "*"],
    /// Present, installed, checked
    Yes => ["\u{2713}", "\u{25a0}", "v"],
    /// Absent, missing, unchecked
    No => ["\u{2717}", "\u{00d7}", "x"],
    /// Waiting on something else to finish, or on someone else
    Waiting => ["\u{25d4}", "\u{25cb}", "o"],
}

impl Mark {
    /// This mark at `tier`. Pure, so tests never depend on process state.
    pub fn at(self, tier: GlyphTier) -> &'static str {
        let [nerd, unicode, ascii] = self.renderings();
        match tier {
            GlyphTier::Nerd | GlyphTier::NerdWide => nerd,
            GlyphTier::Unicode => unicode,
            GlyphTier::Ascii => ascii,
        }
    }

    /// This mark at the tier the process resolved at startup.
    pub fn glyph(self) -> &'static str {
        self.at(tier())
    }

    /// Whether `text` begins with any of this mark's renderings.
    ///
    /// A few surfaces stamp a mark into a string and re-read it later. The tier can move between
    /// those two moments, so the check has to accept every rendering.
    pub fn leads(self, text: &str) -> bool {
        self.renderings().iter().any(|glyph| text.starts_with(glyph))
    }

    /// This mark and the space after it, or the same width in blanks when `shown` is false.
    ///
    /// The `❯ label` / `  label` pair that a dozen pickers spelled out by hand. One owner,
    /// because the invariant is easy to state and easy to break: an unselected row spends the
    /// gutter too, or every label steps sideways as the cursor lands on it.
    pub fn gutter(self, shown: bool) -> String {
        if shown {
            format!("{} ", self.glyph())
        } else {
            " ".repeat(UnicodeWidthStr::width(self.glyph()) + 1)
        }
    }
}

impl std::fmt::Display for Mark {
    /// So a mark drops into `format!` where a literal would sit.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.glyph())
    }
}

impl Icon {
    /// This icon at `tier`. Pure, so tests never depend on process state.
    pub fn at(self, tier: GlyphTier) -> &'static str {
        let [nerd, unicode, ascii] = self.renderings();
        match tier {
            GlyphTier::Nerd | GlyphTier::NerdWide => nerd,
            GlyphTier::Unicode => unicode,
            GlyphTier::Ascii => ascii,
        }
    }

    /// This icon and the clearance after it, or nothing at all when it has no rendering at this tier.
    ///
    /// For the `{icon} {label}` shape that most surfaces use: an omitted icon has to take its separator
    /// with it, or the label sits one column in from where it belongs.
    pub fn prefix(self) -> String {
        icon_prefix(self.glyph())
    }

    /// Whether `text` begins with any of this icon's renderings.
    ///
    /// A couple of surfaces stamp an icon into a string and re-read it later to recover what the
    /// string described. The tier can move between those two moments — a `/reload` is enough — so the
    /// check has to accept every rendering rather than the one that happens to be current.
    pub fn leads(self, text: &str) -> bool {
        self.renderings().iter().any(|glyph| text.starts_with(glyph))
    }

    /// This icon at the tier the process resolved at startup.
    ///
    /// The vocabulary is a property of the terminal and the config, so it is process-wide rather than
    /// threaded through every view signature; `/reload` can move it, which is why it is an atomic
    /// rather than a `OnceLock`.
    pub fn glyph(self) -> &'static str {
        self.at(tier())
    }
}

#[cfg(test)]
mod tests;
