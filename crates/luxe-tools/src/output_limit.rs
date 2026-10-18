//! Output bounding — cap line-oriented tool results before they reach the model.
//!
//! `bash` already runs a full compression pipeline (`luxe_core::compression`), but the search/
//! listing tools (`grep`, `glob`, `git`, `read`) must not stream their entire output straight into
//! the model's context: a broad `grep` or a large `git diff` would bury the conversation and burn
//! tokens. This keeps the first `max_lines` of signal, states exactly how much was dropped,
//! and hard-clamps total bytes so a single pathological line can't blow the budget either.

/// Last-resort byte clamp applied after the line cap (guards one huge single line).
const MAX_OUTPUT_BYTES: usize = 200_000;

/// Caps line-oriented output to `max_lines`, and to the shared character budget, appending a
/// one-line note about what was dropped.
///
/// `unit` names the thing being counted (`"matches"`, `"files"`, `"lines"`) and `hint` tells the
/// model how to recover the rest (narrow the query, use offset/limit, scope to a path). Output at
/// or under both caps is returned untouched. The character budget is the one `read` keeps: a line
/// count alone would let a diff of one minified file through at the 200 KB backstop.
pub(crate) fn cap_lines(text: &str, max_lines: usize, unit: &str, hint: &str) -> String {
    cap_lines_within(text, max_lines, luxe_core::output_limit::RESULT_MAX_CHARS, unit, hint)
}

/// [`cap_lines`] under a character budget as well: whichever bound binds first ends the output.
///
/// A line cap is not a token cap, because a line has no length limit: a broad `grep` over generated
/// output can fit tens of kilobytes inside its 200-line cap. So the count alone cannot be the whole
/// promise, and a tool whose output is *wide* rather than long needs the second bound.
///
/// The two are folded into one cut so the note stays last and stays true: capping by line and then
/// by character would slice off the very footer that says how much went missing. At least one line
/// always survives, however long it is — [`clamp_bytes`] is what stands behind that one.
pub(crate) fn cap_lines_within(
    text: &str,
    max_lines: usize,
    max_chars: usize,
    unit: &str,
    hint: &str,
) -> String {
    let total = text.lines().count();
    let kept_lines =
        lines_within(text.lines().map(|line| line.chars().count()), max_lines, max_chars);
    if kept_lines >= total {
        return clamp_bytes(text);
    }
    let kept = text.lines().take(kept_lines).collect::<Vec<_>>().join("\n");
    with_note(&kept, total - kept_lines, total, unit, hint)
}

/// How many lines, given each one's length in characters, fit under both `max_lines` and
/// `max_chars`; at least one when there is any, however long it is.
///
/// The cut [`cap_lines_within`] makes, for a tool that lays its lines out after choosing them.
pub(crate) fn lines_within(
    lengths: impl IntoIterator<Item = usize>,
    max_lines: usize,
    max_chars: usize,
) -> usize {
    let mut kept = 0usize;
    let mut spent = 0usize;
    for length in lengths.into_iter().take(max_lines) {
        let cost = length.saturating_add(1);
        if kept > 0 && spent.saturating_add(cost) > max_chars {
            break;
        }
        spent = spent.saturating_add(cost);
        kept += 1;
    }
    kept
}

/// `kept`, then the note that `remaining` more of `total` were cut and how to see them, clamped to
/// the byte backstop.
pub(crate) fn with_note(
    kept: &str,
    remaining: usize,
    total: usize,
    unit: &str,
    hint: &str,
) -> String {
    clamp_bytes(&format!("{kept}\n\n… {remaining} more {unit} not shown ({total} total). {hint}"))
}

/// [`clamp_bytes`] for a tool that caps its own lines first.
pub(crate) fn clamp(text: &str) -> String {
    clamp_bytes(text)
}

/// Head+tail byte clamp for the rare single-huge-line case the line cap can't shrink.
fn clamp_bytes(text: &str) -> String {
    if text.len() <= MAX_OUTPUT_BYTES {
        return text.to_string();
    }
    // Slice on char boundaries so we never split a multi-byte codepoint.
    let head_end = floor_boundary(text, MAX_OUTPUT_BYTES / 2);
    let tail_start = ceil_boundary(text, text.len() - MAX_OUTPUT_BYTES / 2);
    format!(
        "{}\n-- clamped {} bytes --\n{}",
        &text[..head_end],
        text.len() - MAX_OUTPUT_BYTES,
        &text[tail_start..]
    )
}

pub(crate) use luxe_core::output_limit::{ceil_boundary, floor_boundary};

/// The head-keeping character cap, shared with every tool that answers, `mcp` included.
pub(crate) use luxe_core::output_limit::cap_chars;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_output_is_untouched() {
        let text = "a\nb\nc";
        assert_eq!(cap_lines(text, 10, "lines", "hint"), text);
    }

    #[test]
    fn long_output_keeps_head_and_notes_the_remainder() {
        let text = (1..=50).map(|n| n.to_string()).collect::<Vec<_>>().join("\n");
        let capped = cap_lines(&text, 10, "matches", "Narrow the pattern.");
        assert!(capped.starts_with("1\n2\n"));
        assert!(capped.contains("10"));
        assert!(!capped.contains("\n11\n"), "line 11 is dropped");
        assert!(capped.contains("40 more matches not shown (50 total)"));
        assert!(capped.contains("Narrow the pattern."));
    }

    /// A line cap is not a token cap. Four short lines fit; four long ones do not, and the caller
    /// gets the same honest footer either way.
    #[test]
    fn a_character_budget_binds_before_the_line_cap_when_the_lines_are_wide() {
        let narrow = (1..=100).map(|n| format!("row {n}\n")).collect::<String>();
        assert_eq!(
            cap_lines_within(&narrow, 200, 10_000, "matching lines", "Narrow it."),
            narrow,
            "a hundred short rows are under both bounds"
        );

        let wide = (1..=100).map(|n| format!("row {n} {}\n", "x".repeat(500))).collect::<String>();
        let capped = cap_lines_within(&wide, 200, 10_000, "matching lines", "Narrow it.");
        assert!(capped.chars().count() < 11_000, "the budget binds: {}", capped.chars().count());
        assert!(capped.contains("more matching lines not shown (100 total)"), "{capped}");
        assert!(capped.trim_end().ends_with("Narrow it."), "the note survives its own cut");
    }

    /// The footer is the first thing a naive second pass would cut, and it is the one line the
    /// reader cannot do without.
    #[test]
    fn the_note_is_never_truncated_away_by_the_character_budget() {
        let text = (1..=50).map(|n| format!("{}\n", "y".repeat(100 + n))).collect::<String>();
        let capped = cap_lines_within(&text, 50, 400, "matches", "Use output_mode files.");
        assert!(capped.contains("not shown (50 total)"), "{capped}");
        assert!(capped.contains("Use output_mode files."), "{capped}");
    }

    /// The budget is what fits, not what almost fits: output landing exactly on it is whole output
    /// and must not be reported as truncated.
    #[test]
    fn output_that_lands_exactly_on_the_budget_is_untouched() {
        // Four rows of four characters, each costing its own newline: twenty on the nose.
        let text = "aaaa\nbbbb\ncccc\ndddd\n";
        assert_eq!(cap_lines_within(text, 200, 20, "matches", "Narrow it."), text);

        let tighter = cap_lines_within(text, 200, 19, "matches", "Narrow it.");
        assert!(tighter.contains("1 more matches not shown (4 total)"), "{tighter}");
    }

    /// A budget smaller than the first line still returns that line rather than nothing.
    #[test]
    fn one_line_always_survives_however_wide_it_is() {
        let text = format!("{}\nsecond\n", "z".repeat(5_000));
        let capped = cap_lines_within(&text, 200, 100, "matches", "Narrow it.");
        assert!(capped.starts_with(&"z".repeat(100)), "the first line is kept whole");
        assert!(capped.contains("1 more matches not shown (2 total)"), "{capped}");
    }

    /// `cap_lines` is the same cut with the character budget switched off, so every existing caller
    /// keeps the behavior it was written against.
    #[test]
    fn cap_lines_is_the_unbudgeted_case_of_cap_lines_within() {
        let text = (1..=80).map(|n| format!("line {n}\n")).collect::<String>();
        assert_eq!(
            cap_lines(&text, 25, "lines", "hint"),
            cap_lines_within(&text, 25, usize::MAX, "lines", "hint")
        );
    }

    #[test]
    fn byte_clamp_handles_a_single_giant_line_on_char_boundaries() {
        let giant = "é".repeat(MAX_OUTPUT_BYTES); // 2 bytes each → well over the clamp
        let capped = cap_lines(&giant, 10, "lines", "hint");
        assert!(capped.contains("-- clamped"));
        assert!(capped.len() < giant.len());
    }
}
