//! Card rows — the structure behind a diff preview string.
//!
//! `edit`/`write` results arrive as text: `<sign><line> <code>` per row, `...` where the file was
//! skipped, and a leading summary sentence. Parsing that back into rows is what lets a card do
//! anything but print it — pair a removal with its replacement, syntax-highlight the code without
//! painting the gutter, lay two columns out, count what it had to leave off.

/// One parsed preview row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Row {
    /// A line-numbered source row.
    Source(SourceRow),
    /// `...` — a stretch of the file the preview skipped.
    Elision,
    /// Anything that isn't a diff row: the summary sentence, an injected diagnostics banner.
    Note(String),
}

/// A line-numbered source row and what happened to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SourceRow {
    pub(crate) kind: RowKind,
    /// Line number as the preview reported it, absent on a row it could not attribute.
    pub(crate) number: Option<usize>,
    pub(crate) code: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RowKind {
    Context,
    Added,
    Removed,
}

impl RowKind {
    /// The sign a unified row wears.
    pub(super) fn marker(self) -> char {
        match self {
            Self::Context => ' ',
            Self::Added => '+',
            Self::Removed => '-',
        }
    }
}

/// Parses a diff preview into rows.
///
/// Two passes, because a row cannot be split on its own: the writer right-aligns every line number
/// into one field width, so `-4     code` is a two-wide field and five spaces of *indentation*,
/// while `-4  code` is the same number in a four-wide field. Guessing per row — consuming spaces and
/// digits until the code starts — silently eats the code's indentation, which in a diff is half of
/// what the reader is looking at. The whole preview agrees on where the separator sits, so find it
/// once and cut every row there.
///
/// The narrowest answer wins. Every *numbered* row reports the same separator, because the writer
/// right-aligns one field across the whole preview; a row with an empty field reports whatever its
/// **code** looks like, and `42 answer` reads as a number followed by a separator. That answer is
/// always the deeper one, so taking the widest would cut every row there — inventing a line number
/// for the unnumbered row and eating the first token of its neighbor's code.
///
/// A `+++`/`---` banner is a note, not a one-character-wide addition: an injected diagnostics block
/// carries both and neither is a change to the file.
pub(super) fn parse(preview: &str) -> Vec<Row> {
    // Tabs survive this pass: the gutter is found by byte offset across every row at once, and
    // expanding one row's indentation would move the separator out from under the others. The code
    // is expanded below, once it has been cut away from the gutter — which is also where an editor
    // measures a tab stop from, so the indentation lands where the file put it.
    let lines: Vec<String> = preview.lines().map(strip_controls).collect();
    let gutter = lines.iter().filter_map(|line| separator_index(line)).min().map(|at| at + 1);
    lines.iter().map(|line| parse_row(line, gutter)).collect()
}

fn parse_row(line: &str, gutter: Option<usize>) -> Row {
    let note = |text: &str| Row::Note(crate::tabstop::display_text(text).into_owned());
    if line.starts_with("+++") || line.starts_with("---") {
        return note(line);
    }
    let Some((gutter, code)) = split_gutter(line, gutter) else {
        return note(line);
    };
    let kind = match gutter.chars().next() {
        Some('+') => RowKind::Added,
        Some('-') => RowKind::Removed,
        _ => RowKind::Context,
    };
    let number = gutter[1..].trim().parse::<usize>().ok();
    // An unnumbered context row whose only content is `...` is the elision marker.
    if number.is_none() && kind == RowKind::Context && code.trim() == "..." {
        return Row::Elision;
    }
    Row::Source(SourceRow { kind, number, code: crate::tabstop::display_text(code).into_owned() })
}

/// Splits a row at the preview's shared gutter width, into `<sign><number> ` and the code.
///
/// Returns `None` for a line that is not a diff row at all, so a summary sentence keeps its first
/// character instead of having it read as a sign. A row that does not fit the shared width falls
/// back to its own separator, so one malformed line cannot mangle the rest.
fn split_gutter(row: &str, shared: Option<usize>) -> Option<(&str, &str)> {
    if !row.starts_with(['+', '-', ' ']) {
        return None;
    }
    let width = shared
        .filter(|width| is_gutter(row.get(..*width)))
        .or_else(|| separator_index(row).map(|at| at + 1))?;
    let (gutter, code) = row.split_at(width);
    (!gutter.trim().is_empty() || !code.is_empty()).then_some((gutter, code))
}

/// Whether a slice is a well-formed gutter: a sign, a right-aligned number field, one space.
fn is_gutter(slice: Option<&str>) -> bool {
    let Some(slice) = slice.filter(|slice| slice.len() >= 2) else {
        return false;
    };
    slice.ends_with(' ')
        && slice[1..].chars().all(|character| character.is_ascii_digit() || character == ' ')
}

/// Index of the one space separating the number field from the code, for a row that has a number.
///
/// The field is right-aligned, so it reads as optional padding then digits then the separator. A row
/// with no digits at all (an elision, a note) has no separator of its own to find.
fn separator_index(row: &str) -> Option<usize> {
    let mut chars = row.char_indices();
    let (_, sign) = chars.next()?;
    if !matches!(sign, '+' | '-' | ' ') {
        return None;
    }
    let mut saw_digit = false;
    for (index, character) in chars {
        match character {
            '0'..='9' => saw_digit = true,
            ' ' if saw_digit => return Some(index),
            // Padding ahead of the right-aligned number.
            ' ' => continue,
            _ => return None,
        }
    }
    None
}

/// Text a card can measure and draw: no cursor-moving control characters, and no tabs left to
/// disagree about.
///
/// A card lays its rows out against a width and the transcript then wraps them against the same
/// one, so the two have to agree on what a line is worth. A tab is the one character they do not:
/// see [`crate::tabstop`]. Spending it here means every measurement downstream — a wrap column, a
/// padded wash, the rule between two diff columns — is the width that actually lands on screen.
pub(super) fn sanitize(value: &str) -> String {
    crate::tabstop::display_text(&strip_controls(value)).into_owned()
}

/// Drops control characters a tool's output should never move the cursor with, keeping tabs.
fn strip_controls(value: &str) -> String {
    value.chars().filter(|character| !character.is_control() || *character == '\t').collect()
}

/// The widest line number any row carries, as a gutter width.
pub(super) fn number_width(rows: &[Row]) -> usize {
    rows.iter()
        .filter_map(|row| match row {
            Row::Source(source) => source.number,
            _ => None,
        })
        .map(|number| number.to_string().len())
        .max()
        .unwrap_or(1)
}

/// A replacement: a run of removals and the additions that stand in for them.
///
/// Removals pair positionally with additions, which is the same assumption the preview writer made
/// when it emitted them in that order. A run with more of one than the other leaves the surplus
/// unpaired — an outright deletion or insertion, with nothing on the other side to compare against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Replacement {
    pub(crate) removed: std::ops::Range<usize>,
    pub(crate) added: std::ops::Range<usize>,
}

impl Replacement {
    /// Whether this changed nothing — no removals and no additions.
    pub(super) fn is_empty(&self) -> bool {
        self.removed.is_empty() && self.added.is_empty()
    }
}

/// Finds every replacement in `rows`, as index ranges into it.
pub(super) fn replacements(rows: &[Row]) -> Vec<Replacement> {
    let kind = |index: usize| match rows.get(index) {
        Some(Row::Source(source)) => Some(source.kind),
        _ => None,
    };
    let mut found = Vec::new();
    let mut index = 0;
    while index < rows.len() {
        let removed_start = index;
        while kind(index) == Some(RowKind::Removed) {
            index += 1;
        }
        let removed_end = index;
        while kind(index) == Some(RowKind::Added) {
            index += 1;
        }
        if removed_end > removed_start || index > removed_end {
            found.push(Replacement {
                removed: removed_start..removed_end,
                added: removed_end..index,
            });
        }
        // Step past a row that started no run (context, note, elision) or we would never advance.
        if index == removed_start {
            index += 1;
        }
    }
    found
}

/// Byte ranges of the intra-line change for each paired row, relative to that row's `code`.
///
/// Ranges are computed over the code rather than the whole row because the signs differ: comparing
/// `-12 x` with `+12 y` finds no common prefix at all and emphasizes the entire line.
pub(super) fn intraline_emphasis(rows: &[Row]) -> IntralineEmphasis {
    let mut emphasis = IntralineEmphasis::new();
    let code = |index: usize| match rows.get(index) {
        Some(Row::Source(source)) => Some(source.code.as_str()),
        _ => None,
    };
    for replacement in replacements(rows) {
        for (removed_index, added_index) in
            replacement.removed.clone().zip(replacement.added.clone())
        {
            let (Some(removed), Some(added)) = (code(removed_index), code(added_index)) else {
                continue;
            };
            let change = luxe_core::intraline::intraline_change(removed, added);
            if !change.is_empty() {
                emphasis.insert(removed_index, change.removed);
                emphasis.insert(added_index, change.added);
            }
        }
    }
    emphasis
}

/// The changed spans of every row a replacement paired up, keyed by row index.
pub(super) type IntralineEmphasis = std::collections::HashMap<usize, Vec<std::ops::Range<usize>>>;

/// The language token a card highlights with, from the path on its header.
///
/// Syntect resolves a bare extension, so the extension *is* the token; a path with none (a
/// `Makefile`, a `Dockerfile`) falls back to its file name, which syntect matches by name.
pub(super) fn language_for(path: &str) -> Option<String> {
    let path = std::path::Path::new(path.trim());
    path.extension()
        .or_else(|| path.file_name())
        .map(|token| token.to_string_lossy().to_lowercase())
        .filter(|token| !token.is_empty())
}

#[cfg(test)]
mod tests;
