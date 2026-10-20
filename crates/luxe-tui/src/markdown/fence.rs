//! Markdown fences — a code block's lines and the frame drawn around them.

use super::line::{FenceRule, MarkdownLine};
use super::voice::{build_code_style, build_fence_border_style, MarkdownVoice};
use crate::glyphs::Icon;
use crate::theme::styles::ThemeStyles;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

/// A code block being collected: whether the walk is inside one, what it declared, its text.
#[derive(Default)]
pub(super) struct FenceBuilder {
    active: bool,
    lang: Option<String>,
    lines: Vec<String>,
}

impl FenceBuilder {
    /// Opens a block; an indented one declares no language.
    pub(super) fn begin(&mut self, lang: Option<String>) {
        self.active = true;
        self.lang = lang;
    }

    /// Whether the walk is inside a code block, where everything inline is source.
    pub(super) fn active(&self) -> bool {
        self.active
    }

    pub(super) fn push(&mut self, text: String) {
        self.lines.push(text);
    }

    /// Closes the block: its framed lines when it held any code, nothing when it was empty.
    pub(super) fn finish(
        &mut self,
        theme: &ThemeStyles,
        voice: MarkdownVoice,
    ) -> Vec<MarkdownLine> {
        let lines = match self.lines.is_empty() {
            true => Vec::new(),
            false => {
                framed_fence(&std::mem::take(&mut self.lines), self.lang.as_deref(), theme, voice)
            }
        };
        self.active = false;
        self.lang = None;
        lines
    }
}

/// A code block's lines between its two rules: the language and the copy affordance on the top
/// one, nothing on the bottom, both the width of the widest row.
fn framed_fence(
    code_lines: &[String],
    code_lang: Option<&str>,
    theme: &ThemeStyles,
    voice: MarkdownVoice,
) -> Vec<MarkdownLine> {
    let mut lines = Vec::new();
    let code_str = code_lines.join("\n");
    let rendered = fence_lines(&code_str, code_lang, theme, voice);
    let caption = theme.muted.add_modifier(voice.modifier());
    let border = build_fence_border_style(theme, voice);
    let ends_at = rendered.iter().map(Line::width).max().unwrap_or(0);
    // The language the fence declared, if it declared one worth naming. An
    // unlabeled fence gets a rule with nothing on its left rather than the word
    // "text", which would be luxe guessing on the author's behalf.
    let lead = fence_language(code_lang)
        .map(|language| {
            // The language's own mark where the terminal can draw one, and the bare
            // word where it cannot — `icon_label` drops the glyph *and* its
            // clearance, so the label does not sit one column in from the rule.
            let icon = crate::widgets::language_icon(&language).glyph();
            vec![Span::styled(crate::glyphs::icon_label(icon, &language), caption)]
        })
        .unwrap_or_default();
    let tail = vec![Span::styled(crate::glyphs::icon_label(Icon::Copy.glyph(), "copy"), caption)];
    // The frame may be wider than the code it frames. A box around three words is
    // still a box, and clamping to the block's own width instead collapsed the
    // rule out of existence on exactly the short snippets answers are full of.
    // Both rules take the same width, or they would not line up.
    let frame = ends_at.max(fence_frame_minimum(&lead, &tail));
    lines.push(MarkdownLine::fence_top(
        FenceRule { lead, tail, ends_at: frame, style: border },
        code_str,
    ));
    lines.extend(rendered.into_iter().map(MarkdownLine::wide));
    lines.push(MarkdownLine::fence_bottom(FenceRule {
        lead: Vec::new(),
        tail: Vec::new(),
        ends_at: frame,
        style: border,
    }));
    lines
}

/// The lines of a fenced code block.
///
/// Syntax highlighting is an answer's; a reasoning fence takes the voice's own code style, because
/// a syntect palette inside a muted italic block reads as a different message entirely.
fn fence_lines(
    code: &str,
    lang: Option<&str>,
    theme: &ThemeStyles,
    voice: MarkdownVoice,
) -> Vec<Line<'static>> {
    match voice {
        MarkdownVoice::Answer => crate::highlight::highlight_code(code, lang, theme),
        MarkdownVoice::Reasoning => {
            let style = build_code_style(theme, voice);
            code.lines().map(|line| Line::from(Span::styled(line.to_string(), style))).collect()
        }
    }
}

/// The narrowest frame that still seats both labels with a cell of rule at each end.
///
/// Mirrors what [`crate::view::wrapping`] spends laying one out; the two have to agree, or the top
/// rule falls back to bare labels while the bottom one draws at a width nothing matches.
fn fence_frame_minimum(lead: &[Span<'static>], tail: &[Span<'static>]) -> usize {
    let width = |spans: &[Span<'static>]| -> usize {
        spans.iter().map(|span| UnicodeWidthStr::width(span.content.as_ref())).sum()
    };
    let label = |spans: &[Span<'static>]| match width(spans) {
        0 => 0,
        cells => cells + crate::view::wrapping::LABEL_FRAME,
    };
    label(lead) + label(tail) + crate::view::wrapping::MIN_FENCE_FILL
}

/// The language a fence's rule announces, normalized, or `None` when it named nothing useful.
///
/// Trimmed and lowercased because a fence is written by hand and `Rust`, `RUST` and `rust ` are the
/// same language. Only the first token survives: rustdoc writes `rust,ignore` and mdBook writes
/// `rust no_run`, and neither suffix is the name of anything. `text` and `plain` are dropped
/// outright — they say a fence is a fence.
pub(super) fn fence_language(lang: Option<&str>) -> Option<String> {
    let lang = lang?.trim().to_lowercase();
    let lang = lang.split([',', ' ', '\t', '{']).next()?.to_string();
    (!lang.is_empty() && !matches!(lang.as_str(), "text" | "plain" | "plaintext" | "txt"))
        .then_some(lang)
}
