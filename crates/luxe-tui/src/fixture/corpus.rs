//! The small review corpus — one change, its provenance and its noise, pinned for the assertions
//! that read it.

use luxe_core::guided_review::{
    ChapterId, ChapterKind, FileId, GuidedReview, HunkId, LineRange, NoiseKind, ReviewBase,
    ReviewChapter, ReviewContext, ReviewFile, ReviewFileStatus, ReviewHunk, ReviewId,
    ReviewItemRef, ReviewLine, ReviewLineKind, ReviewRevisionId, ReviewSource, ReviewStats,
    ReviewTarget,
};
use std::path::Path;

/// A clean empty-tree working-tree source pinned to `content_digest`.
pub(crate) fn fixture_source(content_digest: ReviewRevisionId) -> ReviewSource {
    ReviewSource {
        base: ReviewBase::EmptyTree,
        content_digest,
        includes_staged: false,
        includes_unstaged: false,
        included_untracked_files: 0,
        captured_at_ms: 0,
        uncovered_paths: Vec::new(),
    }
}

pub(super) fn fixture_guided_review() -> GuidedReview {
    let revision_id = ReviewRevisionId::from_canonical_bytes(b"fixture guided review");
    let file_id = FileId::from_path_lineage(None, Some(Path::new("src/notification.rs")));
    let hunk_id = HunkId::from_content(&file_id, "notification model", "pub struct Notification");
    let hunk = ReviewHunk {
        id: hunk_id.clone(),
        old_range: LineRange::new(4, 3),
        new_range: LineRange::new(4, 4),
        heading: Some("pub struct Notification".into()),
        symbol: None,
        lines: vec![
            ReviewLine {
                old_line: Some(4),
                new_line: Some(4),
                kind: ReviewLineKind::Context,
                text: "pub struct Notification {".into(),
            },
            ReviewLine {
                old_line: Some(5),
                new_line: None,
                kind: ReviewLineKind::Deletion,
                text: "    pub category: String,".into(),
            },
            ReviewLine {
                old_line: None,
                new_line: Some(5),
                kind: ReviewLineKind::Addition,
                text: "    pub category: NotificationCategory,".into(),
            },
            ReviewLine {
                old_line: Some(6),
                new_line: Some(6),
                kind: ReviewLineKind::Context,
                text: "    pub title: String,".into(),
            },
        ],
        noise: NoiseKind::Semantic,
    };
    let second_hunk_id =
        HunkId::from_content(&file_id, "notification display", "impl Notification");
    let second_hunk = ReviewHunk {
        id: second_hunk_id.clone(),
        old_range: LineRange::new(18, 1),
        new_range: LineRange::new(18, 2),
        heading: Some("impl Notification".into()),
        symbol: None,
        lines: vec![
            ReviewLine {
                old_line: Some(18),
                new_line: Some(18),
                kind: ReviewLineKind::Context,
                text: "impl Notification {".into(),
            },
            ReviewLine {
                old_line: None,
                new_line: Some(19),
                kind: ReviewLineKind::Addition,
                text: "    fn category_name(&self) -> &str { \"notification\" }".into(),
            },
        ],
        noise: NoiseKind::Semantic,
    };
    let file = ReviewFile {
        id: file_id,
        old_path: None,
        new_path: Some("src/notification.rs".into()),
        status: ReviewFileStatus::Modified,
        language: Some("rust".into()),
        additions: Some(2),
        deletions: Some(1),
        hunks: vec![hunk, second_hunk],
        binary: false,
        generated: false,
        non_text: None,
    };
    GuidedReview {
        conversation: None,
        id: ReviewId::new(),
        revision_id: revision_id.clone(),
        target: ReviewTarget::WorkingTree,
        source: fixture_source(revision_id),
        context: ReviewContext {
            title: "Refactor notification model".into(),
            goal: None,
            phase_index: Some(30),
            phase_title: Some("Guided Review model".into()),
            planning_excerpt: None,
            acceptance_excerpt: None,
            source_intent: None,
            source_commits: 0,
            pull_request: None,
        },
        files: vec![file],
        chapters: vec![ReviewChapter {
            id: ChapterId::from_items("core", &[hunk_id.clone(), second_hunk_id.clone()]),
            title: "Shared notification model".into(),
            rationale: "Separates category concepts so platform clients share one stable shape."
                .into(),
            impact: None,
            kind: ChapterKind::Core,
            items: vec![
                ReviewItemRef::Hunk(hunk_id.clone()),
                ReviewItemRef::Hunk(second_hunk_id.clone()),
            ],
            // The narration a reviewer actually reads: one sentence per piece of the change.
            notes: vec![
                luxe_core::guided_review::ItemNote {
                    item: hunk_id.as_str().to_owned(),
                    note: "Swaps the free-form `category: String` for the shared enum, so an invalid category stops being representable.".into(),
                },
                luxe_core::guided_review::ItemNote {
                    item: second_hunk_id.as_str().to_owned(),
                    note: "Adds `category_name` so the dispatcher and the Swift client render the same label from the enum.".into(),
                },
            ],
        }],
        // Real review-gate results only. The harness does not invent checks from the agent's
        // shell history, so an interactive review has none.
        checks: luxe_core::guided_review::CheckSummary::default(),
        stats: ReviewStats {
            files: 1,
            hunks: 2,
            additions: Some(2),
            deletions: Some(1),
            ..ReviewStats::default()
        },
        generated_at_ms: 0,
        insight: Some(luxe_core::guided_review::ReviewInsight {
            files: vec![luxe_core::guided_review::FileInsight {
                file: "src/notification.rs".into(),
                relevant: vec![
                    luxe_core::guided_review::RelevantFile {
                        file: "src/dispatcher.rs".into(),
                        reason: "dispatch calls category_name".into(),
                    },
                    luxe_core::guided_review::RelevantFile {
                        file: "tests/notification.rs".into(),
                        reason: "renders_category tests category_name".into(),
                    },
                ],
            }],
            untested_symbols: vec!["src/notification.rs::category_name".into()],
            covered_symbols: vec!["src/notification.rs::Notification".into()],
        }),
        provenance: Some(fixture_turn_provenance()),
        seen: None,
        brief: Some(luxe_core::guided_review::ReviewBrief {
            intent: "Notification categories were free-form strings, so a typo reached the \
                     dispatcher and rendered as an empty label."
                .into(),
            account: vec![
                "Replaces `category: String` with a `Category` enum, making an invalid category \
                 unrepresentable."
                    .into(),
                "Adds `category_name` so the dispatcher and the Swift client render one label \
                 from one source."
                    .into(),
            ],
        }),
    }
}

pub(super) fn fixture_turn_provenance() -> luxe_core::guided_review::TurnProvenance {
    luxe_core::guided_review::TurnProvenance {
        requests: vec!["Make notification categories type-safe across both clients".into()],
        account: Some(
            "Replaced the free-form String category with a shared NotificationCategory enum and \
             added category_name so the dispatcher and the Swift client agree on one shape."
                .into(),
        ),
        steps: vec![
            "read src/notification.rs".into(),
            "edit src/notification.rs".into(),
            "bash cargo test --workspace".into(),
        ],
        read_paths: ["src/notification.rs".to_owned()].into_iter().collect(),
        written_paths: ["src/notification.rs".to_owned(), "src/dispatcher.rs".to_owned()]
            .into_iter()
            .collect(),
        commands: vec![luxe_core::guided_review::CommandRun {
            command: "cargo test --workspace".into(),
            failed: false,
            tool: "bash".into(),
            after_last_edit: true,
        }],
        had_tool_failure: false,
        verified_after_last_edit: true,
    }
}

pub(super) fn fixture_guided_review_with_noise() -> GuidedReview {
    let mut review = fixture_guided_review();
    let file = &mut review.files[0];
    let hunk_id = HunkId::from_content(&file.id, "format imports", "use std::{fmt, io};");
    file.hunks.push(ReviewHunk {
        id: hunk_id.clone(),
        old_range: LineRange::new(1, 1),
        new_range: LineRange::new(1, 1),
        heading: Some("imports".into()),
        symbol: None,
        lines: vec![
            ReviewLine {
                old_line: Some(1),
                new_line: None,
                kind: ReviewLineKind::Deletion,
                text: "use std::{fmt,io};".into(),
            },
            ReviewLine {
                old_line: None,
                new_line: Some(1),
                kind: ReviewLineKind::Addition,
                text: "use std::{fmt, io};".into(),
            },
        ],
        noise: NoiseKind::FormattingOnly,
    });
    review.chapters[0].items.push(ReviewItemRef::Hunk(hunk_id));
    review.stats.hunks = 3;
    review.stats.additions = Some(3);
    review.stats.deletions = Some(2);
    review
}
