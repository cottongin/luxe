//! kitty — the graphics protocol in the one shape a cell buffer can hold.
//!
//! An image is transmitted once as a *virtual placement* (`U=1`), which displays nothing. It
//! appears only where the client prints [`PLACEHOLDER`] cells whose foreground color carries the
//! image id and whose combining diacritics carry the (row, column) of the image that cell shows.
//! Those are ordinary characters in the terminal's grid, so the terminal owns scrolling, clipping
//! and erasure — and printing a subset of the rows shows exactly that subset of the image, which is
//! what a scrolling viewport needs and what the alternative (cursor-positioned placements plus
//! source-rectangle math, re-derived every frame) never stops costing.
//!
//! # Every cell says where it is
//!
//! The protocol lets a bare placeholder inherit its coordinates from the cell to its left, and the
//! obvious optimization is to address only the first cell of each row. We address all of them. A
//! cell buffer is repainted by *diff*, so any single cell may be rewritten alone, and a run whose
//! meaning depends on its neighbors has a way to be wrong that a self-describing cell does not.
//! The cost is about ten bytes per cell, which stays inside ratatui's inline symbol storage.
//!
//! Nothing here writes to the terminal, and nothing here belongs in the cell buffer except
//! [`placeholder`] and [`id_color`]. [`transmit`] and [`delete`] are written to the stream directly
//! by the store that owns the image's lifetime — measured on kitty and Ghostty, placeholders
//! printed *before* the image they name resolve as soon as it arrives, so the two are independent.

use base64::Engine as _;
use ratatui::style::Color;
use std::fmt::Write as _;

/// The private-use codepoint a terminal substitutes an image cell for.
const PLACEHOLDER: char = '\u{10EEEE}';

/// Base64 payload one graphics command may carry. Fixed by the protocol.
const CHUNK: usize = 4096;

/// Cells an image may span in either direction.
///
/// The coordinate alphabet is finite, and a truncated one does not fail loudly: the indices wrap
/// and the image silently tiles itself across the row.
pub(super) const MAX_CELLS: u16 = DIACRITICS.len() as u16;

/// Largest image id we hand out.
///
/// Ids are global to the terminal and the low 24 bits travel as a foreground color, so staying
/// inside 24 bits keeps the id exactly recoverable from that color and leaves the most-significant
/// byte permanently zero — one less thing for a terminal to disagree about.
pub(super) const MAX_ID: u32 = 0x00FF_FFFF;

/// Transmits `png` as image `id` and creates a virtual placement `cols`×`rows` cells in size.
///
/// The image is fitted to that rectangle with its aspect ratio preserved, so a rectangle that does
/// not match the source letterboxes rather than distorting. Re-transmitting an id replaces the
/// image and drops its placements, which is how a resize is applied.
///
/// Sent inline (`t=d`) rather than by shared file: a 263 KB screenshot measures 343 KB of base64
/// across 86 chunks in about 6 ms, which is not worth assuming the terminal can read our
/// filesystem — an assumption that is wrong over ssh and inside a container, and that owes a
/// temp-file cleanup story either way.
pub(super) fn transmit(id: u32, png: &[u8], cols: u16, rows: u16) -> String {
    let payload = base64::engine::general_purpose::STANDARD.encode(png);
    let mut out = String::with_capacity(payload.len() + payload.len() / CHUNK * 24 + 64);
    let mut chunks = payload.as_bytes().chunks(CHUNK).peekable();
    let mut first = true;
    while let Some(chunk) = chunks.next() {
        let more = u8::from(chunks.peek().is_some());
        // Quiet on every command: an unsuppressed `OK` arrives on stdin, where it is
        // indistinguishable from the user typing.
        out.push_str("\x1b_Gq=2,");
        if first {
            let _ = write!(out, "i={id},a=T,U=1,f=100,t=d,c={cols},r={rows},");
            first = false;
        }
        let _ = write!(out, "m={more};");
        out.push_str(&String::from_utf8_lossy(chunk));
        out.push_str("\x1b\\");
    }
    out
}

/// Frees image `id` and every placement made from it.
///
/// Uppercase `I` releases the pixels as well. Deleting only the placement leaves the image in the
/// terminal's memory, which is how a long session ends up holding every screenshot it ever showed.
pub(super) fn delete(id: u32) -> String {
    format!("\x1b_Ga=d,d=I,i={id},q=2\x1b\\")
}

/// The foreground color that names image `id`.
///
/// Set as an ordinary cell style, so the backend emits the color change itself — and emits it once
/// per run of cells that share it, rather than once per cell.
pub(super) fn id_color(id: u32) -> Color {
    Color::Rgb((id >> 16) as u8, (id >> 8) as u8, id as u8)
}

/// The symbol for the cell showing `(row, col)` of an image.
///
/// One grapheme, one cell wide: a private-use base character plus three combining marks, the third
/// being the image id's most significant byte — always zero here, and still written, because the
/// first cell of a row has nothing to its left to inherit it from.
pub(super) fn placeholder(row: u16, col: u16) -> String {
    let mut symbol = String::with_capacity(16);
    symbol.push(PLACEHOLDER);
    symbol.push(diacritic(row));
    symbol.push(diacritic(col));
    symbol.push(diacritic(0));
    symbol
}

/// The diacritic encoding a row or column index.
///
/// Out-of-range folds to zero rather than panicking: a mis-sized image should draw wrong, not take
/// the session down. [`MAX_CELLS`] keeps callers from getting there.
fn diacritic(index: u16) -> char {
    DIACRITICS.get(usize::from(index)).copied().unwrap_or(DIACRITICS[0])
}

/// Row/column coordinate alphabet, from the protocol's `rowcolumn-diacritics.txt`.
///
/// Every entry is a combining mark, so a placeholder plus its coordinates is one grapheme one cell
/// wide — the property that lets an image sit in the text grid at all.
static DIACRITICS: [char; 297] = [
    '\u{305}',
    '\u{30D}',
    '\u{30E}',
    '\u{310}',
    '\u{312}',
    '\u{33D}',
    '\u{33E}',
    '\u{33F}',
    '\u{346}',
    '\u{34A}',
    '\u{34B}',
    '\u{34C}',
    '\u{350}',
    '\u{351}',
    '\u{352}',
    '\u{357}',
    '\u{35B}',
    '\u{363}',
    '\u{364}',
    '\u{365}',
    '\u{366}',
    '\u{367}',
    '\u{368}',
    '\u{369}',
    '\u{36A}',
    '\u{36B}',
    '\u{36C}',
    '\u{36D}',
    '\u{36E}',
    '\u{36F}',
    '\u{483}',
    '\u{484}',
    '\u{485}',
    '\u{486}',
    '\u{487}',
    '\u{592}',
    '\u{593}',
    '\u{594}',
    '\u{595}',
    '\u{597}',
    '\u{598}',
    '\u{599}',
    '\u{59C}',
    '\u{59D}',
    '\u{59E}',
    '\u{59F}',
    '\u{5A0}',
    '\u{5A1}',
    '\u{5A8}',
    '\u{5A9}',
    '\u{5AB}',
    '\u{5AC}',
    '\u{5AF}',
    '\u{5C4}',
    '\u{610}',
    '\u{611}',
    '\u{612}',
    '\u{613}',
    '\u{614}',
    '\u{615}',
    '\u{616}',
    '\u{617}',
    '\u{657}',
    '\u{658}',
    '\u{659}',
    '\u{65A}',
    '\u{65B}',
    '\u{65D}',
    '\u{65E}',
    '\u{6D6}',
    '\u{6D7}',
    '\u{6D8}',
    '\u{6D9}',
    '\u{6DA}',
    '\u{6DB}',
    '\u{6DC}',
    '\u{6DF}',
    '\u{6E0}',
    '\u{6E1}',
    '\u{6E2}',
    '\u{6E4}',
    '\u{6E7}',
    '\u{6E8}',
    '\u{6EB}',
    '\u{6EC}',
    '\u{730}',
    '\u{732}',
    '\u{733}',
    '\u{735}',
    '\u{736}',
    '\u{73A}',
    '\u{73D}',
    '\u{73F}',
    '\u{740}',
    '\u{741}',
    '\u{743}',
    '\u{745}',
    '\u{747}',
    '\u{749}',
    '\u{74A}',
    '\u{7EB}',
    '\u{7EC}',
    '\u{7ED}',
    '\u{7EE}',
    '\u{7EF}',
    '\u{7F0}',
    '\u{7F1}',
    '\u{7F3}',
    '\u{816}',
    '\u{817}',
    '\u{818}',
    '\u{819}',
    '\u{81B}',
    '\u{81C}',
    '\u{81D}',
    '\u{81E}',
    '\u{81F}',
    '\u{820}',
    '\u{821}',
    '\u{822}',
    '\u{823}',
    '\u{825}',
    '\u{826}',
    '\u{827}',
    '\u{829}',
    '\u{82A}',
    '\u{82B}',
    '\u{82C}',
    '\u{82D}',
    '\u{951}',
    '\u{953}',
    '\u{954}',
    '\u{F82}',
    '\u{F83}',
    '\u{F86}',
    '\u{F87}',
    '\u{135D}',
    '\u{135E}',
    '\u{135F}',
    '\u{17DD}',
    '\u{193A}',
    '\u{1A17}',
    '\u{1A75}',
    '\u{1A76}',
    '\u{1A77}',
    '\u{1A78}',
    '\u{1A79}',
    '\u{1A7A}',
    '\u{1A7B}',
    '\u{1A7C}',
    '\u{1B6B}',
    '\u{1B6D}',
    '\u{1B6E}',
    '\u{1B6F}',
    '\u{1B70}',
    '\u{1B71}',
    '\u{1B72}',
    '\u{1B73}',
    '\u{1CD0}',
    '\u{1CD1}',
    '\u{1CD2}',
    '\u{1CDA}',
    '\u{1CDB}',
    '\u{1CE0}',
    '\u{1DC0}',
    '\u{1DC1}',
    '\u{1DC3}',
    '\u{1DC4}',
    '\u{1DC5}',
    '\u{1DC6}',
    '\u{1DC7}',
    '\u{1DC8}',
    '\u{1DC9}',
    '\u{1DCB}',
    '\u{1DCC}',
    '\u{1DD1}',
    '\u{1DD2}',
    '\u{1DD3}',
    '\u{1DD4}',
    '\u{1DD5}',
    '\u{1DD6}',
    '\u{1DD7}',
    '\u{1DD8}',
    '\u{1DD9}',
    '\u{1DDA}',
    '\u{1DDB}',
    '\u{1DDC}',
    '\u{1DDD}',
    '\u{1DDE}',
    '\u{1DDF}',
    '\u{1DE0}',
    '\u{1DE1}',
    '\u{1DE2}',
    '\u{1DE3}',
    '\u{1DE4}',
    '\u{1DE5}',
    '\u{1DE6}',
    '\u{1DFE}',
    '\u{20D0}',
    '\u{20D1}',
    '\u{20D4}',
    '\u{20D5}',
    '\u{20D6}',
    '\u{20D7}',
    '\u{20DB}',
    '\u{20DC}',
    '\u{20E1}',
    '\u{20E7}',
    '\u{20E9}',
    '\u{20F0}',
    '\u{2CEF}',
    '\u{2CF0}',
    '\u{2CF1}',
    '\u{2DE0}',
    '\u{2DE1}',
    '\u{2DE2}',
    '\u{2DE3}',
    '\u{2DE4}',
    '\u{2DE5}',
    '\u{2DE6}',
    '\u{2DE7}',
    '\u{2DE8}',
    '\u{2DE9}',
    '\u{2DEA}',
    '\u{2DEB}',
    '\u{2DEC}',
    '\u{2DED}',
    '\u{2DEE}',
    '\u{2DEF}',
    '\u{2DF0}',
    '\u{2DF1}',
    '\u{2DF2}',
    '\u{2DF3}',
    '\u{2DF4}',
    '\u{2DF5}',
    '\u{2DF6}',
    '\u{2DF7}',
    '\u{2DF8}',
    '\u{2DF9}',
    '\u{2DFA}',
    '\u{2DFB}',
    '\u{2DFC}',
    '\u{2DFD}',
    '\u{2DFE}',
    '\u{2DFF}',
    '\u{A66F}',
    '\u{A67C}',
    '\u{A67D}',
    '\u{A6F0}',
    '\u{A6F1}',
    '\u{A8E0}',
    '\u{A8E1}',
    '\u{A8E2}',
    '\u{A8E3}',
    '\u{A8E4}',
    '\u{A8E5}',
    '\u{A8E6}',
    '\u{A8E7}',
    '\u{A8E8}',
    '\u{A8E9}',
    '\u{A8EA}',
    '\u{A8EB}',
    '\u{A8EC}',
    '\u{A8ED}',
    '\u{A8EE}',
    '\u{A8EF}',
    '\u{A8F0}',
    '\u{A8F1}',
    '\u{AAB0}',
    '\u{AAB2}',
    '\u{AAB3}',
    '\u{AAB7}',
    '\u{AAB8}',
    '\u{AABE}',
    '\u{AABF}',
    '\u{AAC1}',
    '\u{FE20}',
    '\u{FE21}',
    '\u{FE22}',
    '\u{FE23}',
    '\u{FE24}',
    '\u{FE25}',
    '\u{FE26}',
    '\u{10A0F}',
    '\u{10A38}',
    '\u{1D185}',
    '\u{1D186}',
    '\u{1D187}',
    '\u{1D188}',
    '\u{1D189}',
    '\u{1D1AA}',
    '\u{1D1AB}',
    '\u{1D1AC}',
    '\u{1D1AD}',
    '\u{1D242}',
    '\u{1D243}',
    '\u{1D244}',
];

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_width::UnicodeWidthStr;

    /// A payload under the chunk limit is one command, and it declares everything the terminal
    /// needs to store the image without displaying it.
    #[test]
    fn a_small_image_transmits_as_one_virtual_placement() {
        let sequence = transmit(42, b"pretend-png", 20, 5);
        assert_eq!(sequence.matches("\x1b_G").count(), 1, "one command: {sequence:?}");
        assert!(sequence.starts_with("\x1b_Gq=2,i=42,a=T,U=1,f=100,t=d,c=20,r=5,m=0;"));
        assert!(sequence.ends_with("\x1b\\"));
        assert!(sequence.contains(&base64::engine::general_purpose::STANDARD.encode("pretend-png")));
    }

    /// The protocol caps a command's payload, and every chunk but the last says more is coming.
    /// Only the first carries the image's parameters; the rest may carry `m` and `q` and nothing
    /// else, which is the rule a hand-rolled chunker gets wrong.
    #[test]
    fn a_large_image_chunks_with_only_m_and_q_after_the_first() {
        let png = vec![0xA5u8; CHUNK * 2];
        let sequence = transmit(7, &png, 10, 4);
        let commands: Vec<&str> = sequence.split("\x1b_G").skip(1).collect();
        assert!(commands.len() > 2, "a payload over the limit is chunked: {}", commands.len());

        let controls = |command: &str| command.split(';').next().unwrap_or_default().to_string();
        assert!(controls(commands[0]).contains("i=7"), "the first chunk declares the image");
        for command in &commands[1..] {
            for key in controls(command).split(',').filter(|key| !key.is_empty()) {
                let name = key.split('=').next().unwrap_or_default();
                assert!(matches!(name, "m" | "q"), "chunk carries only m/q, found {key:?}");
            }
        }
        assert!(commands.last().unwrap().contains("m=0;"), "the last chunk closes the sequence");
        assert_eq!(sequence.matches("m=1;").count(), commands.len() - 1);
    }

    /// The invariant the whole approach rests on: a placeholder plus its coordinates measures one
    /// cell. If it ever measured two, every row an image sits in would shear.
    #[test]
    fn a_placeholder_is_one_grapheme_one_cell_wide() {
        let symbol = placeholder(3, 9);
        assert_eq!(symbol.chars().count(), 4, "base character plus three coordinates");
        assert_eq!(symbol.width(), 1, "and it measures one cell: {symbol:?}");
        assert!(symbol.len() <= 24, "stays inside ratatui's inline symbol storage");
    }

    /// Every cell says exactly where it is, so a diff that rewrites one cell alone still resolves.
    #[test]
    fn every_cell_carries_its_own_coordinates() {
        assert_eq!(placeholder(0, 0), format!("{PLACEHOLDER}\u{305}\u{305}\u{305}"));
        assert_eq!(placeholder(1, 2), format!("{PLACEHOLDER}\u{30D}\u{30E}\u{305}"));
        assert_ne!(placeholder(1, 2), placeholder(2, 1), "row and column are not interchangeable");
    }

    /// The id has to survive the round trip through a foreground color, because that color is the
    /// only place the terminal reads it from.
    #[test]
    fn the_id_travels_intact_as_a_foreground_color() {
        let id = 0x0A_0B0C;
        assert_eq!(id_color(id), Color::Rgb(0x0A, 0x0B, 0x0C));
        let Color::Rgb(r, g, b) = id_color(MAX_ID) else { panic!("rgb") };
        assert_eq!(u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b), MAX_ID);
    }

    /// Uppercase `I` frees the pixels too; deleting only the placement leaks them.
    #[test]
    fn deleting_an_image_frees_its_data() {
        assert_eq!(delete(99), "\x1b_Ga=d,d=I,i=99,q=2\x1b\\");
    }

    /// The coordinate alphabet has to be the protocol's, in the protocol's order and at its full
    /// length. A short table does not fail loudly — the indices wrap and the image tiles itself
    /// across the row, which is exactly what a truncated table produced in testing.
    #[test]
    fn the_coordinate_alphabet_is_complete_and_in_order() {
        assert_eq!(DIACRITICS.len(), 297);
        assert_eq!(MAX_CELLS, 297);
        assert_eq!(diacritic(0), '\u{305}');
        assert_eq!(diacritic(1), '\u{30D}');
        assert_eq!(diacritic(2), '\u{30E}');
        assert_eq!(diacritic(296), '\u{1D244}');
        assert_eq!(diacritic(u16::MAX), DIACRITICS[0], "out of range folds, never panics");

        let mut seen = DIACRITICS.to_vec();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), DIACRITICS.len(), "no duplicate coordinates");
        // Every entry must be zero-width, or a placeholder stops being one cell.
        for mark in DIACRITICS {
            assert_eq!(mark.to_string().width(), 0, "{mark:?} is not a combining mark");
        }
    }
}
