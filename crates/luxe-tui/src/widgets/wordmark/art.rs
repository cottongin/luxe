//! Art — the logotype's cells, exactly as they were drawn.
//!
//! Three glyphs, and that is the whole portability story: the upper half block, the lower half
//! block, and the full block (`U+2580`, `U+2584`, `U+2588`). Every terminal with built-in box
//! drawing synthesizes those against the cell box rather than scaling a font outline into it, so
//! the mark lands identically whether the reader has a patched Nerd Font or the plainest monospace
//! their distribution shipped. Verified rather than assumed: it renders the same in Liberation
//! Mono, which contains no Powerline or Nerd glyphs at all.
//!
//! Not the Powerline Extra triangles: those are private use, every terminal invents its own shape
//! for them, and some draw them the wrong size. One mark that is right everywhere beats two marks
//! and a setting asking which terminal you have.
//!
//! Whatever arrives has to satisfy what the renderer assumes, and the tests below check exactly
//! that: equal-width rows, nothing but the three blocks, and five separated groups of ink — the
//! four letters and the full stop — for the ramp to color.

/// The full 44×7 logotype.
pub(super) const FULL_ART: [&str; 7] = [
    " ▄▄▄▄                                       ",
    " ▀▀██                                       ",
    "   ██      ██    ██  ▀██  ██▀   ▄████▄      ",
    "   ██      ██    ██    ████    ██▄▄▄▄██     ",
    "   ██      ██    ██    ▄██▄    ██▀▀▀▀▀▀     ",
    "   ██▄▄▄   ██▄▄▄███   ▄█▀▀█▄   ▀██▄▄▄▄█   ██",
    "    ▀▀▀▀    ▀▀▀▀ ▀▀  ▀▀▀  ▀▀▀    ▀▀▀▀▀    ▀▀",
];

/// A 32×5 reduction for a terminal too narrow or too short for [`FULL_ART`].
///
/// The same drawing at a smaller size rather than a different one, so the mark does not
/// change character when the window does.
pub(super) const SMALL_ART: [&str; 5] = [
    " ▀▀█                            ",
    "   █    ▄   ▄  ▄   ▄   ▄▄▄      ",
    "   █    █   █   █▄█   █▀  █     ",
    "   █    █   █   ▄█▄   █▀▀▀▀     ",
    "   ▀▄▄  ▀▄▄▀█  ▄▀ ▀▄  ▀█▄▄▀    █",
];

/// What is left when even [`SMALL_ART`] will not fit: the wordmark, set as text.
pub(super) const LINE_ART: [&str; 1] = ["luxe."];

/// The only glyphs the art may contain: the two half blocks and the full block.
///
/// Not a style preference. This range is the one every terminal draws itself at exact cell
/// fractions, which is what lets the mark look the same on a machine that has never heard of a
/// patched font.
#[cfg(test)]
pub(super) const BLOCKS: [char; 3] = ['\u{2580}', '\u{2584}', '\u{2588}'];

/// Which group of ink owns each column: `0` for the `l`, up to `4` for the full stop.
///
/// Derived from the drawing rather than declared beside it, because a table of letter boundaries is
/// a second copy of the art that drifts the first time anyone nudges a stem. Columns with no ink
/// take their neighbor's group, so a gap never resets the ramp mid-letter.
pub(super) fn column_groups(rows: &[&str]) -> Vec<usize> {
    let width = rows.iter().map(|row| row.chars().count()).max().unwrap_or(0);
    let inked: Vec<bool> = (0..width)
        .map(|column| rows.iter().any(|row| row.chars().nth(column).is_some_and(|c| c != ' ')))
        .collect();

    let mut groups = vec![0usize; width];
    let mut group = 0;
    let mut seen_ink = false;
    for column in 0..width {
        if inked[column] {
            // A run of blank columns between two inked ones ends the group before it.
            if seen_ink && !inked[column.saturating_sub(1)] {
                group += 1;
            }
            seen_ink = true;
        }
        groups[column] = group;
    }
    groups
}

#[cfg(test)]
mod shape {
    use super::*;

    /// Everything the renderer cannot draw correctly, as an error.
    pub(super) fn validate(rows: &[&str]) -> Result<usize, String> {
        // Ragged rows shear a centerd logotype. Counted in *chars*: every glyph here is multi-byte,
        // so comparing `len()` would measure the wrong thing.
        let widths: std::collections::BTreeSet<usize> =
            rows.iter().map(|row| row.chars().count()).collect();
        if widths.len() != 1 {
            return Err(format!("rows are not all the same width ({widths:?})"));
        }
        if rows.iter().all(|row| row.trim().is_empty()) {
            return Err("blank art".into());
        }

        // The portability guarantee, and the whole reason this art exists.
        for glyph in rows.iter().flat_map(|row| row.chars()) {
            if glyph != ' ' && !BLOCKS.contains(&glyph) {
                return Err(format!("U+{:04X} is not one of the three blocks", glyph as u32));
            }
        }

        let groups = column_groups(rows);
        let count = groups.last().map(|last| last + 1).unwrap_or(0);
        if count != 5 {
            return Err(format!("expected four letters and a full stop, found {count} ink groups"));
        }
        Ok(count)
    }

    #[test]
    fn the_shipped_art_is_drawable() {
        for (name, rows) in [("full", &FULL_ART[..]), ("small", &SMALL_ART[..])] {
            validate(rows).unwrap_or_else(|error| panic!("{name}: {error}"));
        }
    }

    /// The line size is text, not a drawing, so it answers to nothing above — but it is still the
    /// wordmark and has to say so.
    #[test]
    fn the_line_size_is_the_wordmark() {
        assert_eq!(LINE_ART, ["luxe."]);
    }

    /// Every letter, and the stop, gets its own step of the ramp.
    #[test]
    fn the_ramp_finds_five_groups() {
        for rows in [&FULL_ART[..], &SMALL_ART[..]] {
            let groups = column_groups(rows);
            assert_eq!(groups.last().copied(), Some(4), "five groups, zero-indexed");
            // Monotonic: a group never comes back after the next one starts.
            assert!(groups.windows(2).all(|pair| pair[1] >= pair[0]), "groups run left to right");
        }
    }

    /// The guard is fed art it must reject, because a check that only ever sees correct input
    /// proves nothing.
    #[test]
    fn the_guard_has_teeth() {
        let cases: [(&str, Vec<&str>); 5] = [
            ("ragged", vec!["\u{2588}\u{2588}", "\u{2588}"]),
            ("blank", vec!["   ", "   "]),
            ("a foreign glyph", vec!["\u{2588}\u{e0b8}\u{2588}", "\u{2588} \u{2588}"]),
            ("too few groups", vec!["\u{2588} \u{2588} \u{2588}", "\u{2588} \u{2588} \u{2588}"]),
            (
                "too many groups",
                vec![
                    "\u{2588} \u{2588} \u{2588} \u{2588} \u{2588} \u{2588}",
                    "\u{2588} \u{2588} \u{2588} \u{2588} \u{2588} \u{2588}",
                ],
            ),
        ];
        for (what, rows) in cases {
            assert!(validate(&rows).is_err(), "{what} should have been rejected");
        }
    }
}
