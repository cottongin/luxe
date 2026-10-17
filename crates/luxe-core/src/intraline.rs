//! Intra-line diff — which words inside a replaced line actually changed.
//!
//! A line-level diff says a line was replaced; it does not say *what* in it was replaced, so the
//! reader re-reads both lines to find the one token that moved. This module answers that: given a
//! removed line and the addition that stands in for it, it returns the byte ranges on each side that
//! differ, so a renderer can mark exactly those.
//!
//! # Design
//!
//! Tokens, not characters. Comparing `String` to `Strong` character-wise marks a lone `i`/`o`, which
//! is both hard to see and useless — the reader wants the *word*. So both sides are tokenized into
//! identifiers, whitespace runs and single symbols, matched with an LCS over those tokens, and the
//! unmatched runs come back as byte ranges.
//!
//! Three rules keep the result worth rendering rather than merely correct:
//!
//! - **Whitespace is never the change.** A run's leading/trailing whitespace is trimmed off and a
//!   whitespace-only run is dropped, so re-indentation never lights up a line's left margin.
//! - **No speckling.** Runs separated by a character or two of unchanged text merge, so `'2'` → `2`
//!   marks one span instead of striping the quotes and leaving the digit bare.
//! - **A rewrite is not an edit.** When too little of the line survives, marking three quarters of it
//!   says nothing the line's own color already said, so the emphasis is dropped entirely.

use std::ops::Range;

/// The changed spans of one replaced line pair.
///
/// Each side's ranges are ascending, non-overlapping, within that side's bounds, and land on
/// character boundaries — a renderer can slice with them directly.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IntralineChange {
    /// Byte ranges into the removed line that the addition does not carry over.
    pub removed: Vec<Range<usize>>,
    /// Byte ranges into the added line that the removal did not have.
    pub added: Vec<Range<usize>>,
}

impl IntralineChange {
    /// Whether there is nothing to emphasize — identical lines, or a pair the rules rejected.
    pub fn is_empty(&self) -> bool {
        self.removed.is_empty() && self.added.is_empty()
    }
}

/// Tokens per side above which the LCS is skipped for one contiguous span instead.
///
/// The table is quadratic and this runs per visible row per frame, so a minified bundle or a
/// base64 blob on one line must not be able to stall a paint. Past the cap the answer degrades to
/// "the middle differs", which is what a whole-line diff would have said anyway. Real code lines sit
/// far below it — this is a fuse, not a budget.
const MAX_LCS_TOKENS: usize = 128;

/// Unchanged characters two changed runs may span and still merge into one.
///
/// Enough to swallow a quote, a bracket, a space or a sigil sitting between two edits, not enough to
/// swallow a word. `'2'` → `2` changes both quotes but not the digit, and `label: String` → `label`
/// drops a colon and a type either side of a space that stayed; marking those fragments separately is
/// technically exact and reads as noise.
const MAX_SPECKLE_GAP: usize = 2;

/// Fraction of a side's visible content that must survive for emphasis to be worth showing.
///
/// Below this the two lines are not the same line with an edit in it, they are a rewrite, and marking
/// nearly all of both adds a second signal that says exactly what their add/remove color already
/// says. Dropping out here is what keeps a full-width wash off a line that was simply replaced.
const MIN_SURVIVING_FRACTION: f32 = 0.25;

/// Computes the word-level change between a removed line and the addition replacing it.
///
/// Returns empty ranges when the lines are identical, when only whitespace differs, or when too
/// little of either line survives for the marks to mean anything.
pub fn intraline_change(removed: &str, added: &str) -> IntralineChange {
    if removed == added {
        return IntralineChange::default();
    }
    let (removed_tokens, added_tokens) = (tokenize(removed), tokenize(added));

    // Trim the shared head and tail first: it is the whole answer for most edits, and it shrinks what
    // the quadratic step below ever sees.
    let head = common_prefix(removed, &removed_tokens, added, &added_tokens);
    let tail = common_suffix(removed, &removed_tokens[head..], added, &added_tokens[head..]);
    let removed_middle = &removed_tokens[head..removed_tokens.len() - tail];
    let added_middle = &added_tokens[head..added_tokens.len() - tail];

    let (removed_changed, added_changed) =
        if removed_middle.len() > MAX_LCS_TOKENS || added_middle.len() > MAX_LCS_TOKENS {
            (all_changed(removed_middle), all_changed(added_middle))
        } else {
            changed_tokens(removed, removed_middle, added, added_middle)
        };

    let change = IntralineChange {
        removed: spans(removed, removed_middle, &removed_changed),
        added: spans(added, added_middle, &added_changed),
    };
    if survives(removed, &change.removed) && survives(added, &change.added) {
        change
    } else {
        IntralineChange::default()
    }
}

/// One token, as a byte range into the line it came from.
///
/// A range and nothing else: what class a token belongs to matters while splitting the line and never
/// again — whitespace is recognized by trimming, not by a tag carried around.
type Token = Range<usize>;

/// Splits a line into identifier runs, whitespace runs, and single symbols.
///
/// Symbols stay one character each so `.`, `(` and `,` can match independently — grouping them would
/// make `foo(a)` and `foo(b, c)` share nothing past the name.
fn tokenize(line: &str) -> Vec<Token> {
    /// What a character belongs to, and so where a token ends.
    #[derive(PartialEq, Eq)]
    enum Class {
        /// Letters, digits, underscores — an identifier-ish run.
        Word,
        /// A run of whitespace.
        Space,
        /// Punctuation or a symbol, which never groups.
        Symbol,
    }
    let classify = |character: char| {
        if character.is_whitespace() {
            Class::Space
        } else if character.is_alphanumeric() || character == '_' {
            Class::Word
        } else {
            Class::Symbol
        }
    };

    let mut tokens = Vec::new();
    let mut chars = line.char_indices().peekable();
    while let Some((start, character)) = chars.next() {
        let class = classify(character);
        let mut end = start + character.len_utf8();
        if class != Class::Symbol {
            while let Some(&(next_start, next)) = chars.peek() {
                if classify(next) != class {
                    break;
                }
                end = next_start + next.len_utf8();
                chars.next();
            }
        }
        tokens.push(start..end);
    }
    tokens
}

fn text<'a>(line: &'a str, token: &Token) -> &'a str {
    &line[token.clone()]
}

/// Tokens the two sides share from the start.
fn common_prefix(removed: &str, left: &[Token], added: &str, right: &[Token]) -> usize {
    left.iter().zip(right).take_while(|(a, b)| text(removed, a) == text(added, b)).count()
}

/// Tokens the two sides share from the end, given a head that has already been trimmed.
fn common_suffix(removed: &str, left: &[Token], added: &str, right: &[Token]) -> usize {
    left.iter()
        .rev()
        .zip(right.iter().rev())
        .take_while(|(a, b)| text(removed, a) == text(added, b))
        .count()
}

/// Every token, as changed — the answer when the LCS is skipped.
fn all_changed(tokens: &[Token]) -> Vec<bool> {
    vec![true; tokens.len()]
}

/// Marks the tokens on each side that the longest common subsequence does not match.
///
/// Plain LCS rather than Myers: the input is one screen line's worth of tokens after the shared head
/// and tail are gone, so the table is small and a backtrack is easier to be sure of.
fn changed_tokens(
    removed: &str,
    left: &[Token],
    added: &str,
    right: &[Token],
) -> (Vec<bool>, Vec<bool>) {
    let (rows, columns) = (left.len(), right.len());
    let mut table = vec![0u16; (rows + 1) * (columns + 1)];
    let at = |row: usize, column: usize| row * (columns + 1) + column;
    for row in (0..rows).rev() {
        for column in (0..columns).rev() {
            table[at(row, column)] = if text(removed, &left[row]) == text(added, &right[column]) {
                table[at(row + 1, column + 1)] + 1
            } else {
                table[at(row + 1, column)].max(table[at(row, column + 1)])
            };
        }
    }

    let mut left_changed = vec![true; rows];
    let mut right_changed = vec![true; columns];
    let (mut row, mut column) = (0usize, 0usize);
    while row < rows && column < columns {
        if text(removed, &left[row]) == text(added, &right[column]) {
            left_changed[row] = false;
            right_changed[column] = false;
            row += 1;
            column += 1;
        } else if table[at(row + 1, column)] >= table[at(row, column + 1)] {
            row += 1;
        } else {
            column += 1;
        }
    }
    (left_changed, right_changed)
}

/// Turns per-token change flags into renderable byte ranges.
///
/// Consecutive changed tokens coalesce, then the three presentation rules apply: whitespace is
/// trimmed off the ends of a run, whitespace-only runs are dropped, and runs a character or two apart
/// merge so the result reads as spans rather than speckle.
fn spans(line: &str, tokens: &[Token], changed: &[bool]) -> Vec<Range<usize>> {
    let mut runs: Vec<Range<usize>> = Vec::new();
    for (token, _) in tokens.iter().zip(changed).filter(|(_, changed)| **changed) {
        match runs.last_mut() {
            Some(last) if last.end == token.start => last.end = token.end,
            _ => runs.push(token.clone()),
        }
    }
    let mut trimmed: Vec<Range<usize>> = Vec::with_capacity(runs.len());
    for run in runs {
        let span = trim_whitespace(line, run);
        if span.is_empty() {
            continue;
        }
        match trimmed.last_mut() {
            Some(last) if span.start - last.end <= MAX_SPECKLE_GAP => last.end = span.end,
            _ => trimmed.push(span),
        }
    }
    trimmed
}

/// Narrows a range to its non-whitespace content, which may empty it.
fn trim_whitespace(line: &str, range: Range<usize>) -> Range<usize> {
    let span = &line[range.clone()];
    let start = range.start + (span.len() - span.trim_start().len());
    let end = range.end - (span.len() - span.trim_end().len());
    start..end.max(start)
}

/// Whether enough of `line` is left unmarked for the marks to carry information.
///
/// Measured in non-whitespace characters, so indentation neither props a line up nor drags it down. A
/// line with no visible content at all (an added blank line) has nothing to emphasize and passes
/// trivially.
fn survives(line: &str, changed: &[Range<usize>]) -> bool {
    if changed.is_empty() {
        return true;
    }
    let visible = |text: &str| text.chars().filter(|character| !character.is_whitespace()).count();
    let total = visible(line);
    if total == 0 {
        return true;
    }
    let marked: usize = changed.iter().map(|range| visible(&line[range.clone()])).sum();
    let surviving = total.saturating_sub(marked) as f32 / total as f32;
    surviving >= MIN_SURVIVING_FRACTION
}

#[cfg(test)]
mod tests;
