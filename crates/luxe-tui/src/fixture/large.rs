//! The large review — a generated change big enough to exercise scrolling, folding and scale.

use super::corpus::{fixture_source, fixture_turn_provenance};
use luxe_core::guided_review::{
    ChapterId, FileId, GuidedReview, HunkId, LineRange, NoiseKind, ReviewBase, ReviewFile,
    ReviewFileStatus, ReviewHunk, ReviewId, ReviewLine, ReviewLineKind, ReviewRevisionId,
    ReviewSource, ReviewTarget,
};
use std::path::Path;

/// A review at the scale the feature exists for: a change too large to read unaided.
///
/// Forty-four files and three hundred-odd hunks across seven chapters, with the awkward cases a
/// real review contains — folded formatting churn, a lockfile, a generated file, a rename, and a
/// binary asset. Claims about behaving at scale should be measured against something this size,
/// not against a two-hunk sample.
pub fn fixture_large_guided_review() -> GuidedReview {
    use luxe_core::guided_review::{
        ChapterKind, NonTextChange, NonTextChangeKind, ReviewChapter, ReviewItemRef, ReviewStats,
    };
    use std::path::PathBuf;

    /// One themed group of files, and the chapter it becomes.
    struct Section {
        title: &'static str,
        rationale: &'static str,
        kind: ChapterKind,
        directory: &'static str,
        files: usize,
        hunks: usize,
        noise: NoiseKind,
    }

    const SECTIONS: [Section; 7] = [
        Section {
            title: "Shared notification model",
            rationale: "Separates category, route, and parse-failure concepts so both platform clients share one shape.",
            kind: ChapterKind::Core,
            directory: "crates/luxe-core/src/notification",
            files: 9,
            hunks: 10,
            noise: NoiseKind::Semantic,
        },
        Section {
            title: "Dispatcher integration",
            rationale: "Threads the new category through every dispatch path that used the old string.",
            kind: ChapterKind::Consequence,
            directory: "crates/luxe-core/src/dispatch",
            files: 8,
            hunks: 9,
            noise: NoiseKind::Semantic,
        },
        Section {
            title: "Front-end rendering",
            rationale: "Renders the category badge where the raw string used to be interpolated.",
            kind: ChapterKind::Consequence,
            directory: "crates/luxe-tui/src/view",
            files: 7,
            hunks: 8,
            noise: NoiseKind::Semantic,
        },
        Section {
            title: "Coverage for the new shape",
            rationale: "Adds parse-failure and round-trip cases the old string type could not express.",
            kind: ChapterKind::Test,
            directory: "crates/luxe-core/tests",
            files: 9,
            hunks: 6,
            noise: NoiseKind::Semantic,
        },
        Section {
            title: "Docs and configuration",
            rationale: "Documents the category contract and its configuration surface.",
            kind: ChapterKind::Supporting,
            directory: "docs/notifications",
            files: 5,
            hunks: 3,
            noise: NoiseKind::Semantic,
        },
        Section {
            title: "Formatter churn",
            rationale: "Whitespace the formatter rewrote while the real change was being made.",
            kind: ChapterKind::Supporting,
            directory: "crates/luxe-providers/src",
            files: 5,
            hunks: 4,
            noise: NoiseKind::FormattingOnly,
        },
        Section {
            title: "Generated and mechanical",
            rationale: "Regenerated schema output and lockfile movement.",
            kind: ChapterKind::Generated,
            directory: "generated",
            files: 4,
            hunks: 3,
            noise: NoiseKind::Generated,
        },
    ];

    let mut files: Vec<ReviewFile> = Vec::new();
    let mut chapters: Vec<ReviewChapter> = Vec::new();

    for section in &SECTIONS {
        let mut items = Vec::new();
        for index in 0..section.files {
            let path = PathBuf::from(format!("{}/module_{index}.rs", section.directory));
            let id = FileId::from_path_lineage(None, Some(&path));
            let hunks: Vec<ReviewHunk> = (0..section.hunks)
                .map(|hunk_index| {
                    let start = 12 + hunk_index as u32 * 40;
                    ReviewHunk {
                        id: HunkId::from_content(
                            &id,
                            &format!("{}-{index}-{hunk_index}", section.directory),
                            "changed",
                        ),
                        old_range: LineRange::new(start, 4),
                        new_range: LineRange::new(start, 5),
                        heading: Some(format!("fn handle_{hunk_index}")),
                        symbol: None,
                        lines: large_hunk_lines(start),
                        noise: section.noise,
                    }
                })
                .collect();
            items.extend(hunks.iter().map(|hunk| ReviewItemRef::Hunk(hunk.id.clone())));
            files.push(ReviewFile {
                id,
                old_path: None,
                new_path: Some(path),
                status: ReviewFileStatus::Modified,
                language: Some("rust".into()),
                additions: Some(section.hunks as u64 * 2),
                deletions: Some(section.hunks as u64),
                hunks,
                binary: false,
                generated: section.noise == NoiseKind::Generated,
                non_text: None,
            });
        }
        chapters.push(ReviewChapter {
            id: ChapterId::from_item_keys(
                section.title,
                &items
                    .iter()
                    .map(|item| match item {
                        ReviewItemRef::Hunk(id) => id.as_str(),
                        ReviewItemRef::File(id) => id.as_str(),
                    })
                    .collect::<Vec<_>>(),
            ),
            title: section.title.into(),
            rationale: section.rationale.into(),
            impact: None,
            kind: section.kind,
            // One sentence per item, as a real guide writes them.
            notes: items
                .iter()
                .map(|item| luxe_core::guided_review::ItemNote {
                    item: match item {
                        ReviewItemRef::Hunk(id) => id.as_str().to_owned(),
                        ReviewItemRef::File(id) => id.as_str().to_owned(),
                    },
                    note: format!("Part of {}: {}", section.title, section.rationale),
                })
                .collect(),
            items,
        });
    }

    // The awkward cases: a rename with no text change, and a binary asset.
    let renamed_path = PathBuf::from("crates/luxe-tui/src/themes/rose-pine.json");
    let renamed = ReviewFile {
        id: FileId::from_path_lineage(
            Some(Path::new("crates/luxe-tui/themes/rose-pine.json")),
            Some(&renamed_path),
        ),
        old_path: Some("crates/luxe-tui/themes/rose-pine.json".into()),
        new_path: Some(renamed_path),
        status: ReviewFileStatus::Renamed,
        language: Some("json".into()),
        additions: Some(0),
        deletions: Some(0),
        hunks: Vec::new(),
        binary: false,
        generated: false,
        non_text: Some(NonTextChange {
            kind: NonTextChangeKind::Rename,
            summary: "Renamed with no content change".into(),
        }),
    };
    let binary_path = PathBuf::from("assets/notification.png");
    let binary = ReviewFile {
        id: FileId::from_path_lineage(None, Some(&binary_path)),
        old_path: None,
        new_path: Some(binary_path),
        status: ReviewFileStatus::Added,
        language: None,
        additions: None,
        deletions: None,
        hunks: Vec::new(),
        binary: true,
        generated: false,
        non_text: Some(NonTextChange {
            kind: NonTextChangeKind::Binary,
            summary: "Binary files differ".into(),
        }),
    };
    if let Some(chapter) = chapters.last_mut() {
        chapter.items.push(ReviewItemRef::File(renamed.id.clone()));
        chapter.items.push(ReviewItemRef::File(binary.id.clone()));
    }
    files.push(renamed);
    files.push(binary);

    let revision_id = ReviewRevisionId::from_canonical_bytes(b"fixture large guided review");
    GuidedReview {
        conversation: None,
        id: ReviewId::new(),
        revision_id: revision_id.clone(),
        target: ReviewTarget::WorkingTree,
        source: ReviewSource {
            base: ReviewBase::Head { oid: "0".repeat(40) },
            includes_staged: true,
            includes_unstaged: true,
            included_untracked_files: 1,
            captured_at_ms: 1,
            ..fixture_source(revision_id)
        },
        context: luxe_core::guided_review::ReviewContext::bounded(
            "Type-safe notification categories",
            Some("Make notification categories type-safe across both clients"),
            None,
            None::<&str>,
            None::<&str>,
            None::<&str>,
        ),
        stats: ReviewStats::from_files(&files),
        files,
        chapters,
        checks: luxe_core::guided_review::CheckSummary::default(),
        generated_at_ms: 2,
        insight: None,
        provenance: Some(fixture_turn_provenance()),
        seen: None,
        brief: None,
    }
}

/// A short realistic hunk body: context, a replacement, and an addition.
fn large_hunk_lines(start: u32) -> Vec<ReviewLine> {
    vec![
        ReviewLine {
            old_line: Some(start),
            new_line: Some(start),
            kind: ReviewLineKind::Context,
            text: "    let category = notification.category();".into(),
        },
        ReviewLine {
            old_line: Some(start + 1),
            new_line: None,
            kind: ReviewLineKind::Deletion,
            text: "    let label: String = category.to_string();".into(),
        },
        ReviewLine {
            old_line: None,
            new_line: Some(start + 1),
            kind: ReviewLineKind::Addition,
            text: "    let label = category.display_name();".into(),
        },
        ReviewLine {
            old_line: None,
            new_line: Some(start + 2),
            kind: ReviewLineKind::Addition,
            text: "    debug_assert!(!label.is_empty());".into(),
        },
        ReviewLine {
            old_line: Some(start + 2),
            new_line: Some(start + 3),
            kind: ReviewLineKind::Context,
            text: "    emit(label);".into(),
        },
    ]
}
