//! Diff previews — line-numbered, context-bearing summaries of `edit`/`write` changes.
//!
//! `edit` knows each change exactly (a literal `oldText → newText` at a unique location), so the
//! preview is computed directly from those replacements — no diff algorithm, no extra dependency.
//!
//! The rendering deliberately drops unified-diff hunk headers (`@@ -1,4 +1,4 @@`), which carry
//! nothing a reader of a tool card wants. Instead every row is `<sign><line> <text>` and skipped
//! regions collapse to a `...` row, so a card reads like the file with the change highlighted:
//!
//! ```text
//!      ...
//!  11  fn main() {
//! -12      println!("old");
//! +12      println!("new");
//!  13  }
//!      ...
//! ```
//!
//! `write` shows its new content the same way, as an all-additions block.
//!
//! Context is measured from what actually *differs*, never from the text the caller matched. A
//! model that matches a whole block to change one line inside it gets a preview of the change
//! rather than of the block.

use crate::output_limit::cap_lines;

/// Rows a preview is capped at before pointing to the guided review.
const DIFF_MAX_LINES: usize = 24;
/// Unchanged lines kept either side of what differs.
const CONTEXT_LINES: usize = 3;

/// One rendered preview row.
struct Row {
    /// `' '` context, `'-'` removed, `'+'` added.
    sign: char,
    /// Line number to show, or `None` for an elision row.
    number: Option<usize>,
    text: String,
}

/// Build a line-numbered preview for a set of literal replacements against `original`.
///
/// Line numbers are the *original* file's, which is what the model just read and what a follow-up
/// edit will be written against. Overlapping or adjacent changes share one context block.
pub fn edits_diff(original: &str, edits: &[(&str, &str)]) -> String {
    let located: Vec<(std::ops::Range<usize>, &str)> = edits
        .iter()
        .filter_map(|(old, new)| original.find(old).map(|at| (at..at + old.len(), *new)))
        .collect();
    ranges_diff(original, &located)
}

/// A preview of replacements already resolved to byte ranges.
///
/// The caller that knows *where* an edit landed should say so. Re-finding the search text here
/// cannot: a match that was tolerated (whitespace, indentation) is not a substring of the file at
/// all, and a `replaceAll` has several homes rather than one; re-finding would preview something
/// other than what was written.
pub fn ranges_diff(original: &str, edits: &[(std::ops::Range<usize>, &str)]) -> String {
    let lines: Vec<&str> = original.split('\n').collect();
    let mut changes: Vec<Change> =
        edits.iter().map(|(range, new)| change_at(original, &lines, range.clone(), new)).collect();
    changes.sort_by_key(|change| change.replaced.start);

    let mut rows: Vec<Row> = Vec::new();
    // The first line no block has covered yet.
    let mut emitted_through: Option<usize> = None;
    for change in &changes {
        let context_start = change.replaced.start.saturating_sub(CONTEXT_LINES);
        // Elide whatever sits between the previous block and this one.
        match emitted_through {
            None if context_start > 0 => rows.push(elision()),
            Some(previous) if context_start > previous => rows.push(elision()),
            _ => {}
        }
        let leading_from =
            emitted_through.map_or(context_start, |previous| context_start.max(previous));
        for index in leading_from..change.replaced.start {
            rows.push(context_row(&lines, index));
        }
        push_change_rows(&mut rows, change);
        let trailing_end = (change.replaced.end + CONTEXT_LINES).min(lines.len());
        for index in change.replaced.end..trailing_end {
            rows.push(context_row(&lines, index));
        }
        emitted_through = Some(trailing_end);
    }
    if emitted_through.is_some_and(|last| last < lines.len()) {
        rows.push(elision());
    }

    cap_lines(&render(&rows), DIFF_MAX_LINES, "diff lines", "`read` the file for the rest.")
}

/// A preview of everything that differs between two versions of a file, as one change.
///
/// For when the change is not one the caller made: format-on-save rewrites the file after an edit,
/// and the preview has to show the text the next edit will be matched against. Without a diff
/// algorithm it spans from the first difference to the last, so far-apart changes share a block.
pub fn texts_diff(before: &str, after: &str) -> String {
    let prefix = before
        .char_indices()
        .zip(after.chars())
        .find(|((_, a), b)| a != b)
        .map_or(before.len().min(after.len()), |((at, _), _)| at);
    let suffix = before[prefix..]
        .chars()
        .rev()
        .zip(after[prefix..].chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a.len_utf8())
        .sum::<usize>();
    let replaced = prefix..before.len() - suffix;
    ranges_diff(before, &[(replaced, &after[prefix..after.len() - suffix])])
}

/// Render `content` as an all-additions block — a `write` creates every line.
pub fn added_lines(content: &str) -> String {
    let rows = content
        .split('\n')
        .enumerate()
        .map(|(index, line)| Row { sign: '+', number: Some(index + 1), text: line.to_string() })
        .collect::<Vec<_>>();
    // Nothing to follow here, so the hint says so rather than sending the caller to re-read a file
    // it just authored: this is the preview of a `write` that already succeeded in full.
    cap_lines(
        &render(&rows),
        DIFF_MAX_LINES,
        "lines",
        "The file has all of it; the preview is trimmed.",
    )
}

/// One located replacement, reduced to the original-file lines it actually changes.
///
/// The span is what *differs*, not what the caller matched. A model routinely matches a whole block
/// to change one line inside it, and measuring from the match spends the preview on lines that
/// never moved — ahead of the three this module promises either side, so on a big enough match the
/// change itself falls off the end of whatever is reading it.
struct Change {
    /// Original-file lines being replaced, 0-based and half-open. Empty for a pure insertion.
    replaced: std::ops::Range<usize>,
    removed: Vec<String>,
    added: Vec<String>,
}

/// Expand a byte-position replacement to the whole lines it touches, so a mid-line edit shows the
/// full before/after line rather than a dangling fragment.
fn change_at(original: &str, lines: &[&str], range: std::ops::Range<usize>, new: &str) -> Change {
    let position = range.start;
    let start_line = original[..position].matches('\n').count();
    let end_line = start_line + original[range.clone()].matches('\n').count();
    let line_start = original[..position].rfind('\n').map_or(0, |index| index + 1);
    let prefix = &original[line_start..position];
    let suffix_start = range.end;
    let suffix_end =
        original[suffix_start..].find('\n').map_or(original.len(), |index| suffix_start + index);
    let suffix = &original[suffix_start..suffix_end];

    let removed: Vec<String> = lines[start_line..=end_line.min(lines.len().saturating_sub(1))]
        .iter()
        .map(|line| (*line).to_string())
        .collect();
    let added: Vec<String> =
        format!("{prefix}{new}{suffix}").split('\n').map(str::to_string).collect();
    trim_unchanged(start_line, removed, added)
}

/// Reduces a matched replacement to the lines that differ, dropping the identical ends.
///
/// Appending one line to a three-line file by matching the whole file described three removals and
/// four additions; trimming turns that back into one addition, which is both what happened and a
/// fraction of the tokens, since the model reads this result back. The trimmed ends are not context
/// rows either — they are ordinary file lines, and [`ranges_diff`] measures its context block from
/// the span this leaves behind.
fn trim_unchanged(start_line: usize, removed: Vec<String>, added: Vec<String>) -> Change {
    let lead = removed.iter().zip(&added).take_while(|(before, after)| before == after).count();
    let tail = removed[lead..]
        .iter()
        .rev()
        .zip(added[lead..].iter().rev())
        .take_while(|(before, after)| before == after)
        .count();
    Change {
        replaced: start_line + lead..start_line + removed.len() - tail,
        removed: removed[lead..removed.len() - tail].to_vec(),
        added: added[lead..added.len() - tail].to_vec(),
    }
}

/// Emits the lines that differ, numbered against the original file.
fn push_change_rows(rows: &mut Vec<Row>, change: &Change) {
    let first = change.replaced.start + 1;
    for (offset, text) in change.removed.iter().enumerate() {
        rows.push(Row { sign: '-', number: Some(first + offset), text: text.clone() });
    }
    for (offset, text) in change.added.iter().enumerate() {
        rows.push(Row { sign: '+', number: Some(first + offset), text: text.clone() });
    }
}

fn context_row(lines: &[&str], index: usize) -> Row {
    Row {
        sign: ' ',
        number: Some(index + 1),
        text: lines.get(index).copied().unwrap_or_default().to_string(),
    }
}

fn elision() -> Row {
    Row { sign: ' ', number: None, text: "...".to_string() }
}

/// Lay the rows out with a right-aligned line-number gutter.
fn render(rows: &[Row]) -> String {
    let width = rows
        .iter()
        .filter_map(|row| row.number)
        .map(|number| number.to_string().len())
        .max()
        .unwrap_or(1);
    rows.iter()
        .map(|row| match row.number {
            Some(number) => format!("{}{number:>width$} {}", row.sign, row.text, width = width),
            None => format!("{}{:>width$} {}", row.sign, "", row.text, width = width),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests;
