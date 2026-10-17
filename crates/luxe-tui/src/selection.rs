//! Text selection — grid selection shared by the transcript and the composer.
//!
//! Capturing the mouse means the terminal's own click-drag selection never fires, so Luxe draws
//! and owns the selection itself: a drag records an anchor and a moving cursor over a surface's
//! rendered lines, the view paints the covered cells, and releasing copies the text out. Positions
//! are `(line, column)` in *terminal cells*, matching what the user actually sees, so wide glyphs
//! and tabs select the way they render.

use std::ops::Range;
use std::time::{Duration, Instant};

use unicode_width::UnicodeWidthChar;

/// How much of the text under a press the gesture claims.
///
/// The three clicks a terminal gives you and mouse capture takes away: one places a caret, two grab
/// the word, three grab the line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SelectionSpan {
    /// Exactly where the pointer is — a drag grows it from here.
    #[default]
    Caret,
    /// The word under the pointer (see [`is_word_char`]).
    Word,
    /// The whole line under the pointer.
    Line,
}

/// How long a second press keeps chaining onto the first.
///
/// Generous, because the gesture is a *deliberate* double-click rather than an accident, and a
/// missed one silently degrades to two clicks.
pub const MULTI_CLICK_WINDOW: Duration = Duration::from_millis(450);

/// The last left press, so the next one can tell a chained click from a fresh one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PressChain {
    /// Cell the press landed on.
    pub cell: (u16, u16),
    /// When it landed.
    pub at: Instant,
    /// What that press claimed, which the next one in the chain escalates.
    pub span: SelectionSpan,
}

impl PressChain {
    /// What a press at `cell` and `now` claims, given the press before it.
    ///
    /// Escalates caret → word → line while the presses stay on the same cell inside the window,
    /// then starts over: a fourth click is a caret again, which is how a terminal behaves.
    pub fn next(previous: Option<Self>, cell: (u16, u16), now: Instant) -> Self {
        let span = match previous {
            Some(previous)
                if previous.cell == cell
                    && now.duration_since(previous.at) <= MULTI_CLICK_WINDOW =>
            {
                match previous.span {
                    SelectionSpan::Caret => SelectionSpan::Word,
                    SelectionSpan::Word => SelectionSpan::Line,
                    SelectionSpan::Line => SelectionSpan::Caret,
                }
            }
            _ => SelectionSpan::Caret,
        };
        Self { cell, at: now, span }
    }
}

/// A position in a text surface's rendered grid.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct TextPoint {
    /// Line index within the surface's rendered lines.
    pub line: usize,
    /// Terminal cell column within that line.
    pub column: usize,
}

impl TextPoint {
    /// A position at `line`/`column`.
    pub fn new(line: usize, column: usize) -> Self {
        Self { line, column }
    }
}

/// A selection between a fixed anchor and a moving cursor. Either may come first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridSelection {
    /// Where the drag (or shift-select) started.
    pub anchor: TextPoint,
    /// Where the pointer is now.
    pub cursor: TextPoint,
}

impl GridSelection {
    /// A zero-width selection at `at` — what a plain click leaves behind.
    pub fn collapsed(at: TextPoint) -> Self {
        Self { anchor: at, cursor: at }
    }

    /// The whole of `line`, however wide it turns out to be.
    ///
    /// The cursor runs past any real line because [`Self::columns_on`] clamps to the width the
    /// frame reports — which is the only place the rendered width is known.
    pub fn whole_line(line: usize) -> Self {
        Self { anchor: TextPoint::new(line, 0), cursor: TextPoint::new(line, usize::MAX) }
    }

    /// Whether nothing is actually covered.
    pub fn is_empty(&self) -> bool {
        self.anchor == self.cursor
    }

    /// The selection ordered as `(start, end)` regardless of drag direction.
    pub fn bounds(&self) -> (TextPoint, TextPoint) {
        if self.anchor <= self.cursor {
            (self.anchor, self.cursor)
        } else {
            (self.cursor, self.anchor)
        }
    }

    /// The `[start, end)` cell range covered on `line`, given that line's rendered width.
    pub fn columns_on(&self, line: usize, line_width: usize) -> Option<Range<usize>> {
        let (start, end) = self.bounds();
        if self.is_empty() || line < start.line || line > end.line {
            return None;
        }
        let from = if line == start.line { start.column } else { 0 };
        let to = if line == end.line { end.column } else { line_width };
        (from < to).then(|| from..to.min(line_width.max(from)))
    }

    /// The selected text across `lines`, one entry per rendered line.
    ///
    /// Trailing cells are trimmed per line: a transcript line is padded out to the band width, and
    /// nobody wants that padding on the clipboard.
    pub fn extract(&self, lines: &[String]) -> String {
        let (start, end) = self.bounds();
        if self.is_empty() {
            return String::new();
        }
        let mut out: Vec<String> = Vec::new();
        for line in start.line..=end.line.min(lines.len().saturating_sub(1)) {
            let Some(text) = lines.get(line) else { break };
            let width = cell_width(text);
            match self.columns_on(line, width) {
                Some(range) => out.push(slice_columns(text, range).trim_end().to_string()),
                None => out.push(String::new()),
            }
        }
        out.join("\n")
    }
}

/// Total terminal-cell width of `text`.
pub fn cell_width(text: &str) -> usize {
    text.chars().map(|character| character.width().unwrap_or(0)).sum()
}

/// Whether `character` belongs to the same double-click word as its neighbors.
///
/// Greedier than a natural-language word on purpose: what a reader double-clicks in a coding
/// transcript is an identifier, a path, or a URL, and handing back half of `Parser::feed` or half a
/// URL is the failure worth avoiding. Brackets, quotes, commas and semicolons stay separators.
pub fn is_word_char(character: char) -> bool {
    character.is_alphanumeric()
        || matches!(
            character,
            '_' | '-'
                | '.'
                | '/'
                | '\\'
                | ':'
                | '~'
                | '@'
                | '#'
                | '$'
                | '%'
                | '+'
                | '='
                | '&'
                | '?'
        )
}

/// Trailing characters a word is not allowed to end on.
///
/// `.` and `:` are word characters because `foo.rs` and `Parser::feed` need them, which also means a
/// word at the end of a sentence would otherwise swallow the full stop.
const WORD_TRAILERS: [char; 2] = ['.', ':'];

/// The cell-column range of the word covering `column` in `text`, or `None` over whitespace.
pub fn word_columns(text: &str, column: usize) -> Option<Range<usize>> {
    let cells = laid_cells(text);
    let at =
        cells.iter().position(|cell| column >= cell.column && column < cell.column + cell.width)?;
    if !is_word_char(cells[at].character) {
        return None;
    }
    let start = cells[..at]
        .iter()
        .rposition(|cell| !is_word_char(cell.character))
        .map_or(0, |before| before + 1);
    let end = cells[at..]
        .iter()
        .position(|cell| !is_word_char(cell.character))
        .map_or(cells.len(), |after| at + after);
    // Never let a word end on punctuation it only contains for the sake of paths and symbols.
    let end = (start + 1..=end)
        .rev()
        .find(|last| !WORD_TRAILERS.contains(&cells[last - 1].character))
        .unwrap_or(end);
    Some(cells[start].column..cells[end - 1].column + cells[end - 1].width)
}

/// The byte range of the word covering `offset` in `text`, or `None` over whitespace.
///
/// The buffer-space twin of [`word_columns`], for the composer — which selects by byte offset
/// because it owns real text rather than a rendered grid.
pub fn word_bytes(text: &str, offset: usize) -> Option<Range<usize>> {
    let characters = text.char_indices().collect::<Vec<_>>();
    let at = characters.iter().rposition(|(start, _)| *start <= offset)?;
    if !is_word_char(characters[at].1) {
        return None;
    }
    let start = characters[..at]
        .iter()
        .rposition(|(_, character)| !is_word_char(*character))
        .map_or(0, |before| before + 1);
    let end = characters[at..]
        .iter()
        .position(|(_, character)| !is_word_char(*character))
        .map_or(characters.len(), |after| at + after);
    let end = (start + 1..=end)
        .rev()
        .find(|last| !WORD_TRAILERS.contains(&characters[last - 1].1))
        .unwrap_or(end);
    let (last_start, last) = characters[end - 1];
    Some(characters[start].0..last_start + last.len_utf8())
}

/// One rendered character with the cell column it starts at.
struct LaidCell {
    character: char,
    column: usize,
    width: usize,
}

/// Lays `text` out onto cell columns, dropping zero-width characters so a column maps to exactly
/// one character.
fn laid_cells(text: &str) -> Vec<LaidCell> {
    let mut cells = Vec::new();
    let mut column = 0;
    for character in text.chars() {
        let width = character.width().unwrap_or(0);
        if width > 0 {
            cells.push(LaidCell { character, column, width });
        }
        column += width;
    }
    cells
}

/// The substring of `text` covering cell columns `range`, clipping partially covered wide glyphs.
pub fn slice_columns(text: &str, range: Range<usize>) -> String {
    let mut out = String::new();
    let mut column = 0usize;
    for character in text.chars() {
        let width = character.width().unwrap_or(0);
        // A glyph straddling either edge is dropped rather than half-copied.
        if column + width > range.end {
            break;
        }
        if column >= range.start {
            out.push(character);
        }
        column += width;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(line: usize, column: usize) -> TextPoint {
        TextPoint::new(line, column)
    }

    #[test]
    fn a_click_selects_nothing() {
        let selection = GridSelection::collapsed(point(2, 4));
        assert!(selection.is_empty());
        assert_eq!(selection.columns_on(2, 40), None);
        assert_eq!(selection.extract(&["whatever".to_string()]), "");
    }

    #[test]
    fn a_single_line_drag_covers_the_dragged_columns_either_direction() {
        let forward = GridSelection { anchor: point(0, 2), cursor: point(0, 6) };
        let backward = GridSelection { anchor: point(0, 6), cursor: point(0, 2) };
        assert_eq!(forward.columns_on(0, 20), Some(2..6));
        assert_eq!(backward.columns_on(0, 20), Some(2..6));
        assert_eq!(forward.extract(&["abcdefghij".to_string()]), "cdef");
        assert_eq!(backward.extract(&["abcdefghij".to_string()]), "cdef");
    }

    #[test]
    fn a_multi_line_drag_runs_to_the_line_ends_in_between() {
        let selection = GridSelection { anchor: point(0, 3), cursor: point(2, 2) };
        let lines =
            vec!["first line".to_string(), "second line".to_string(), "third line".to_string()];
        assert_eq!(selection.columns_on(0, 10), Some(3..10));
        assert_eq!(selection.columns_on(1, 11), Some(0..11));
        assert_eq!(selection.columns_on(2, 10), Some(0..2));
        assert_eq!(selection.columns_on(3, 10), None);
        assert_eq!(selection.extract(&lines), "st line\nsecond line\nth");
    }

    #[test]
    fn extraction_trims_band_padding_and_respects_wide_glyphs() {
        let selection = GridSelection { anchor: point(0, 0), cursor: point(0, 20) };
        assert_eq!(selection.extract(&["padded text        ".to_string()]), "padded text");

        // 界 occupies two cells, so a column range lands on cell boundaries, not char counts.
        let wide = GridSelection { anchor: point(0, 2), cursor: point(0, 4) };
        assert_eq!(wide.extract(&["ab界cd".to_string()]), "界");
    }

    #[test]
    fn slicing_clips_a_wide_glyph_straddling_the_end() {
        assert_eq!(slice_columns("ab界cd", 0..3), "ab");
        assert_eq!(slice_columns("ab界cd", 0..4), "ab界");
        assert_eq!(cell_width("ab界cd"), 6);
    }

    /// What a reader double-clicks in a coding transcript is a symbol, a path, or a URL — all of
    /// which have to come back whole, and none of which should drag a sentence's full stop along.
    #[test]
    fn a_word_covers_a_whole_symbol_path_or_url() {
        let word = |text: &str, column: usize| {
            word_columns(text, column).map(|range| slice_columns(text, range))
        };

        let line = "call Parser::feed on src/main.rs first";
        assert_eq!(word(line, 8).as_deref(), Some("Parser::feed"));
        assert_eq!(word(line, 0).as_deref(), Some("call"));
        assert_eq!(word(line, 25).as_deref(), Some("src/main.rs"));
        // Whitespace is not a word.
        assert_eq!(word(line, 4), None);

        assert_eq!(word("see https://x.dev/a?b=1 now", 10).as_deref(), Some("https://x.dev/a?b=1"));
        // A trailing full stop or colon is not part of the word it terminates.
        assert_eq!(word("before yielding.", 10).as_deref(), Some("yielding"));
        assert_eq!(word("note: this", 1).as_deref(), Some("note"));
        assert_eq!(word("in v1.2.", 4).as_deref(), Some("v1.2"));
        // Brackets and quotes bound a word.
        assert_eq!(word("(wrapped)", 3).as_deref(), Some("wrapped"));
        // A wide glyph still maps one column range to one word.
        assert_eq!(word("a 界面 b", 2).as_deref(), Some("界面"));
    }

    /// The composer selects by byte offset, so the same rule has to hold there.
    #[test]
    fn a_word_in_buffer_space_matches_the_rendered_one() {
        let text = "call Parser::feed on src/main.rs.";
        let word = |offset: usize| word_bytes(text, offset).map(|range| text[range].to_string());
        assert_eq!(word(8).as_deref(), Some("Parser::feed"));
        assert_eq!(word(24).as_deref(), Some("src/main.rs"));
        assert_eq!(word(4), None);

        // Multi-byte characters index by byte, and a click inside one still resolves.
        let wide = "界面 ok";
        assert_eq!(
            word_bytes(wide, 1).map(|range| wide[range].to_string()).as_deref(),
            Some("界面")
        );
    }
}
