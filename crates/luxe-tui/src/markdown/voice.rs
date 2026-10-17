//! Markdown voices — the two readers a block is styled for, and every style each one spends.

use crate::glyphs::Icon;
use crate::theme::styles::ThemeStyles;
use pulldown_cmark::{BlockQuoteKind, HeadingLevel};
use ratatui::style::{Color as RatatuiColor, Modifier, Style};

/// Which reader a markdown block is being rendered for.
///
/// Reasoning has to stay recognizable as reasoning while still being *formatted*, so the two
/// voices differ in what they spend on structure rather than in what they parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MarkdownVoice {
    /// Assistant prose: the theme's markdown tokens, syntax-highlighted fences.
    Answer,
    /// Model reasoning: muted italic throughout, headings and `**strong**` earning weight instead
    /// of a color, `code` keeping the theme's code hue, and fences left unhighlighted.
    Reasoning,
}

impl MarkdownVoice {
    /// The modifier every span in this voice carries.
    pub(super) fn modifier(self) -> Modifier {
        match self {
            Self::Answer => Modifier::empty(),
            Self::Reasoning => Modifier::ITALIC,
        }
    }

    /// Whether a soft break ends the line. Reasoning is the model's own notes rather than authored
    /// prose, so its line breaks are kept instead of reflowed into one paragraph.
    pub(super) fn breaks_on_soft_break(self) -> bool {
        matches!(self, Self::Reasoning)
    }
}

/// Build a Style from the current style stack.
pub(super) fn build_style(stack: &[Modifier], base: Style) -> Style {
    let mut style = base;
    for modifier in stack {
        style = style.add_modifier(*modifier);
    }
    style
}

/// The voice's base prose style — what unadorned text, list bullets, and table cells take.
pub(super) fn build_text_style(theme: &ThemeStyles, voice: MarkdownVoice) -> Style {
    let base = match voice {
        MarkdownVoice::Answer => theme.text,
        MarkdownVoice::Reasoning => theme.thinking_text,
    };
    base.add_modifier(voice.modifier())
}

/// Build a distinct Style for inline code spans using the md_code token.
///
/// Reasoning keeps the hue: a backtick in a model's notes almost always names a symbol, a path or a
/// command, and that is worth finding at a glance even in a block meant to recede.
pub(super) fn build_code_style(theme: &ThemeStyles, voice: MarkdownVoice) -> Style {
    theme.md_code.add_modifier(voice.modifier())
}

/// Build a heading style using the md_heading token with BOLD modifier.
///
/// A reasoning heading spends weight instead of color, so the section titles GPT models lead with
/// read as titles without breaking the muted voice the rest of the block keeps.
pub(super) fn build_heading_style(
    theme: &ThemeStyles,
    _level: HeadingLevel,
    voice: MarkdownVoice,
) -> Style {
    let base = match voice {
        MarkdownVoice::Answer => theme.md_heading,
        MarkdownVoice::Reasoning => theme.thinking_text,
    };
    base.add_modifier(Modifier::BOLD).add_modifier(voice.modifier())
}

/// Build a blockquote prefix style using the md_quote token.
pub(super) fn build_quote_style(theme: &ThemeStyles, voice: MarkdownVoice) -> Style {
    if voice == MarkdownVoice::Reasoning {
        return build_text_style(theme, voice);
    }
    if theme.md_quote.fg.is_some() {
        theme.md_quote
    } else {
        Style::new().fg(RatatuiColor::DarkGray) // fallback: dim gray
    }
}

/// The mark a GFM alert wears beside its word, where the terminal can draw one.
pub(super) fn alert_icon(kind: BlockQuoteKind) -> Icon {
    match kind {
        BlockQuoteKind::Note => Icon::AlertNote,
        BlockQuoteKind::Tip => Icon::AlertTip,
        BlockQuoteKind::Important => Icon::AlertImportant,
        BlockQuoteKind::Warning => Icon::AlertWarning,
        BlockQuoteKind::Caution => Icon::AlertCaution,
    }
}

/// The word a GFM alert announces itself with.
pub(super) fn alert_label(kind: BlockQuoteKind) -> &'static str {
    match kind {
        BlockQuoteKind::Note => "Note",
        BlockQuoteKind::Tip => "Tip",
        BlockQuoteKind::Important => "Important",
        BlockQuoteKind::Warning => "Warning",
        BlockQuoteKind::Caution => "Caution",
    }
}

/// How an alert's word is drawn: the severity's own color for an answer, and weight alone for
/// reasoning, which spends structure rather than color everywhere else too.
pub(super) fn build_alert_style(
    theme: &ThemeStyles,
    voice: MarkdownVoice,
    kind: BlockQuoteKind,
) -> Style {
    if voice == MarkdownVoice::Reasoning {
        return build_text_style(theme, voice).add_modifier(Modifier::BOLD);
    }
    let base = match kind {
        BlockQuoteKind::Note => theme.info,
        BlockQuoteKind::Tip => theme.success,
        BlockQuoteKind::Important => theme.accent,
        BlockQuoteKind::Warning => theme.warning,
        BlockQuoteKind::Caution => theme.error,
    };
    base.add_modifier(Modifier::BOLD)
}

/// Build a link style using the md_link theme token with UNDERLINED modifier.
pub(super) fn build_link_style(theme: &ThemeStyles, voice: MarkdownVoice) -> Style {
    if voice == MarkdownVoice::Reasoning {
        return build_text_style(theme, voice).add_modifier(Modifier::UNDERLINED);
    }
    let mut style = theme.md_link;
    style = style.add_modifier(Modifier::UNDERLINED);
    if style.fg.is_none() {
        style = style.fg(RatatuiColor::Blue); // fallback: blue for links
    }
    style
}

/// Build a link URL style using the md_link_url theme token.
pub(super) fn build_link_url_style(theme: &ThemeStyles, voice: MarkdownVoice) -> Style {
    if voice == MarkdownVoice::Reasoning {
        return build_text_style(theme, voice);
    }
    if theme.md_link_url.fg.is_some() {
        theme.md_link_url
    } else {
        Style::new().fg(RatatuiColor::DarkGray) // fallback: dim gray for URLs
    }
}

/// Build a readable table-cell style.
pub(super) fn build_table_style(theme: &ThemeStyles, voice: MarkdownVoice) -> Style {
    build_text_style(theme, voice)
}

/// The style a code fence's frame is drawn in — the table border's, for the same reason: two
/// framed blocks in one answer should look like one idea.
pub(super) fn build_fence_border_style(theme: &ThemeStyles, voice: MarkdownVoice) -> Style {
    build_table_border_style(theme, voice)
}

/// Border color for the box-drawing frame — subtle, distinct from cell text.
pub(super) fn build_table_border_style(theme: &ThemeStyles, voice: MarkdownVoice) -> Style {
    if voice == MarkdownVoice::Reasoning {
        return build_text_style(theme, voice);
    }
    Style::new().fg(theme.border.fg.or(theme.muted.fg).unwrap_or(RatatuiColor::DarkGray))
}

/// Build a horizontal rule style using the md_rule token with BOLD modifier.
pub(super) fn build_hr_style(theme: &ThemeStyles, voice: MarkdownVoice) -> Style {
    let mut style = Style::new().add_modifier(Modifier::BOLD).add_modifier(voice.modifier());
    // Use muted token for horizontal rules — distinct from code but subtle.
    style = style.fg(theme.muted.fg.unwrap_or(RatatuiColor::DarkGray));
    style
}
