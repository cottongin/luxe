//! Diff settings — one layout rule for every surface that draws a diff.

use super::{known_or, Lenient};
use serde::{Deserialize, Serialize};

/// How a diff is drawn, wherever luxe draws one.
///
/// One setting for three surfaces — the `edit`/`write` tool card, the peeked file beside the
/// conversation, and Guided Review. They differ only in how much room they have, and
/// [`DiffLayout::splits`] is handed that room rather than each surface keeping an opinion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffLayout {
    /// Side by side where the surface can give each column a readable width, unified below that.
    Auto,
    /// Always one column.
    Unified,
    /// Side by side down to a cramped column, for a reader who would rather have the pairing.
    Split,
    /// Unknown values fall back to automatic behavior.
    #[serde(other)]
    Unknown,
}

impl Lenient for DiffLayout {
    const UNKNOWN: Self = Self::Unknown;
    const DEFAULT: Self = Self::Auto;
    const DEFAULT_NAME: &'static str = "auto";
    const NAME: &'static str = "diff layout";
}

/// Cells of code a column needs before [`DiffLayout::Auto`] prefers two of them to one.
///
/// Measured after the column's own line-number gutter, so it is the same quantity on a tool card,
/// a peeked file and a full-screen review. Each converts it using the chrome it knows about, so
/// each reaches the figure at a different terminal width: a review spends a third of a wide
/// terminal on its guide pane and a peeked file shares its column with the conversation, so they
/// are legitimately later to split than a card that has the whole width.
///
/// Chosen by measuring all three: at 44 a terminal of 160 columns pairs the sides everywhere and
/// one of 100 pairs them nowhere. Below it a line of ordinary source wraps more than once and the
/// row stops reading as a row, though it is never *lost*, since every surface wraps rather than
/// clips.
pub const MIN_SPLIT_COLUMN: usize = 44;

/// Cells of code a column needs before [`DiffLayout::Split`] will draw two of them.
///
/// Cramped, and deliberately reachable: someone who set `split` would rather read a wrapped pairing
/// than a unified list, and the only thing left to protect them from is a column too narrow to be
/// a column.
pub const SPLIT_FLOOR_COLUMN: usize = 28;

impl DiffLayout {
    /// Whether a surface offering `column_cells` of code per side draws two columns.
    ///
    /// The three settings differ on exactly one axis — how much width they demand before pairing
    /// the sides up: never, [`MIN_SPLIT_COLUMN`], or down to [`SPLIT_FLOOR_COLUMN`].
    ///
    /// `two_sided` is whether the change has both a removal and an addition to pair. An
    /// all-additions block has nothing for the left column, so it stays unified on every setting.
    pub fn splits(self, column_cells: usize, two_sided: bool) -> bool {
        if !two_sided {
            return false;
        }
        match self {
            Self::Unified => false,
            Self::Split => column_cells >= SPLIT_FLOOR_COLUMN,
            Self::Auto | Self::Unknown => column_cells >= MIN_SPLIT_COLUMN,
        }
    }
}

/// How every diff is drawn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct DiffConfig {
    /// One column or two. Omitted = [`DiffLayout::Auto`].
    pub layout: Option<DiffLayout>,
    /// Soft-wrap long lines rather than scrolling them sideways. Omitted = scroll.
    ///
    /// Only two of the three surfaces have a choice here: a tool card and a peeked file cannot
    /// scroll sideways, so they always wrap — clipping there would put content out of reach rather
    /// than one keystroke away. Guided Review can, so this is its default, and `w` flips it live.
    pub word_wrap: Option<bool>,
}

impl DiffConfig {
    /// Resolves the layout, safely defaulting unknown values to automatic.
    pub fn layout(&self) -> DiffLayout {
        known_or(self.layout)
    }

    /// Whether long lines soft-wrap where the surface could have scrolled them instead.
    pub fn word_wrap(&self) -> bool {
        self.word_wrap.unwrap_or(false)
    }
}
