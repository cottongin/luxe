//! Flagged — a review whose every hunk is flagged and would otherwise fold away, for tests of what
//! the reading list and the call-outs do past the cap of two a kind.

use luxe_core::guided_review::{GuidedReview, NoiseKind, ReviewStats, TurnProvenance};

/// A change adding a credential-shaped line to each of `secrets` (two, in two hunks, to the first)
/// and editing each of `unread` without reading it, every hunk formatting-only so that only its
/// flag keeps it on the list.
pub(crate) fn flagged_review(secrets: &[&str], unread: &[&str]) -> GuidedReview {
    let key = "ghp_9fKq2mZv8RtY4bNx7Wc3LpF6hJd5s"; // luxe:allow-secret
    let change = |path: &str, added: &str| {
        format!(
            "diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n@@ -1,1 +1,1 @@\n\
             -let token = \"\";\n+{added}\n"
        )
    };
    let secret = format!("let token = \"{key}\";");
    let diff: String = secrets
        .iter()
        .enumerate()
        .map(|(at, path)| match at {
            // two credential lines in two hunks of the first file: two flags, one thing to say
            0 => {
                change(path, &secret) + &format!("@@ -9,1 +9,1 @@\n-let spare = \"\";\n+{secret}\n")
            }
            _ => change(path, &secret),
        })
        .chain(unread.iter().map(|path| change(path, "let token = None;")))
        .collect();
    let mut files = luxe_core::guided_review::parse_unified_diff(&diff).expect("parses").files;
    for hunk in files.iter_mut().flat_map(|file| file.hunks.iter_mut()) {
        hunk.noise = NoiseKind::FormattingOnly;
    }
    let mut review = super::fixture_demo_guided_review();
    review.chapters = luxe_core::guided_review::fallback_chapters(&files).expect("chapters");
    review.stats = ReviewStats::from_files(&files);
    review.files = files;
    review.provenance = Some(TurnProvenance {
        written_paths: unread.iter().map(|path| path.to_string()).collect(),
        ..TurnProvenance::default()
    });
    (review.insight, review.seen) = (None, None);
    review
}
