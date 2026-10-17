//! Conversation blocks — semantic lines for transcript and tool activity rendering.
//!
//! Keeps per-block presentation separate from viewport anchoring, wrapping, and input layout.

mod group;
mod message;
mod tool;

use super::tool_card;
use crate::app::ConvoBlock;
use crate::glyphs::{icon_label, Icon};
use crate::graphics::ImageKey;
use crate::markdown::{layout_markdown, ImageRow, MarkdownBlockKind, MarkdownLine};
use crate::theme::styles::ThemeStyles;
pub(super) use group::{run_is_working, tool_group_lines};
use message::{prefixed_text_lines, system_lines, thinking_lines, user_lines};
use ratatui::{
    style::Style,
    text::{Line, Span},
};
use tool::tool_lines;
pub(super) use tool::{header_subject_column, BODY_INDENT};
#[cfg(test)]
use {
    crate::{
        app::ConversationItem,
        widgets::{tool_glyph, GENERIC_TOOL},
    },
    message::{chipped_lines, line_text},
    ratatui::style::Modifier,
    tool::format_tool_duration,
};

/// Everything a block needs about the frame it is being drawn into.
///
/// `spinner_frame` is the app-wide animation tick (the same counter the statusline uses), so a
/// running tool card spins instead of showing a frozen glyph. `never_sent` marks a submission the
/// engine never received, and `expanded` shows a long user message past its preview cap. The
/// card fields travel with it because a tool card's body has to be laid out against a real width —
/// two diff columns are only worth it if the card can seat them.
pub(super) struct BlockContext<'a> {
    pub(crate) styles: &'a ThemeStyles,
    pub(crate) spinner_frame: usize,
    /// Whether the turn is waiting on the reader: a question to answer or a permission to give.
    pub(crate) waiting_on_user: bool,
    pub(crate) never_sent: bool,
    pub(crate) expanded: bool,
    /// Every image the transcript can draw, and how big each one wants to be.
    pub(crate) images: &'a crate::graphics::ImageStore,
    /// Which images each tool call produced.
    pub(crate) tool_images: &'a std::collections::HashMap<(String, String), Vec<ImageKey>>,
    /// Cells an inline image may occupy across and down.
    pub(crate) image_bounds: (u16, u16),
    /// Cells a wide block's content may occupy, after the card's own indent.
    pub(crate) card_width: u16,
    /// Cells a prose block may occupy — the measure a collapsed reasoning peek is cut to.
    pub(crate) prose_width: u16,
    pub(crate) diff_layout: luxe_core::config::DiffLayout,
    /// Whether every tool card is showing its full output.
    pub(crate) tool_output_expanded: bool,
    /// The chord that toggles that, named in a truncation footer.
    pub(crate) expand_chord: Option<String>,
    /// How long the call in flight has been running, in milliseconds.
    ///
    /// Handed in rather than read off the clock, so the figure moves with the instant the frame is
    /// drawn against — a recording pins that, and a timer that consults the wall reports how fast
    /// the recorder rendered.
    pub(crate) running_ms: Option<u64>,
}

impl BlockContext<'_> {
    fn card(&self) -> tool_card::CardContext<'_> {
        tool_card::CardContext {
            styles: self.styles,
            width: self.card_width,
            layout: self.diff_layout,
            expanded: self.tool_output_expanded,
            expand_chord: self.expand_chord.clone(),
        }
    }
}

/// The rows an inline image reserves, or the line that stands in for it.
///
/// One or the other, never both. A terminal that cannot draw images gets a line naming what it is
/// not showing — and, when the picture is a real file on disk, an affordance that opens it in
/// whatever the desktop uses for images. That is the honest version of this feature on a terminal
/// without it: not a low-fidelity imitation of the picture, but the picture, one click away.
fn image_lines(key: &ImageKey, ctx: &BlockContext<'_>, style: Style) -> Vec<MarkdownLine> {
    let Some((width, height)) = ctx.images.source_size(key) else {
        return vec![MarkdownLine::wide(Line::from(Span::styled(
            icon_label(Icon::Image.glyph(), "image unavailable"),
            ctx.styles.muted,
        )))];
    };
    let (max_cols, max_rows) = ctx.image_bounds;
    if ctx.images.tier().draws_pixels() {
        if let Some(fit) = ctx.images.fit(key, max_cols, max_rows) {
            return (0..fit.rows)
                .map(|row| MarkdownLine::image_row(ImageRow { key: key.clone(), fit, row }))
                .collect();
        }
    }

    let origin = ctx.images.origin(key);
    let label = origin.map_or("image", |origin| origin.label.as_str());
    let mut spans = vec![Span::styled(
        icon_label(Icon::Image.glyph(), &format!("{label} \u{b7} {width}\u{d7}{height}")),
        style,
    )];
    let openable = origin.and_then(|origin| origin.path.as_deref());
    if let Some(path) = openable {
        let name = path.file_name().map_or_else(
            || luxe_core::paths::display(path),
            |name| name.to_string_lossy().into_owned(),
        );
        spans.push(Span::styled(format!("  {name}"), ctx.styles.muted));
        spans.push(Span::styled("  open \u{2197}", ctx.styles.accent));
    }
    let line = MarkdownLine::wide(Line::from(spans));
    vec![if openable.is_some() { line.opens_image(key.clone()) } else { line }]
}

/// Renders one conversation block into semantic lines.
pub(super) fn lines(block: &ConvoBlock, ctx: &BlockContext<'_>) -> Vec<MarkdownLine> {
    let styles = ctx.styles;
    let style = block_style(block, styles);
    match block {
        ConvoBlock::User { text, attached, chips } => user_lines(text, attached, chips, style, ctx),
        ConvoBlock::Image { key, .. } => image_lines(key, ctx, style),
        ConvoBlock::Assistant(text) => {
            // What the agent wrote for Luxe — a remediation report, a draft — is shown where it lands
            let text = &*luxe_core::briefs::for_the_reader(text);
            if text.trim().is_empty() {
                return Vec::new();
            }
            let rendered = layout_markdown(text, styles);
            if rendered.is_empty() {
                layout_lines(prefixed_text_lines("", text, style), MarkdownBlockKind::Prose)
            } else {
                rendered
            }
        }
        ConvoBlock::Thinking { text, expanded } => thinking_lines(text, *expanded, ctx),
        ConvoBlock::Tool(item) => tool_lines(item, style, ctx),
        ConvoBlock::System(text) => layout_lines(
            system_lines(Icon::SystemLine.glyph(), text, style, styles),
            MarkdownBlockKind::Prose,
        ),
        // Wide: a row is a label column, a figure column and a fixed-width meter, so reflowing it
        // for a narrow canvas would shear the alignment that makes the rows comparable.
        ConvoBlock::Context(report) => {
            layout_lines(super::context_report::lines(report, styles), MarkdownBlockKind::Wide)
        }
    }
}

/// Background band style for blocks that should stand out from plain assistant prose (user
/// messages and tool calls). `None` renders with no band.
pub(super) fn background(block: &ConvoBlock, styles: &ThemeStyles) -> Option<Style> {
    match block {
        ConvoBlock::User { .. } => Some(styles.user_msg_bg),
        // Live tool calls carry a status, so the band is tinted by outcome (subtle success/error
        // wash); a still-running/cancelled call and the persisted ToolCall/ToolResult blocks (no
        // status) keep the neutral band.
        // Skills get a neutral band so their purple foreground (see block_style) reads as a
        // distinct activation cue rather than another green/red success/failure wash.
        ConvoBlock::Tool(item) if item.activity.name == "skill" => Some(styles.tool_call_bg),
        ConvoBlock::Tool(item) => Some(match item.activity.state {
            luxe_core::ToolActivityState::Succeeded => styles.tool_call_success_bg,
            luxe_core::ToolActivityState::Failed => styles.tool_call_error_bg,
            _ => styles.tool_call_bg,
        }),
        // Reasoning stays band-less so it recedes like plain prose rather than a status card.
        ConvoBlock::Thinking { .. } => None,
        _ => None,
    }
}

/// Whether a block's text is paced onto the screen rather than appearing all at once.
///
/// Only what the model is composing: prose, and reasoning that is shown in full. A card, a user
/// message or a system line lands the moment it exists, and a collapsed reasoning block is a peek at
/// its *first* line — there is nothing there to type out.
pub(super) fn animates_reveal(block: &ConvoBlock) -> bool {
    matches!(block, ConvoBlock::Assistant(_) | ConvoBlock::Thinking { expanded: true, .. })
}

fn block_style(block: &ConvoBlock, styles: &ThemeStyles) -> Style {
    match block {
        ConvoBlock::User { .. } | ConvoBlock::Image { .. } => styles.accent,
        ConvoBlock::Assistant(_) => styles.success,
        // Skills carry the theme's secondary/purple accent so an activated playbook is instantly
        // recognizable in the transcript; a failed/cancelled skill still falls back to the usual
        // error/muted cue so problems aren't hidden behind the accent.
        ConvoBlock::Tool(item) if item.activity.name == "skill" => match item.activity.state {
            luxe_core::ToolActivityState::Failed => styles.error,
            luxe_core::ToolActivityState::Cancelled => styles.muted,
            _ => styles.accent_secondary,
        },
        ConvoBlock::Tool(item) => match item.activity.state {
            luxe_core::ToolActivityState::Running
            | luxe_core::ToolActivityState::Composing
            | luxe_core::ToolActivityState::Interrupted => styles.warning,
            luxe_core::ToolActivityState::Succeeded => styles.success,
            luxe_core::ToolActivityState::Failed => styles.error,
            // A call that is only waiting its turn recedes; the warning color belongs to the one
            // card that is actually moving.
            luxe_core::ToolActivityState::Queued | luxe_core::ToolActivityState::Cancelled => {
                styles.muted
            }
        },
        ConvoBlock::System(_) => styles.muted,
        // The report colors its own rows by what each one says; a block style would fight them.
        ConvoBlock::Context(_) => styles.text,
        ConvoBlock::Thinking { .. } => styles.thinking_text,
    }
}

fn layout_lines(lines: Vec<Line<'static>>, kind: MarkdownBlockKind) -> Vec<MarkdownLine> {
    lines
        .into_iter()
        .map(|line| match kind {
            MarkdownBlockKind::Prose => MarkdownLine::prose(line),
            MarkdownBlockKind::Wide => MarkdownLine::wide(line),
        })
        .collect()
}

#[cfg(test)]
mod tests;
