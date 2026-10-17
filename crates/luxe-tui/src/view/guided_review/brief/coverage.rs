//! Coverage — what a review leaves out, said where the reader starts.
//!
//! Counted, never silent: a review that shows less than the diff has to say so, whether a project
//! asked for the omission or the capture could not hold a file.

use luxe_core::guided_review::GuidedReview;
use ratatui::text::{Line, Span};

use crate::theme::styles::ThemeStyles;

/// The rows saying what this review does not show, in the order they matter.
pub(super) fn coverage_lines(review: &GuidedReview, styles: &ThemeStyles) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if let Some(uncovered) = uncovered_note(review) {
        lines.push(Line::from(Span::styled(uncovered, styles.warning)));
    }
    if review.stats.excluded_files > 0 {
        lines.push(Line::from(Span::styled(
            format!(
                "{} {} excluded by review.exclude",
                review.stats.excluded_files,
                if review.stats.excluded_files == 1 { "file" } else { "files" }
            ),
            styles.dim,
        )));
    }
    lines
}

/// Names the changed files the capture could not include, a few of them, so a clean review of the
/// rest never reads as a clean review of everything.
fn uncovered_note(review: &GuidedReview) -> Option<String> {
    const NAMED: usize = 3;
    let paths = &review.source.uncovered_paths;
    if paths.is_empty() {
        return None;
    }
    let named: Vec<String> =
        paths.iter().take(NAMED).map(|path| luxe_core::paths::display(path)).collect();
    let more = match paths.len().saturating_sub(NAMED) {
        0 => String::new(),
        rest => format!(" and {rest} more"),
    };
    Some(format!(
        "Changed but not shown, too large or not a readable file: {}{more}",
        named.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A review missing files names them, a few at a time, and a complete one says nothing.
    #[test]
    fn a_review_says_which_files_it_could_not_cover() {
        let mut review = crate::fixture::fixture_demo_guided_review();
        assert_eq!(uncovered_note(&review), None);

        review.source.uncovered_paths = vec!["docs/big.md".into()];
        let note = uncovered_note(&review).expect("said");
        assert!(note.ends_with(": docs/big.md"), "{note}");

        review.source.uncovered_paths = ["a", "b", "c", "d", "e"].map(Into::into).to_vec();
        let note = uncovered_note(&review).expect("said");
        assert!(note.ends_with(": a, b, c and 2 more"), "{note}");
    }
}
