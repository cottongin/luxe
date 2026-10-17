//! Glyph formatting — how a tier spells its icons, and the helpers that set an icon beside words.

use super::detect::nerd_icons_available;
use super::state::tier;
use super::{GlyphTier, Icon};
use luxe_core::config::GlyphStyle;
use unicode_width::UnicodeWidthStr;

impl GlyphTier {
    /// The tier a preference resolves to, asking the system only when it says `auto`.
    ///
    /// `auto` never picks [`Self::NerdWide`], because the question it would have to answer — which
    /// cut of the face the terminal loaded — has no answer: both report a one-cell advance.
    ///
    /// Knowing the terminal does not answer it either. Ghostty and kitty spread an icon into a
    /// following space only when the face drew it wide — the `Mono` cut stays in its cell under
    /// Ghostty, measured — so they inherit the question rather than settle it.
    pub fn resolve(preference: GlyphStyle) -> Self {
        match preference {
            GlyphStyle::Nerd => Self::Nerd,
            GlyphStyle::NerdWide => Self::NerdWide,
            GlyphStyle::Unicode => Self::Unicode,
            GlyphStyle::Ascii => Self::Ascii,
            GlyphStyle::Auto | GlyphStyle::Unknown => {
                if nerd_icons_available() {
                    Self::Nerd
                } else {
                    Self::Unicode
                }
            }
        }
    }

    /// The clearance between an icon and the words it labels, at this tier.
    ///
    /// Two cells where the face draws its icons two cells wide, so the first of them is the one the
    /// icon is already painting on. Everywhere else the usual single space.
    pub const fn gap(self) -> &'static str {
        match self {
            Self::NerdWide => "  ",
            Self::Nerd | Self::Unicode | Self::Ascii => " ",
        }
    }
}

/// The clearance between an icon and the words it labels, at the resolved tier.
///
/// One owner for the gap, because it is spent in about thirty places and a surface that hard-codes
/// its own space is a surface that stops moving with the rest of them.
pub fn icon_gap() -> &'static str {
    tier().gap()
}

/// `glyph` and the clearance after it, or nothing where the tier draws no glyph.
pub fn icon_prefix(glyph: &str) -> String {
    if glyph.is_empty() {
        String::new()
    } else {
        format!("{glyph}{}", icon_gap())
    }
}

/// A glyph the reader configured, where Nerd Font icons are drawn, and `fallback` elsewhere.
///
/// What a person puts in a config is most likely a Nerd Font mark, which is tofu at the Unicode and
/// ASCII tiers; there, and when they named none, the fallback's own rendering stands in.
pub fn configured_glyph(configured: Option<&str>, fallback: Icon) -> &str {
    match (configured, tier()) {
        (Some(glyph), GlyphTier::Nerd | GlyphTier::NerdWide) => glyph,
        _ => fallback.glyph(),
    }
}

/// `{glyph}{gap}{label}`, or bare `label` where the tier draws no glyph.
///
/// The shape all but a handful of rows in the product are built from. An omitted icon takes its
/// clearance with it, so the words land on the same column at every tier rather than one cell in
/// from where they belong.
pub fn icon_label(glyph: &str, label: &str) -> String {
    format!("{}{label}", icon_prefix(glyph))
}

/// Cells `glyph` and its clearance occupy — zero where the tier draws none.
///
/// For the surfaces that have to *measure* a row before drawing it: a panel sizing its box to its own
/// title, a dock row working out what is left for a path.
pub fn icon_cells(glyph: &str) -> usize {
    if glyph.is_empty() {
        0
    } else {
        UnicodeWidthStr::width(glyph) + UnicodeWidthStr::width(icon_gap())
    }
}
