mod findings;
mod layout;
mod navigation;
mod refresh;
mod search;
mod sources;

use super::*;
use luxe_core::guided_review::{
    LineRange, NoiseKind, ReviewFileStatus, ReviewHunk, ReviewLine, ReviewLineKind,
};
use std::path::Path;

/// A minimal pinned review over `files`, with deterministic chapters, for exercising navigation
/// (which is chapter-scoped and therefore needs the whole review, not just its files).
fn review_of(files: &[ReviewFile]) -> luxe_core::guided_review::GuidedReview {
    use luxe_core::guided_review::{
        CheckSummary, ReviewContext, ReviewId, ReviewRevisionId, ReviewStats, ReviewTarget,
    };
    let revision_id = ReviewRevisionId::from_canonical_bytes(b"navigation fixture");
    luxe_core::guided_review::GuidedReview {
        conversation: None,
        id: ReviewId::new(),
        revision_id: revision_id.clone(),
        target: ReviewTarget::WorkingTree,
        source: luxe_core::guided_review::ReviewSource {
            includes_unstaged: true,
            ..crate::fixture::fixture_source(revision_id)
        },
        context: ReviewContext::bounded(
            "Navigation fixture",
            None::<&str>,
            None,
            None::<&str>,
            None::<&str>,
            None::<&str>,
        ),
        chapters: luxe_core::guided_review::fallback_chapters(files).unwrap_or_default(),
        stats: ReviewStats::from_files(files),
        files: files.to_vec(),
        checks: CheckSummary::default(),
        generated_at_ms: 0,
        insight: None,
        provenance: None,
        seen: None,
        brief: None,
    }
}

/// A file with both a removal and its replacement, which is what side-by-side needs to pair up.
///
/// [`text_file`] is `Added` and therefore one-sided, so it can never split however wide the pane
/// is — the layout rule asks about the content, not only the room.
fn changed_file(path: &str) -> ReviewFile {
    let mut file = text_file(path);
    file.status = ReviewFileStatus::Modified;
    file.hunks[0].lines = vec![
        ReviewLine {
            old_line: Some(1),
            new_line: None,
            kind: ReviewLineKind::Deletion,
            text: "let value = 1;".into(),
        },
        ReviewLine {
            old_line: None,
            new_line: Some(1),
            kind: ReviewLineKind::Addition,
            text: "let value = 2;".into(),
        },
    ];
    file
}

fn text_file(path: &str) -> ReviewFile {
    let id = FileId::from_path_lineage(None, Some(Path::new(path)));
    let hunk = ReviewHunk {
        id: HunkId::from_content(&id, "fixture", "let value = 1;"),
        old_range: LineRange::new(1, 1),
        new_range: LineRange::new(1, 1),
        heading: None,
        symbol: None,
        lines: vec![ReviewLine {
            old_line: Some(1),
            new_line: Some(1),
            kind: ReviewLineKind::Context,
            text: "let value = 1;".into(),
        }],
        noise: NoiseKind::Semantic,
    };
    ReviewFile {
        id,
        old_path: None,
        new_path: Some(path.into()),
        status: ReviewFileStatus::Added,
        language: Some("rust".into()),
        additions: Some(1),
        deletions: Some(0),
        hunks: vec![hunk],
        binary: false,
        generated: false,
        non_text: None,
    }
}
