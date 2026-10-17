//! The demo review — built from its literal change, with the counts and provenance it implies.

use super::corpus::fixture_source;
use super::demo_spec::{DemoHunk, DEMO_CHANGE};
use luxe_core::guided_review::{
    ChapterId, FileId, GuidedReview, HunkId, LineRange, ReviewChapter, ReviewContext, ReviewFile,
    ReviewHunk, ReviewId, ReviewItemRef, ReviewLine, ReviewLineKind, ReviewRevisionId,
    ReviewSource, ReviewStats, ReviewTarget,
};
use std::path::Path;

/// What the demo's agent was asked, did, and verified.
///
/// Its own rather than [`fixture_turn_provenance`], because an unread edit is checked against the
/// paths it names, and the small corpus's paths are not the demo's.
fn demo_turn_provenance() -> luxe_core::guided_review::TurnProvenance {
    // `src/render/badge.rs` is absent: a formatter reordered its imports during the turn, and the
    // review accounts for that hunk as formatting
    let paths = [
        "src/notification.rs",
        "src/notification/category.rs",
        "src/dispatch/router.rs",
        "src/api/webhook.rs",
        "tests/notification_category.rs",
    ];
    luxe_core::guided_review::TurnProvenance {
        requests: vec!["Stop notification categories being free-form strings".into()],
        account: Some(
            "Added a Category enum, parsed the inbound string once at the webhook, and made the \
             router match exhaustively."
                .into(),
        ),
        steps: vec![
            "grep category: String".into(),
            "write src/notification/category.rs".into(),
            "edit src/notification.rs".into(),
            "edit src/dispatch/router.rs".into(),
            "edit src/api/webhook.rs".into(),
            "write tests/notification_category.rs".into(),
            "test cargo nextest run --workspace".into(),
        ],
        read_paths: paths.iter().map(|path| (*path).to_owned()).collect(),
        written_paths: paths.iter().map(|path| (*path).to_owned()).collect(),
        commands: vec![luxe_core::guided_review::CommandRun {
            command: "cargo nextest run --workspace".into(),
            failed: false,
            tool: "test".into(),
            after_last_edit: true,
        }],
        had_tool_failure: false,
        verified_after_last_edit: true,
    }
}

/// A demo hunk's added and removed lines, and the lines on each side its header counts, context
/// included, as a real header does.
fn demo_counts(lines: &[ReviewLine]) -> (u64, u64, u32, u32) {
    let count = |keep: fn(&ReviewLine) -> bool| lines.iter().filter(|line| keep(line)).count();
    (
        count(|line| line.kind == ReviewLineKind::Addition) as u64,
        count(|line| line.kind == ReviewLineKind::Deletion) as u64,
        count(|line| line.old_line.is_some()) as u32,
        count(|line| line.new_line.is_some()) as u32,
    )
}

/// Turns a diff-shaped line spec into review lines, counting the numbers rather than trusting them.
fn demo_lines(hunk: &DemoHunk) -> Vec<ReviewLine> {
    let (mut old_line, mut new_line) = (hunk.old_start, hunk.new_start);
    hunk.lines
        .iter()
        .map(|raw| {
            let (kind, text) = match raw.split_at(1) {
                ("-", rest) => (ReviewLineKind::Deletion, rest),
                ("+", rest) => (ReviewLineKind::Addition, rest),
                (_, _) => (ReviewLineKind::Context, &raw[raw.len().min(1)..]),
            };
            match kind {
                ReviewLineKind::Deletion => {
                    let at = old_line;
                    old_line += 1;
                    ReviewLine { old_line: Some(at), new_line: None, kind, text: text.into() }
                }
                ReviewLineKind::Addition => {
                    let at = new_line;
                    new_line += 1;
                    ReviewLine { old_line: None, new_line: Some(at), kind, text: text.into() }
                }
                _ => {
                    let (old_at, new_at) = (old_line, new_line);
                    old_line += 1;
                    new_line += 1;
                    ReviewLine {
                        old_line: Some(old_at),
                        new_line: Some(new_at),
                        kind: ReviewLineKind::Context,
                        text: text.into(),
                    }
                }
            }
        })
        .collect()
}

pub fn fixture_demo_guided_review() -> GuidedReview {
    let revision_id = ReviewRevisionId::from_canonical_bytes(b"fixture demo guided review");
    let mut files: Vec<ReviewFile> = Vec::new();
    let mut chapters: Vec<ReviewChapter> = Vec::new();
    let (mut additions, mut deletions) = (0_u64, 0_u64);

    for chapter in DEMO_CHANGE {
        let mut items = Vec::new();
        let mut notes = Vec::new();
        for spec in chapter.files {
            let path = Path::new(spec.path);
            let id = FileId::from_path_lineage(None, Some(path));
            let mut hunks = Vec::new();
            let (mut file_additions, mut file_deletions) = (0_u64, 0_u64);
            for hunk in spec.hunks {
                let lines = demo_lines(hunk);
                let (added, removed, old_count, new_count) = demo_counts(&lines);
                file_additions += added;
                file_deletions += removed;
                let hunk_id = HunkId::from_content(&id, hunk.key, hunk.heading);
                items.push(ReviewItemRef::Hunk(hunk_id.clone()));
                notes.push(luxe_core::guided_review::ItemNote {
                    item: hunk_id.as_str().to_owned(),
                    note: hunk.note.into(),
                });
                hunks.push(ReviewHunk {
                    id: hunk_id,
                    old_range: LineRange::new(hunk.old_start, old_count),
                    new_range: LineRange::new(hunk.new_start, new_count),
                    heading: Some(hunk.heading.into()),
                    symbol: None,
                    lines,
                    noise: spec.noise,
                });
            }
            additions += file_additions;
            deletions += file_deletions;
            files.push(ReviewFile {
                id,
                old_path: None,
                new_path: Some(path.to_path_buf()),
                status: spec.status,
                language: Some("rust".into()),
                additions: Some(file_additions),
                deletions: Some(file_deletions),
                hunks,
                binary: false,
                generated: false,
                non_text: None,
            });
        }
        chapters.push(ReviewChapter {
            id: ChapterId::from_item_keys(
                chapter.title,
                &items
                    .iter()
                    .map(|item| match item {
                        ReviewItemRef::Hunk(id) => id.as_str(),
                        ReviewItemRef::File(id) => id.as_str(),
                    })
                    .collect::<Vec<_>>(),
            ),
            title: chapter.title.into(),
            rationale: chapter.rationale.into(),
            impact: None,
            kind: chapter.kind,
            notes,
            items,
        });
    }

    let file_count = files.len() as u64;
    let hunk_count = files.iter().map(|file| file.hunks.len() as u64).sum();
    GuidedReview {
        conversation: None,
        id: ReviewId::new(),
        revision_id: revision_id.clone(),
        target: ReviewTarget::WorkingTree,
        source: ReviewSource {
            includes_unstaged: true,
            included_untracked_files: 2,
            ..fixture_source(revision_id)
        },
        context: ReviewContext {
            title: "Type the notification category".into(),
            goal: None,
            phase_index: None,
            phase_title: None,
            planning_excerpt: None,
            acceptance_excerpt: None,
            source_intent: None,
            source_commits: 0,
            pull_request: None,
        },
        files,
        chapters,
        checks: luxe_core::guided_review::CheckSummary::default(),
        stats: ReviewStats {
            files: file_count,
            hunks: hunk_count,
            additions: Some(additions),
            deletions: Some(deletions),
            ..ReviewStats::default()
        },
        generated_at_ms: 0,
        insight: Some(luxe_core::guided_review::ReviewInsight {
            files: vec![luxe_core::guided_review::FileInsight {
                file: "src/notification/category.rs".into(),
                relevant: vec![
                    luxe_core::guided_review::RelevantFile {
                        file: "src/dispatch/router.rs".into(),
                        reason: "routes on Category".into(),
                    },
                    luxe_core::guided_review::RelevantFile {
                        file: "src/api/webhook.rs".into(),
                        reason: "parses the inbound string".into(),
                    },
                ],
            }],
            untested_symbols: vec!["src/api/webhook.rs::receive".into()],
            covered_symbols: vec![
                "src/notification/category.rs::Category".into(),
                "src/notification.rs::label".into(),
            ],
        }),
        provenance: Some(demo_turn_provenance()),
        seen: None,
        brief: Some(luxe_core::guided_review::ReviewBrief {
            intent: "A notification category was a free-form string, so a typo type-checked, \
                     reached the router, and rendered as an empty badge."
                .into(),
            account: vec![
                "Adds a `Category` enum and parses the inbound string once, at the HTTP boundary."
                    .into(),
                "Replaces the router's string comparison with an exhaustive match, so a new \
                 category fails to compile rather than routing to the digest."
                    .into(),
                "Covers the typo case, which the old shape could not express.".into(),
            ],
        }),
    }
}
