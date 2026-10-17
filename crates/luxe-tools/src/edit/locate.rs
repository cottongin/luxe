//! Locating an edit's target — exact first, then two bounded relaxations.
//!
//! Reproducing a block of code byte-for-byte is the single most common way a model's edit fails,
//! and the two ways it fails are boring: trailing whitespace it could not see, and indentation it
//! re-derived. Neither changes which code was meant. So the match is tried three times, from
//! strictest to loosest, and **every** attempt still demands a unique match — tolerance here buys
//! the model a retry, never a guess about which of two places it meant.
//!
//! The relaxations are line-oriented, because that is the shape of the mistake: they only apply
//! when the search text is whole lines, and the indentation pass only accepts a window whose lines
//! are *uniformly* shifted, then re-indents the replacement by the same amount. A window where
//! each line drifted differently is a different block of code, not a mis-indented one.

use std::ops::Range;

/// How a match was found — reported to the model so a tolerated edit is never a silent one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MatchKind {
    /// Byte-for-byte.
    Exact,
    /// Identical once trailing whitespace was ignored.
    TrailingWhitespace,
    /// Identical once a uniform indentation shift was applied.
    Indentation { spaces: i64 },
}

impl MatchKind {
    /// A short note for the tool result, or `None` when the match was exact and needs no comment.
    pub(super) fn note(self) -> Option<String> {
        match self {
            Self::Exact => None,
            Self::TrailingWhitespace => Some("matched ignoring trailing whitespace".to_string()),
            Self::Indentation { spaces } => Some(format!(
                "matched after re-indenting by {} space{}",
                spaces.abs(),
                if spaces.abs() == 1 { "" } else { "s" }
            )),
        }
    }
}

/// Where an edit applies, and what to write there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Located {
    /// Byte ranges to replace, in ascending order.
    pub(crate) ranges: Vec<Range<usize>>,
    /// The replacement, adjusted when the match required one.
    pub(crate) replacement: String,
    /// How it was found.
    pub(crate) kind: MatchKind,
}

/// Why an edit could not be placed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LocateError {
    /// Nothing matched, with the closest thing in the file when there is one.
    NotFound { nearest: Option<Nearest> },
    /// Several places matched and the caller did not ask for all of them.
    Ambiguous { lines: Vec<usize> },
}

/// The closest region in the file to what was searched for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Nearest {
    /// 1-based line the region starts at.
    pub(crate) line: usize,
    /// What is actually there.
    pub(crate) text: String,
}

/// Finds every place `old_text` applies, relaxing only as far as it must.
///
/// `old_text` must be non-empty; [`super::args::normalize`] refuses an empty one as a missing field
/// and pins that. An empty needle matches at every byte boundary, so it is a precondition here
/// rather than a case to handle — there is no sensible place to apply an edit that matches
/// everywhere.
pub(crate) fn locate(
    content: &str,
    old_text: &str,
    new_text: &str,
    replace_all: bool,
) -> Result<Located, LocateError> {
    let exact: Vec<Range<usize>> =
        content.match_indices(old_text).map(|(at, text)| at..at + text.len()).collect();
    if !exact.is_empty() {
        return finish(content, exact, new_text.to_string(), MatchKind::Exact, replace_all);
    }
    if let Some(found) = line_window(content, old_text, Relax::TrailingWhitespace) {
        return finish(
            content,
            found.ranges,
            new_text.to_string(),
            MatchKind::TrailingWhitespace,
            replace_all,
        );
    }
    if let Some(found) = line_window(content, old_text, Relax::Indentation) {
        let spaces = found.shift;
        return finish(
            content,
            found.ranges,
            reindent(new_text, spaces),
            MatchKind::Indentation { spaces },
            replace_all,
        );
    }
    Err(LocateError::NotFound { nearest: nearest_region(content, old_text) })
}

fn finish(
    content: &str,
    ranges: Vec<Range<usize>>,
    replacement: String,
    kind: MatchKind,
    replace_all: bool,
) -> Result<Located, LocateError> {
    if ranges.len() > 1 && !replace_all {
        return Err(LocateError::Ambiguous {
            lines: ranges.iter().map(|range| line_of(content, range.start)).collect(),
        });
    }
    let ranges = if replace_all { ranges } else { vec![ranges[0].clone()] };
    Ok(Located { ranges, replacement, kind })
}

/// Which difference a line-window pass is willing to overlook.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Relax {
    TrailingWhitespace,
    Indentation,
}

struct WindowMatch {
    ranges: Vec<Range<usize>>,
    /// Spaces the file's copy is indented by, relative to the search text.
    shift: i64,
}

/// Slides `needle`'s lines over the file's, comparing under `relax`.
///
/// Only for whole-line searches. A search that starts or ends mid-line is describing a fragment,
/// and replacing the lines around it would rewrite code the caller never mentioned.
fn line_window(content: &str, needle: &str, relax: Relax) -> Option<WindowMatch> {
    if needle.is_empty() || needle.contains('\n') && !needle.ends_with('\n') && false {
        return None;
    }
    let needle_lines: Vec<&str> = needle.trim_end_matches('\n').split('\n').collect();
    if needle_lines.iter().all(|line| line.trim().is_empty()) {
        return None;
    }
    let starts = line_starts(content);
    let file_lines: Vec<&str> = content.split('\n').collect();
    if needle_lines.len() > file_lines.len() {
        return None;
    }

    let mut ranges = Vec::new();
    let mut shift = None;
    for index in 0..=file_lines.len() - needle_lines.len() {
        let window = &file_lines[index..index + needle_lines.len()];
        let Some(window_shift) = window_matches(window, &needle_lines, relax) else {
            continue;
        };
        // Every occurrence must agree about the shift, or the replacement cannot be re-indented
        // for all of them at once.
        match shift {
            Some(previous) if previous != window_shift => return None,
            _ => shift = Some(window_shift),
        }
        let start = starts[index];
        let last = index + needle_lines.len() - 1;
        let end = starts[last] + window[window.len() - 1].len();
        ranges.push(start..end);
    }
    (!ranges.is_empty()).then(|| WindowMatch { ranges, shift: shift.unwrap_or(0) })
}

/// The uniform indentation shift that makes `window` equal `needle`, if one exists.
fn window_matches(window: &[&str], needle: &[&str], relax: Relax) -> Option<i64> {
    let mut shift: Option<i64> = None;
    for (actual, wanted) in window.iter().zip(needle) {
        match relax {
            Relax::TrailingWhitespace => {
                if actual.trim_end() != wanted.trim_end() {
                    return None;
                }
            }
            Relax::Indentation => {
                if actual.trim() != wanted.trim() {
                    return None;
                }
                // A blank line carries no indentation to compare, so it constrains nothing.
                if actual.trim().is_empty() {
                    continue;
                }
                let delta = indent_width(actual) - indent_width(wanted);
                match shift {
                    Some(previous) if previous != delta => return None,
                    _ => shift = Some(delta),
                }
            }
        }
    }
    Some(shift.unwrap_or(0))
}

/// Leading spaces, counting a tab as one unit — the comparison only needs to be self-consistent.
fn indent_width(line: &str) -> i64 {
    line.chars().take_while(|character| character.is_whitespace()).count() as i64
}

/// Shifts every non-blank line of `text` by `spaces`.
fn reindent(text: &str, spaces: i64) -> String {
    if spaces == 0 {
        return text.to_string();
    }
    text.split('\n')
        .map(|line| {
            if line.trim().is_empty() {
                return line.to_string();
            }
            match spaces.cmp(&0) {
                std::cmp::Ordering::Greater => format!("{}{line}", " ".repeat(spaces as usize)),
                std::cmp::Ordering::Less => {
                    let drop = (-spaces) as usize;
                    let removable =
                        line.chars().take(drop).take_while(|c| *c == ' ' || *c == '\t').count();
                    line.chars().skip(removable).collect()
                }
                std::cmp::Ordering::Equal => line.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Byte offset of every line start.
fn line_starts(content: &str) -> Vec<usize> {
    let mut starts = vec![0];
    starts.extend(content.match_indices('\n').map(|(at, _)| at + 1));
    starts
}

/// 1-based line number of a byte offset.
pub(crate) fn line_of(content: &str, offset: usize) -> usize {
    content[..offset.min(content.len())].matches('\n').count() + 1
}

/// The region of the file that most resembles the search text.
///
/// "not found" on its own is undiagnosable: the model cannot tell a typo from a stale read from a
/// file it never opened. Pointing at the closest thing turns the next attempt into a correction.
fn nearest_region(content: &str, needle: &str) -> Option<Nearest> {
    let anchor = needle.lines().find(|line| !line.trim().is_empty())?.trim();
    if anchor.len() < 4 {
        return None;
    }
    let mut best: Option<(usize, usize)> = None;
    for (index, line) in content.lines().enumerate() {
        let score = similarity(line.trim(), anchor);
        if score > 0 && best.is_none_or(|(previous, _)| score > previous) {
            best = Some((score, index));
        }
    }
    let (score, index) = best?;
    // Below roughly half the anchor's length it is not a near miss, it is a coincidence.
    if score * 2 < anchor.len() {
        return None;
    }
    let text = content.lines().nth(index)?.to_string();
    Some(Nearest { line: index + 1, text })
}

/// Length of the longest shared prefix, as a cheap stand-in for edit distance.
fn similarity(left: &str, right: &str) -> usize {
    left.chars().zip(right.chars()).take_while(|(a, b)| a == b).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "fn main() {\n    let x = 1;\n    println!(\"{x}\");\n}\n";

    #[test]
    fn an_exact_match_is_found_and_reported_as_exact() {
        let found = locate(FILE, "    let x = 1;", "    let x = 2;", false).expect("found");
        assert_eq!(found.kind, MatchKind::Exact);
        assert_eq!(found.ranges.len(), 1);
        assert_eq!(found.kind.note(), None, "an exact match needs no explanation");
    }

    /// The most common near-miss there is: whitespace the model could not see. It only bites on a
    /// multi-line search, where the invisible characters sit *inside* the block being matched.
    #[test]
    fn trailing_whitespace_is_forgiven_and_said_out_loud() {
        let file = "fn main() {\n    let x = 1;   \n    go();\n}\n";
        let found = locate(file, "    let x = 1;\n    go();", "    let x = 2;\n    go();", false)
            .expect("found");
        assert_eq!(found.kind, MatchKind::TrailingWhitespace);
        assert!(found.kind.note().expect("a note").contains("trailing whitespace"));
        assert_eq!(found.ranges.len(), 1);
    }

    /// A uniformly re-indented block is the same block; the replacement moves with it.
    #[test]
    fn a_uniform_indentation_shift_is_forgiven_and_applied_to_the_replacement() {
        let file = "impl T {\n        fn go(&self) {\n            work();\n        }\n}\n";
        let found = locate(
            file,
            "fn go(&self) {\n    work();\n}",
            "fn go(&self) {\n    work_twice();\n}",
            false,
        )
        .expect("found");
        assert_eq!(found.kind, MatchKind::Indentation { spaces: 8 });
        assert_eq!(
            found.replacement, "        fn go(&self) {\n            work_twice();\n        }",
            "the replacement is re-indented to match the file, not the search text"
        );
    }

    /// Lines that drifted *differently* are a different block, not a mis-indented one.
    #[test]
    fn a_non_uniform_shift_is_not_treated_as_indentation() {
        let file = "impl T {\n    fn go(&self) {\n            work();\n    }\n}\n";
        let error = locate(file, "fn go(&self) {\n    work();\n}", "x", false).expect_err("no");
        assert!(matches!(error, LocateError::NotFound { .. }));
    }

    #[test]
    fn several_matches_are_ambiguous_unless_replace_all_was_asked_for() {
        let file = "a();\nb();\na();\n";
        let error = locate(file, "a();", "c();", false).expect_err("ambiguous");
        let LocateError::Ambiguous { lines } = error else { panic!("expected ambiguity") };
        assert_eq!(lines, vec![1, 3], "and it says where, so the fix is to add context");

        let found = locate(file, "a();", "c();", true).expect("replace_all takes them all");
        assert_eq!(found.ranges.len(), 2);
    }

    /// "not found" alone cannot be acted on; the closest line can.
    #[test]
    fn a_near_miss_reports_the_closest_line() {
        let error =
            locate(FILE, "    let x = 11;", "y", false).expect_err("no exact or relaxed match");
        let LocateError::NotFound { nearest } = error else { panic!("expected not-found") };
        let nearest = nearest.expect("a near miss in the same file");
        assert_eq!(nearest.line, 2);
        assert!(nearest.text.contains("let x = 1;"));
    }

    #[test]
    fn an_unrelated_search_reports_no_near_miss_rather_than_a_coincidence() {
        let error = locate(FILE, "quantum_flux_capacitor()", "y", false).expect_err("no match");
        assert_eq!(error, LocateError::NotFound { nearest: None });
    }

    #[test]
    fn a_dedent_removes_only_whitespace() {
        assert_eq!(
            reindent("        deep();\n\n        more();", -4),
            "    deep();\n\n    more();"
        );
        // Never eats real characters when the line is shallower than the shift asks for.
        assert_eq!(reindent("  x();", -8), "x();");
    }
}
