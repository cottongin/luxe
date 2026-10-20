//! Glyph codepoints — every character the TUI's glyph table can draw, one `U+XXXX` per line.
//!
//! `scripts/remote-fonts.sh` reads this to subset the phone app's font, so a glyph added to the
//! table reaches the app without a capture having to show it first. Every tier is listed: which
//! one a reader sees depends on their terminal, and the app draws the same glyphs.
//!
//! Run with `cargo run -q -p luxe-tui --example glyph_codepoints`.

use luxe_tui::glyphs::{GlyphTier, Icon, Mark, EVERY_ICON, EVERY_MARK};
use std::collections::BTreeSet;

const TIERS: [GlyphTier; 4] =
    [GlyphTier::Nerd, GlyphTier::NerdWide, GlyphTier::Unicode, GlyphTier::Ascii];

fn main() {
    for codepoint in codepoints() {
        println!("U+{codepoint:04X}");
    }
}

/// Every codepoint past ASCII that any icon or mark is drawn with, at any tier, in order.
fn codepoints() -> BTreeSet<u32> {
    let icons = EVERY_ICON.iter().flat_map(|icon: &Icon| TIERS.map(|tier| icon.at(tier)));
    let marks = EVERY_MARK.iter().flat_map(|mark: &Mark| TIERS.map(|tier| mark.at(tier)));
    icons.chain(marks).flat_map(str::chars).map(u32::from).filter(|&c| c > 0x7e).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A glyph from each tier is listed, not just the default one.
    ///
    /// The break this catches: listing only the Nerd tier, so a phone whose session draws the
    /// Unicode tier gets those shapes from a fallback font, in a different width.
    #[test]
    fn every_tier_is_listed() {
        let listed = codepoints();
        for tier in [GlyphTier::Nerd, GlyphTier::Unicode] {
            let glyphs = EVERY_ICON.iter().map(|icon| icon.at(tier)).flat_map(str::chars);
            let missing: Vec<char> = glyphs
                .filter(|&c| u32::from(c) > 0x7e && !listed.contains(&u32::from(c)))
                .collect();
            assert!(missing.is_empty(), "{tier:?} glyphs missing: {missing:?}");
        }
    }
}
