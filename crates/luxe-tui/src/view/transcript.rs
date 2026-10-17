//! transcript — the transcript's laid-out rows, kept between frames.
//!
//! Laying out a conversation is not cheap: every block is parsed as markdown, highlighted, wrapped
//! to the canvas and banded, and the result is one owned [`Line`] per screen row. Doing that for
//! the whole transcript on every frame makes the cost of a keystroke proportional to the length of
//! the session, tens of milliseconds a frame on a long one, and a held key leaves the loop behind.
//!
//! So the rows are laid out once and kept. Each **unit** — one conversation item, or one collapsed
//! run of tool calls — remembers the [`UnitSignature`] it was drawn against, and a frame that finds
//! the same signature reuses the rows instead of rebuilding them. Typing changes nothing about the
//! transcript, so it rebuilds nothing; streaming changes the tail, so it rebuilds the tail.
//!
//! Two things make that safe rather than merely fast:
//!
//! - **Content is compared by revision, not by value.** [`BlockRevision`] is a process-wide
//!   monotonic counter that only [`crate::app::ConversationItem::block_mut`] can advance, so "unchanged" is a
//!   fact the compiler helps enforce rather than a hash that might collide.
//! - **Anything that draws by itself opts out.** A running spinner, an inline image, a row the
//!   reveal is still typing out — those units are marked not reusable and rebuilt
//!   every frame, because there is genuinely nothing to reuse.

pub(super) mod layout;
mod paint;
pub(super) mod reveal;
mod rows;
pub(super) mod selection;

pub(super) use paint::render_transcript;

use crate::app::{BlockRevision, ConversationItemId};
use ratatui::text::Line;
use std::rc::Rc;

/// One laid-out row of the transcript.
///
/// `line` and `item_id` are shared rather than owned: a frame concatenates every unit's rows into
/// one list to index by scroll offset, and that copy has to be refcount bumps rather than a fresh
/// string per span.
#[derive(Debug, Clone)]
pub(crate) struct RenderedConversationLine {
    pub(crate) item_index: usize,
    pub(crate) item_id: Rc<ConversationItemId>,
    pub(crate) visual_line_within_item: usize,
    pub(crate) line: Rc<Line<'static>>,
    /// Set on a fenced code block's copy-affordance row; carries the code text for a click.
    pub(crate) code_copy: Option<Rc<str>>,
    /// Set on a row reserved for an inline image, with the column it starts at.
    pub(crate) image: Option<(crate::markdown::ImageRow, u16)>,
    /// Set on the row that offers to open an image outside the terminal.
    pub(crate) opens_image: Option<crate::graphics::ImageKey>,
    /// Whether a click on this row opens or closes its block — a card header, a truncation footer,
    /// a collapsed peek. Every other row is text to select.
    pub(crate) expands: bool,
}

/// Frame-wide inputs every unit was laid out against.
///
/// Changing any of them changes every row, so the whole cache is dropped rather than reconciled.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LayoutContext {
    pub(crate) canvas: super::ConversationCanvasWidths,
    pub(crate) card_width: u16,
    pub(crate) image_bounds: (u16, u16),
    pub(crate) diff_layout: luxe_core::config::DiffLayout,
    pub(crate) tool_output_expanded: bool,
    pub(crate) expand_chord: Option<String>,
    pub(crate) styles: crate::theme::styles::ThemeStyles,
    pub(crate) glyphs: crate::glyphs::GlyphTier,
}

/// Everything about one unit that changes what it draws.
///
/// Deliberately compared rather than hashed: the fields are a handful of words, so equality is
/// cheaper than a hash and cannot be wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct UnitSignature {
    /// First conversation item the unit covers.
    pub(crate) first: usize,
    /// Items it covers — more than one only for a collapsed run.
    pub(crate) len: usize,
    /// Highest revision among them.
    ///
    /// Sound because revisions are strictly monotonic and process-wide: editing any member raises
    /// the maximum, replacing one raises it, and removing one changes `len` or lowers it. The only
    /// change it cannot see is a reordering of the same items within one unit, which nothing does.
    pub(crate) revision: BlockRevision,
    /// Whether a blank row precedes the unit, which is true for every unit but the first drawn.
    pub(crate) leading_gap: bool,
    /// Whether the unit is drawn as one collapsed line rather than as its members.
    pub(crate) folded: bool,
    /// Whether a folded run is still expecting members.
    pub(crate) arriving: bool,
    pub(crate) selected: bool,
    pub(crate) never_sent: bool,
    pub(crate) expanded: bool,
}

/// One unit's rows, and what they were drawn against.
#[derive(Debug)]
struct LaidUnit {
    signature: UnitSignature,
    rows: Vec<RenderedConversationLine>,
    /// Rendered characters, which is what the reveal paces against.
    chars: usize,
    /// Whether a later frame may reuse these rows. False for anything that redraws by itself.
    reusable: bool,
}

/// The transcript's rows, kept between frames.
#[derive(Debug, Default)]
pub(crate) struct TranscriptLayout {
    context: Option<LayoutContext>,
    units: Vec<LaidUnit>,
    /// Every unit's rows, concatenated — what the render path indexes by scroll offset.
    rows: Vec<RenderedConversationLine>,
    /// Whether `rows` is still the concatenation of `units`.
    rows_current: bool,
    /// How far the current frame's walk has got.
    cursor: usize,
    /// Rows the walk has emitted so far, which is what decides the next unit's leading gap.
    emitted: usize,
}

impl TranscriptLayout {
    /// Starts a frame's walk, dropping everything if the frame-wide inputs moved.
    pub(crate) fn begin(&mut self, context: &LayoutContext) {
        if self.context.as_ref() != Some(context) {
            self.context = Some(context.clone());
            self.units.clear();
            self.rows_current = false;
        }
        self.cursor = 0;
        self.emitted = 0;
    }

    /// Whether the next unit is the first to draw, and so takes no leading blank row.
    pub(crate) fn at_start(&self) -> bool {
        self.emitted == 0
    }

    /// The rendered-character count of the unit at the walk's position, if it can be reused.
    ///
    /// The reveal needs this *before* it decides how much of the unit to draw, which is the whole
    /// reason a unit stores it: asking would otherwise mean laying the unit out to count.
    pub(crate) fn reusable(&self, signature: &UnitSignature) -> Option<usize> {
        let unit = self.units.get(self.cursor)?;
        (unit.reusable && unit.signature == *signature).then_some(unit.chars)
    }

    /// Keeps the unit at the walk's position and steps past it.
    pub(crate) fn keep(&mut self) {
        if let Some(unit) = self.units.get(self.cursor) {
            self.emitted += unit.rows.len();
        }
        self.cursor += 1;
    }

    /// Replaces the unit at the walk's position with freshly laid-out rows.
    pub(crate) fn replace(
        &mut self,
        signature: UnitSignature,
        rows: Vec<RenderedConversationLine>,
        chars: usize,
        reusable: bool,
    ) {
        self.emitted += rows.len();
        let unit = LaidUnit { signature, rows, chars, reusable };
        match self.units.get_mut(self.cursor) {
            Some(slot) => *slot = unit,
            None => self.units.push(unit),
        }
        self.cursor += 1;
        self.rows_current = false;
    }

    /// Ends the frame's walk, dropping any units the transcript no longer has.
    pub(crate) fn finish(&mut self) {
        if self.units.len() > self.cursor {
            self.units.truncate(self.cursor);
            self.rows_current = false;
        }
        if !self.rows_current {
            self.rows.clear();
            self.rows.reserve(self.units.iter().map(|unit| unit.rows.len()).sum());
            for unit in &self.units {
                self.rows.extend(unit.rows.iter().cloned());
            }
            self.rows_current = true;
        }
    }

    /// The transcript's rows, in order.
    pub(crate) fn rows(&self) -> &[RenderedConversationLine] {
        &self.rows
    }
}

#[cfg(test)]
mod tests;
