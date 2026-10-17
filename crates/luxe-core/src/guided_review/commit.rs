//! Chapter commits — turning a finished review into history.
//!
//! Chapters are already a semantically grouped, hunk-exact partition of the change, which is
//! exactly the input an atomic-commit flow needs. Without this, a review transfers understanding
//! and then discards it: the reasoning that made the change legible never reaches the log, and the
//! next reader starts from nothing again.
//!
//! Reconstruction is mechanical and total. A chapter's patch is re-emitted from the *pinned*
//! review, new-side offsets are recomputed for the subset being applied, and anything that cannot
//! be represented as text (binary, submodule, mode-only) is reported rather than quietly skipped.

use super::{ChapterId, GuidedReview, ReviewChapter, ReviewFile, ReviewItemRef, ReviewLineKind};
use thiserror::Error;

/// Longest commit subject before the chapter title is elided.
pub const MAX_COMMIT_SUBJECT_CHARS: usize = 72;

/// Why a chapter cannot be committed from the pinned review.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ChapterCommitError {
    /// The chapter is not part of this review.
    #[error("chapter is not part of this review")]
    UnknownChapter,
    /// Every item in the chapter is non-text, so there is no patch to apply.
    #[error("chapter {title} contains only non-text changes, which cannot be re-applied")]
    NonTextOnly {
        /// User-facing chapter title.
        title: String,
    },
}

/// One chapter rendered as a commit: the message and the patch that produces it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChapterCommit {
    /// Chapter this commit came from.
    pub chapter_id: ChapterId,
    /// Commit subject line.
    pub subject: String,
    /// Full commit message including the body and review trailers.
    pub message: String,
    /// Unified diff to stage, always applying cleanly against the reviewed base.
    pub patch: String,
    /// Items the chapter owns that no patch can carry, named so nothing vanishes silently.
    pub skipped: Vec<String>,
}

/// Renders one commit per chapter, in Guide order.
///
/// `chapters` selects which to include; an empty selection means every chapter.
pub fn chapter_commits(
    review: &GuidedReview,
    selected: &[ChapterId],
) -> Result<Vec<ChapterCommit>, ChapterCommitError> {
    let chosen: Vec<&ReviewChapter> = if selected.is_empty() {
        review.chapters.iter().collect()
    } else {
        selected
            .iter()
            .map(|id| {
                review
                    .chapters
                    .iter()
                    .find(|chapter| &chapter.id == id)
                    .ok_or(ChapterCommitError::UnknownChapter)
            })
            .collect::<Result<Vec<_>, _>>()?
    };
    chosen.into_iter().map(|chapter| chapter_commit(review, chapter)).collect()
}

/// Renders every selected chapter as a single squashed commit.
pub fn squashed_commit(
    review: &GuidedReview,
    selected: &[ChapterId],
) -> Result<ChapterCommit, ChapterCommitError> {
    let commits = chapter_commits(review, selected)?;
    let first = commits.first().ok_or(ChapterCommitError::UnknownChapter)?;
    let subject = bounded_subject(&review.context.title);
    let mut message = format!("{subject}\n\n");
    for commit in &commits {
        message.push_str(&format!("- {}\n", commit.subject));
    }
    message.push('\n');
    message.push_str(&trailers(review));
    Ok(ChapterCommit {
        chapter_id: first.chapter_id.clone(),
        subject,
        message,
        patch: commits.iter().map(|commit| commit.patch.as_str()).collect(),
        skipped: commits.iter().flat_map(|commit| commit.skipped.clone()).collect(),
    })
}

fn chapter_commit(
    review: &GuidedReview,
    chapter: &ReviewChapter,
) -> Result<ChapterCommit, ChapterCommitError> {
    let (patch, skipped) = chapter_patch(review, chapter);
    if patch.is_empty() {
        return Err(ChapterCommitError::NonTextOnly { title: chapter.title.clone() });
    }
    let subject = bounded_subject(&chapter.title);
    // The rationale is the reason the chapter exists; it is precisely what a future reader of the
    // log needs and precisely what a diff cannot say.
    let mut message = format!("{subject}\n\n{}\n\n", chapter.rationale.trim());
    if let Some(impact) = chapter.impact.as_deref().filter(|text| !text.trim().is_empty()) {
        message.push_str(&format!("{}\n\n", impact.trim()));
    }
    message.push_str(&trailers(review));
    Ok(ChapterCommit { chapter_id: chapter.id.clone(), subject, message, patch, skipped })
}

/// Provenance trailers so a commit can be traced back to the review that produced it.
fn trailers(review: &GuidedReview) -> String {
    format!("Luxe-review: {}\nLuxe-revision: {}\n", review.id.as_str(), review.revision_id.as_str())
}

fn bounded_subject(title: &str) -> String {
    let subject: String = title
        .lines()
        .next()
        .unwrap_or_default()
        .chars()
        .filter(|character| !character.is_control())
        .collect();
    let subject = subject.trim();
    if subject.chars().count() <= MAX_COMMIT_SUBJECT_CHARS {
        return subject.to_owned();
    }
    let head: String = subject.chars().take(MAX_COMMIT_SUBJECT_CHARS.saturating_sub(1)).collect();
    format!("{}…", head.trim_end())
}

/// Re-emits the chapter's hunks as a unified diff, plus the items no patch can carry.
fn chapter_patch(review: &GuidedReview, chapter: &ReviewChapter) -> (String, Vec<String>) {
    let owned: std::collections::HashSet<&str> = chapter.items.iter().map(item_key).collect();
    let mut patch = String::new();
    let mut skipped = Vec::new();
    for file in &review.files {
        let path = display_path(file);
        if file.non_text.is_some() || file.binary {
            if owned.contains(file.id.as_str()) {
                skipped.push(path);
            }
            continue;
        }
        let included: Vec<&super::ReviewHunk> =
            file.hunks.iter().filter(|hunk| owned.contains(hunk.id.as_str())).collect();
        if included.is_empty() {
            continue;
        }
        patch.push_str(&file_header(file));
        // A subset of a file's hunks shifts every later new-side start by the net line delta of
        // the hunks actually being applied, so the numbers have to be recomputed rather than
        // copied from the full review.
        let mut delta: i64 = 0;
        for hunk in &included {
            let new_start = (i64::from(hunk.old_range.start) + delta).max(1) as u32;
            patch.push_str(&hunk_header(hunk, new_start));
            for line in &hunk.lines {
                match line.kind {
                    ReviewLineKind::Addition => {
                        patch.push('+');
                        patch.push_str(&line.text);
                        patch.push('\n');
                    }
                    ReviewLineKind::Deletion => {
                        patch.push('-');
                        patch.push_str(&line.text);
                        patch.push('\n');
                    }
                    ReviewLineKind::Context => {
                        patch.push(' ');
                        patch.push_str(&line.text);
                        patch.push('\n');
                    }
                    ReviewLineKind::NoNewlineMarker => {
                        patch.push_str("\\ No newline at end of file\n");
                    }
                }
            }
            delta += i64::from(hunk.new_range.count) - i64::from(hunk.old_range.count);
        }
    }
    (patch, skipped)
}

fn file_header(file: &ReviewFile) -> String {
    let old = file.old_path.as_ref().map(|path| path.to_string_lossy().into_owned());
    let new = file.new_path.as_ref().map(|path| path.to_string_lossy().into_owned());
    let (left, right) = match (&old, &new) {
        (Some(old), Some(new)) => (format!("a/{old}"), format!("b/{new}")),
        (Some(old), None) => (format!("a/{old}"), "/dev/null".to_owned()),
        (None, Some(new)) => ("/dev/null".to_owned(), format!("b/{new}")),
        (None, None) => ("/dev/null".to_owned(), "/dev/null".to_owned()),
    };
    let label = new.clone().or(old.clone()).unwrap_or_else(|| "unknown".into());
    // Git reads `/dev/null` as "no file" only after a mode line says so; without one, an addition
    // is refused as `dev/null: does not exist in index` and a deletion stages a file by that name
    // NOTE: the review keeps no file mode, so an added executable is staged without its bit
    let mode = match (&old, &new) {
        (None, Some(_)) => "new file mode 100644\n",
        (Some(_), None) => "deleted file mode 100644\n",
        _ => "",
    };
    format!("diff --git a/{label} b/{label}\n{mode}--- {left}\n+++ {right}\n")
}

fn hunk_header(hunk: &super::ReviewHunk, new_start: u32) -> String {
    let heading = hunk.heading.as_deref().map(|heading| format!(" {heading}")).unwrap_or_default();
    format!(
        "@@ -{},{} +{},{} @@{heading}\n",
        hunk.old_range.start, hunk.old_range.count, new_start, hunk.new_range.count
    )
}

fn display_path(file: &ReviewFile) -> String {
    file.new_path
        .as_deref()
        .or(file.old_path.as_deref())
        .map_or_else(|| "(unknown path)".to_owned(), |path| path.display().to_string())
}

fn item_key(item: &ReviewItemRef) -> &str {
    match item {
        ReviewItemRef::Hunk(id) => id.as_str(),
        ReviewItemRef::File(id) => id.as_str(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guided_review::{
        fallback_chapters, parse_unified_diff, CheckSummary, ReviewBase, ReviewContextDraft,
        ReviewId, ReviewRevisionId, ReviewSource, ReviewStats, ReviewTarget,
    };

    fn review_from(diff: &str) -> GuidedReview {
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
            context: ReviewContextDraft {
                title: "Shared notification model".into(),
                ..Default::default()
            }
            .build(),
            chapters: fallback_chapters(&files).expect("chapters cover the source"),
            stats: ReviewStats::from_files(&files),
            files,
            checks: CheckSummary::default(),
            generated_at_ms: 0,
            insight: None,
            provenance: None,
            seen: None,
            brief: None,
        }
    }

    const TWO_FILES: &str = "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,1 +1,2 @@\n let a = 1;\n+let b = 2;\n@@ -10,1 +11,1 @@\n-let c = 3;\n+let c = 4;\ndiff --git a/tests/lib.rs b/tests/lib.rs\n--- a/tests/lib.rs\n+++ b/tests/lib.rs\n@@ -1,1 +1,1 @@\n-assert!(false);\n+assert!(true);\n";

    #[test]
    fn each_chapter_becomes_a_commit_carrying_its_reasoning() {
        let review = review_from(TWO_FILES);

        let commits = chapter_commits(&review, &[]).expect("chapters commit");

        assert_eq!(commits.len(), 2, "core and tests are separate commits");
        assert_eq!(commits[0].subject, "Core implementation");
        // The rationale is exactly what a diff cannot say, so it becomes the commit body.
        assert!(commits[0].message.contains("Groups source-backed implementation changes."));
        assert!(commits[0].message.contains("Luxe-review: review:"));
        assert!(commits[0].message.contains("Luxe-revision: revision:"));
        assert!(commits[0].patch.contains("--- a/src/lib.rs"));
        assert!(!commits[0].patch.contains("tests/lib.rs"), "chapters do not leak into each other");
        assert!(commits[1].patch.contains("--- a/tests/lib.rs"));
    }

    #[test]
    fn a_partial_file_recomputes_new_side_offsets() {
        let review = review_from(TWO_FILES);
        // Keep only the *second* hunk of src/lib.rs, so the first one's +1 line is not applied.
        let core = review.chapters[0].clone();
        let second_hunk = review.files[0].hunks[1].id.clone();
        let mut partial = core;
        partial.items = vec![ReviewItemRef::Hunk(second_hunk)];
        let mut review = review;
        review.chapters[0] = partial;

        let commits = chapter_commits(&review, &[review.chapters[0].id.clone()]).unwrap();

        // Without the earlier addition the new side starts where the old side does, not at 11.
        assert!(
            commits[0].patch.contains("@@ -10,1 +10,1 @@"),
            "offsets follow the subset actually applied: {}",
            commits[0].patch
        );
    }

    #[test]
    fn a_squashed_commit_summarizes_every_chapter() {
        let review = review_from(TWO_FILES);

        let commit = squashed_commit(&review, &[]).expect("squash");

        assert_eq!(commit.subject, "Shared notification model");
        assert!(commit.message.contains("- Core implementation"));
        assert!(commit.message.contains("- Tests and fixtures"));
        assert!(commit.patch.contains("src/lib.rs") && commit.patch.contains("tests/lib.rs"));
    }

    #[test]
    fn non_text_items_are_named_rather_than_dropped() {
        let mut review = review_from(TWO_FILES);
        let binary = crate::guided_review::ReviewFile {
            id: crate::guided_review::FileId::from_path_lineage(
                None,
                Some(std::path::Path::new("assets/logo.png")),
            ),
            old_path: None,
            new_path: Some("assets/logo.png".into()),
            status: crate::guided_review::ReviewFileStatus::Added,
            language: None,
            additions: None,
            deletions: None,
            hunks: Vec::new(),
            binary: true,
            generated: false,
            non_text: Some(crate::guided_review::NonTextChange {
                kind: crate::guided_review::NonTextChangeKind::Binary,
                summary: "Binary files differ".into(),
            }),
        };
        review.chapters[0].items.push(ReviewItemRef::File(binary.id.clone()));
        review.files.push(binary);

        let commits = chapter_commits(&review, &[review.chapters[0].id.clone()]).unwrap();

        assert_eq!(commits[0].skipped, ["assets/logo.png"]);
    }

    #[test]
    fn a_chapter_with_nothing_committable_reports_it() {
        let mut review = review_from(TWO_FILES);
        review.chapters[0].items.clear();

        assert!(matches!(
            chapter_commits(&review, &[review.chapters[0].id.clone()]),
            Err(ChapterCommitError::NonTextOnly { .. })
        ));
    }

    #[test]
    fn a_long_chapter_title_is_elided_into_a_subject() {
        let subject = bounded_subject(&"x".repeat(200));
        assert_eq!(subject.chars().count(), MAX_COMMIT_SUBJECT_CHARS);
        assert!(subject.ends_with('…'));
    }
}
