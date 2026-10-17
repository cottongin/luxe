//! Test support — a working-tree review built from a unified diff, for unit tests in core.

use super::{
    fallback_chapters, parse_unified_diff, CheckSummary, GuidedReview, ReviewBase,
    ReviewContextDraft, ReviewId, ReviewRevisionId, ReviewSource, ReviewStats, ReviewTarget,
};

/// Two files, one hunk each: source and a test, so structural chapters give each its own.
pub(crate) const TWO_FILES: &str = "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,1 +1,1 @@\n-let a = 1;\n+let a = 2;\ndiff --git a/tests/b.rs b/tests/b.rs\n--- a/tests/b.rs\n+++ b/tests/b.rs\n@@ -1,1 +1,2 @@\n let b = 1;\n+let c = 3;\n";

/// A pinned working-tree review of `diff`, with structural chapters, under a fresh id.
pub(crate) fn review_from(diff: &str) -> GuidedReview {
    let files = parse_unified_diff(diff).expect("fixture diff parses").files;
    let revision_id = ReviewRevisionId::from_canonical_bytes(diff.as_bytes());
    GuidedReview {
        conversation: None,
        id: ReviewId::new(),
        revision_id: revision_id.clone(),
        target: ReviewTarget::WorkingTree,
        source: ReviewSource {
            base: ReviewBase::EmptyTree,
            content_digest: revision_id,
            includes_staged: false,
            includes_unstaged: true,
            included_untracked_files: 0,
            captured_at_ms: 0,
            uncovered_paths: Vec::new(),
        },
        context: ReviewContextDraft::default().build(),
        stats: ReviewStats::from_files(&files),
        chapters: fallback_chapters(&files).expect("fixture chapters cover every item"),
        files,
        checks: CheckSummary::default(),
        generated_at_ms: 0,
        insight: None,
        provenance: None,
        seen: None,
        brief: None,
    }
}
