//! Markdown lines — one laid-out row and what the transcript needs to know about it beyond its
//! text: its width class, a fence's rule, an image row, a table, a copy or open affordance.

use super::table::TableBlock;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

/// Structural kind used by the conversation width policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MarkdownBlockKind {
    /// Normal prose, headings, lists, quotes, and rules.
    Prose,
    /// Fenced code and tables which may use the wide canvas.
    Wide,
}

/// One styled Markdown line with its structural layout kind.
#[derive(Debug, Clone)]
pub(crate) struct MarkdownLine {
    /// Structural kind retained through layout.
    pub(crate) kind: MarkdownBlockKind,
    /// Styled terminal content.
    pub(crate) line: Line<'static>,
    /// When set, this line is a clickable "copy" affordance carrying its code block's source text.
    pub(crate) code_copy: Option<String>,
    /// When set, the line is pushed right until it ends at this column, clamped to the canvas.
    ///
    /// Carried rather than padded at layout time because the column a line should end at is a
    /// property of the block it belongs to — a code block's own widest row — while the canvas that
    /// can overrule it is only known once the transcript is being drawn.
    pub(crate) ends_at: Option<usize>,
    /// When set, this line is one row of an inline image rather than text.
    pub(crate) image: Option<ImageRow>,
    /// When set, clicking this line opens that image in the desktop's image viewer.
    ///
    /// The affordance a terminal that cannot draw images gets instead — the picture is still right
    /// there on disk, so the honest answer is to hand it over rather than to approximate it.
    pub(crate) opens_image: Option<crate::graphics::ImageKey>,
    /// Whether a click on this line opens or closes the block it belongs to.
    ///
    /// A handle, not the whole block: a card's header and the footer offering the expand are things
    /// to click, while a card's *body* is text to select — collapsing the card out from under a
    /// double-click is how you make a path in it impossible to grab.
    pub(crate) expands: bool,
    /// When set, this line is a code fence's frame rather than text; see [`FenceRule`].
    pub(crate) fence_rule: Option<FenceRule>,
    /// When set, the column this line's wrapped rows start at, whatever its kind.
    ///
    /// For a line that knows where its own text begins, like a tool card's header, whose subject
    /// sits after the tool's name: wrapped back to the margin, a long command or question runs
    /// under the name and stops reading as one block.
    pub(crate) hang: Option<usize>,
    /// When set, this line is a whole table, laid out once the width it lands in is known; see
    /// [`TableBlock`].
    pub(crate) table: Option<std::sync::Arc<TableBlock>>,
}

/// A fenced code block's frame line: a rule, with labels sitting on its ends.
///
/// Expanded at wrap time rather than built here, for the same reason [`MarkdownLine::ends_at`] is
/// deferred: the rule runs the width of the block's own widest row, and only the canvas being drawn
/// into knows whether that width survives.
#[derive(Debug, Clone)]
pub(crate) struct FenceRule {
    /// Spans that open the rule — the language label on a top rule, nothing on a bottom one.
    pub(crate) lead: Vec<Span<'static>>,
    /// Spans that close it — the copy affordance on a top rule.
    pub(crate) tail: Vec<Span<'static>>,
    /// The block's widest row: where the rule wants to stop, before the canvas clamps it.
    pub(crate) ends_at: usize,
    /// The rule glyphs' own style.
    pub(crate) style: Style,
}

/// One row of an inline image: which image, how big it is, and which of its rows this is.
#[derive(Debug, Clone)]
pub(crate) struct ImageRow {
    pub(crate) key: crate::graphics::ImageKey,
    /// The size the whole image was laid out at.
    pub(crate) fit: crate::graphics::Fit,
    /// Which of the image's rows this line reserves.
    pub(crate) row: u16,
}

impl MarkdownLine {
    /// A line of `kind` with nothing else set: the one place every field gets its default, so a
    /// field added later cannot be forgotten by one constructor and not another.
    fn of(kind: MarkdownBlockKind, line: Line<'static>) -> Self {
        Self {
            kind,
            line,
            code_copy: None,
            ends_at: None,
            image: None,
            opens_image: None,
            expands: false,
            fence_rule: None,
            hang: None,
            table: None,
        }
    }

    /// Tags a styled line as readable prose.
    pub(crate) fn prose(line: Line<'static>) -> Self {
        Self::of(MarkdownBlockKind::Prose, line)
    }

    /// Tags a styled line as wide content.
    pub(crate) fn wide(line: Line<'static>) -> Self {
        Self::of(MarkdownBlockKind::Wide, line)
    }

    /// Reserves one blank wide row for a row of an inline image.
    ///
    /// Blank rather than a placeholder glyph: the pixels are painted over it once the transcript
    /// has rendered, and anything drawn here would show through an image with transparency in it.
    pub(crate) fn image_row(image: ImageRow) -> Self {
        Self { image: Some(image), ..Self::wide(Line::default()) }
    }

    /// The rule that opens a code block: the language on the left, the copy affordance on the
    /// right, and the block's own width between them.
    ///
    /// One line doing both jobs, so the copy caption cannot read as a stray word and the rule says
    /// where the code begins. Clicking anywhere along the rule copies, which is what a block header
    /// does everywhere else.
    pub(crate) fn fence_top(rule: FenceRule, code: String) -> Self {
        Self { code_copy: Some(code), fence_rule: Some(rule), ..Self::wide(Line::default()) }
    }

    /// The rule that closes a code block — no labels, and nothing to click.
    pub(crate) fn fence_bottom(rule: FenceRule) -> Self {
        Self { fence_rule: Some(rule), ..Self::wide(Line::default()) }
    }

    /// A whole table, as one line the transcript lays out at the width it is drawn in.
    pub(crate) fn table(table: TableBlock) -> Self {
        Self { table: Some(std::sync::Arc::new(table)), ..Self::wide(Line::default()) }
    }

    /// Marks this line as the handle a click uses to open or close its block.
    pub(crate) fn expandable(mut self) -> Self {
        self.expands = true;
        self
    }

    /// Marks this line as the affordance that opens `key` outside the terminal.
    pub(crate) fn opens_image(mut self, key: crate::graphics::ImageKey) -> Self {
        self.opens_image = Some(key);
        self
    }
}
