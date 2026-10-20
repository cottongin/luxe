//! Aggregate Guided Review model and source provenance.
//!
//! Source provenance describes exactly what was captured; chapters only reference immutable items
//! from that capture. This separates presentation order from source truth.

use super::{
    fallback_chapters, parse_unified_diff, ChapterCoverageError, FileId, GuidedReviewParseError,
    HunkId, ReviewContext, ReviewFile, ReviewId, ReviewRevisionId, ReviewSourceArtifact,
    ReviewTarget,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Immutable provenance for one pinned review source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewSource {
    /// Base used for the captured comparison.
    pub base: ReviewBase,
    /// Digest of the complete canonical source artifact.
    pub content_digest: ReviewRevisionId,
    /// Whether the captured working tree had staged changes.
    pub includes_staged: bool,
    /// Whether the captured working tree had unstaged changes.
    pub includes_unstaged: bool,
    /// Number of eligible untracked files included in the source.
    pub included_untracked_files: u64,
    /// Capture timestamp in milliseconds since Unix epoch.
    pub captured_at_ms: u64,
    /// Changed files the capture could not include — too large, unreadable, or not a regular file
    /// — so the review can say what it did not cover rather than look complete. A file left out
    /// that the change did not touch is not listed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub uncovered_paths: Vec<std::path::PathBuf>,
}

/// Exact base semantics for a review source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ReviewBase {
    /// A resolved `HEAD` tree.
    Head {
        /// Resolved `HEAD` object identifier.
        oid: String,
    },
    /// The empty tree used when a repository or commit has no parent tree.
    EmptyTree,
    /// Local merge base resolved for a requested reference.
    MergeBase {
        /// User-requested local reference.
        reference: String,
        /// Resolved merge-base object identifier.
        oid: String,
    },
    /// First-parent comparison for a selected commit.
    CommitParent {
        /// Reviewed commit object identifier.
        commit_oid: String,
        /// First parent, absent for a root commit.
        parent_oid: Option<String>,
    },
    /// Complete pre-turn baseline retained by luxe.
    TurnBaseline {
        /// Immutable baseline revision identifier.
        revision_id: ReviewRevisionId,
    },
    /// A pull request, pinned to the commits GitHub reported when it was read.
    PullRequest {
        /// Number within its repository.
        number: u64,
        /// `owner/name` of the repository the pull request is on.
        repository: String,
        /// Branch it targets.
        base_ref: String,
        /// Branch it comes from.
        head_ref: String,
        /// Where the head branched off the base: the left side of the comparison.
        merge_base: String,
        /// The head commit: the right side of the comparison.
        head_oid: String,
    },
}

/// An immutable source item that can appear in exactly one chapter.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "id")]
pub enum ReviewItemRef {
    /// Text hunk with complete source lines.
    Hunk(HunkId),
    /// Non-text file item with no line-level diff.
    File(FileId),
}

/// Presentation priority for a semantic Guide chapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChapterKind {
    /// Primary implementation or contract change.
    Core,
    /// Direct integration or downstream effect.
    Consequence,
    /// Tests or fixtures that verify the change.
    Test,
    /// Documentation, configuration, or supporting glue.
    Supporting,
    /// Generated, lockfile, or otherwise lower-signal content.
    Generated,
}

/// A lossless semantic presentation section for immutable review items.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewChapter {
    /// Stable identity derived from the ordered referenced items.
    pub id: super::ChapterId,
    /// User-facing concise section title.
    pub title: String,
    /// Grounded explanation of why these items change together.
    pub rationale: String,
    /// Optional grounded product or integration impact.
    pub impact: Option<String>,
    /// Presentation priority for this section.
    pub kind: ChapterKind,
    /// Immutable source items, each owned by exactly one chapter.
    pub items: Vec<ReviewItemRef>,
    /// One sentence per item, keyed by item id: what this piece does and why it is here.
    ///
    /// The narration a reviewer reads instead of reconstructing intent from the diff. Advisory, so a
    /// structural fallback simply carries none.
    #[serde(default)]
    pub notes: Vec<ItemNote>,
}

/// A sentence about one immutable review item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemNote {
    /// Item id (hunk or non-text file) this note describes.
    pub item: String,
    /// The sentence itself, bounded and control-filtered.
    pub note: String,
}

impl ReviewChapter {
    /// The note for one item, when the guide wrote one.
    pub fn note_for(&self, item: &ReviewItemRef) -> Option<&str> {
        let key = match item {
            ReviewItemRef::Hunk(id) => id.as_str(),
            ReviewItemRef::File(id) => id.as_str(),
        };
        self.notes.iter().find(|note| note.item == key).map(|note| note.note.as_str())
    }

    /// The note for one hunk id.
    pub fn note_for_hunk(&self, hunk: &super::HunkId) -> Option<&str> {
        self.notes.iter().find(|note| note.item == hunk.as_str()).map(|note| note.note.as_str())
    }
}

/// A review-gate or acceptance-check result paired with the same review source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewCheck {
    /// Stable check label or command name.
    pub name: String,
    /// Latest known outcome for this review revision.
    pub status: ReviewCheckStatus,
    /// Bounded result detail when available.
    pub summary: Option<String>,
}

/// State of an acceptance or review check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewCheckStatus {
    /// Check has passed.
    Passed,
    /// Check has failed.
    Failed,
    /// Check has not completed yet.
    Pending,
    /// Check cannot run for this source.
    Unavailable,
}

/// Check results attached to one immutable review revision.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckSummary {
    /// Individual check outcomes in stable display order.
    pub checks: Vec<ReviewCheck>,
}

/// Aggregate counts for one review source.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewStats {
    /// Total review files, including non-text files.
    pub files: u64,
    /// Total text hunks.
    pub hunks: u64,
    /// Text additions, or `None` when totals are unknown.
    pub additions: Option<u64>,
    /// Text deletions, or `None` when totals are unknown.
    pub deletions: Option<u64>,
    /// Number of binary files.
    pub binary_files: u64,
    /// Number of explicit non-text file items.
    pub non_text_files: u64,
    /// Files left out of the review by `review.exclude` or as luxe's own artifacts.
    ///
    /// Counted, never silent: a review that shows less than the diff has to say so.
    #[serde(default)]
    pub excluded_files: u64,
}

/// A complete, pinned Guided Review ready for lossless presentation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuidedReview {
    /// Logical review identity retained across refreshes.
    pub id: ReviewId,
    /// Immutable captured source revision.
    pub revision_id: ReviewRevisionId,
    /// Requested comparison target.
    pub target: ReviewTarget,
    /// Explainable immutable source provenance.
    pub source: ReviewSource,
    /// Bounded task and phase context.
    pub context: ReviewContext,
    /// Complete immutable source files in canonical order.
    pub files: Vec<ReviewFile>,
    /// Semantic presentation sections that reference every source item once.
    pub chapters: Vec<ReviewChapter>,
    /// Acceptance and reviewer checks for this revision.
    pub checks: CheckSummary,
    /// Complete source totals.
    pub stats: ReviewStats,
    /// Generation timestamp in milliseconds since Unix epoch.
    pub generated_at_ms: u64,
    /// Advisory graph-derived structural grounding (relevant files + test gaps). NOT part of the
    /// chapter/source-item bijection; recomputed on each (re)build, absent until a graph provides it.
    #[serde(default)]
    pub insight: Option<super::ReviewInsight>,
    /// What produced this change: the request, the agent's account, the commands it ran, and the
    /// files it edited without reading. Advisory like `insight`, and likewise outside the
    /// chapter/source-item bijection.
    #[serde(default)]
    pub provenance: Option<super::TurnProvenance>,
    /// What the previous review of this target left behind, so this one can lead with what is new.
    #[serde(default)]
    pub seen: Option<super::SeenReview>,
    /// The model's summary of the change, written for the reviewer.
    ///
    /// Produced by the guide pass that already runs, so it costs no extra request. Absent when
    /// that pass degraded, in which case the Brief falls back to the raw record of the turn.
    #[serde(default)]
    pub brief: Option<ReviewBrief>,
    /// What reviewers already said, for a pull request.
    ///
    /// Kept here and not in [`GuidedReview::context`], which is serialized into the guide's
    /// prompt: the guide should not argue with a reviewer on the reader's behalf.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation: Option<super::EarlierConversation>,
}

/// A change summarized for whoever is about to review it.
///
/// The same pass's answer to the two questions a reviewer actually arrives with: what was this for,
/// and what does it do. The agent's closing message is not it — the reviewer dismissed that a
/// moment earlier to *open* the review.
///
/// Deliberately no verification field. A model's account of its own evidence is the self-report
/// this subsystem exists to distrust, and the harness holds the fact that matters — whether
/// anything ran after the final edit — as a fact rather than a claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewBrief {
    /// The problem the change sets out to solve.
    pub intent: String,
    /// What it does — claims a reviewer can check against the diff.
    #[serde(default)]
    pub account: Vec<String>,
}

/// Prevents an aggregate review from claiming a source different from its pinned artifact.
#[derive(Debug, Error)]
pub enum GuidedReviewBuildError {
    /// The caller's source provenance did not identify the pinned artifact revision.
    #[error("review source revision does not match its pinned artifact")]
    SourceRevisionMismatch,
    /// A canonical source artifact could not be decoded as UTF-8 unified diff text.
    #[error("review source artifact is not valid UTF-8")]
    InvalidUtf8,
    /// The pinned canonical diff could not be parsed completely.
    #[error(transparent)]
    Parse(#[from] GuidedReviewParseError),
    /// An internal fallback could not preserve the parsed immutable source items.
    #[error(transparent)]
    Fallback(#[from] ChapterCoverageError),
}

impl GuidedReview {
    /// Builds a complete review only from a verified pinned source artifact and supplied metadata.
    pub fn from_pinned_artifact(
        id: ReviewId,
        target: ReviewTarget,
        source: ReviewSource,
        context: ReviewContext,
        artifact: &ReviewSourceArtifact,
        exclusions: &super::ReviewExclusions,
        generated_at_ms: u64,
    ) -> Result<Self, GuidedReviewBuildError> {
        if source.content_digest != artifact.manifest.revision_id {
            return Err(GuidedReviewBuildError::SourceRevisionMismatch);
        }
        let canonical_source = std::str::from_utf8(&artifact.canonical_bytes)
            .map_err(|_| GuidedReviewBuildError::InvalidUtf8)?;
        // The pinned source stays whole on disk; what is excluded is only what the review
        // *presents*, so the revision a reviewer signs off on is still the complete diff.
        let parsed = parse_unified_diff(canonical_source)?.files;
        let before = parsed.len();
        let files: Vec<ReviewFile> = parsed
            .into_iter()
            .filter(|file| {
                !file
                    .new_path
                    .as_deref()
                    .or(file.old_path.as_deref())
                    .map(|path| path.display().to_string())
                    .is_some_and(|path| exclusions.excludes(&path))
            })
            .collect();
        let excluded_files = (before - files.len()) as u64;
        let chapters = fallback_chapters(&files)?;
        let stats = ReviewStats { excluded_files, ..ReviewStats::from_files(&files) };
        Ok(Self {
            conversation: None,
            id,
            revision_id: artifact.manifest.revision_id.clone(),
            target,
            source,
            context,
            files,
            chapters,
            checks: CheckSummary::default(),
            stats,
            generated_at_ms,
            insight: None,
            provenance: None,
            seen: None,
            brief: None,
        })
    }
}

impl ReviewStats {
    /// Computes exact review totals without inventing text counts for non-text source records.
    pub fn from_files(files: &[ReviewFile]) -> Self {
        // Text totals over the files that *have* text: one binary in the change must not make the
        // whole review's line counts unknown. Binaries are reported as `non_text_files`.
        let counted = |count: fn(&ReviewFile) -> Option<u64>| {
            let mut known = false;
            let total = files.iter().filter_map(count).inspect(|_| known = true).sum::<u64>();
            known.then_some(total)
        };
        let additions = counted(|file| file.additions);
        let deletions = counted(|file| file.deletions);
        Self {
            files: files.len() as u64,
            hunks: files.iter().map(|file| file.hunks.len() as u64).sum(),
            additions,
            deletions,
            binary_files: files.iter().filter(|file| file.binary).count() as u64,
            non_text_files: files.iter().filter(|file| file.non_text.is_some()).count() as u64,
            excluded_files: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_source_preserves_unborn_and_first_parent_semantics() {
        let revision = ReviewRevisionId::from_canonical_bytes(b"source");
        let source = ReviewSource {
            base: ReviewBase::EmptyTree,
            content_digest: revision,
            includes_staged: false,
            includes_unstaged: true,
            included_untracked_files: 2,
            captured_at_ms: 7,
            uncovered_paths: Vec::new(),
        };

        let json = serde_json::to_string(&source).unwrap();
        assert_eq!(serde_json::from_str::<ReviewSource>(&json).unwrap(), source);
        assert!(matches!(source.base, ReviewBase::EmptyTree));
        assert!(!json.contains("uncovered_paths"), "a complete source keeps its old shape: {json}");
    }

    /// The field name a saved record and a front-end read, pinned: a rename would load as empty.
    #[test]
    fn uncovered_paths_read_from_their_saved_spelling() {
        let digest = ReviewRevisionId::from_canonical_bytes(b"source");
        let saved = format!(
            r#"{{"base":{{"kind":"empty_tree"}},"content_digest":{},"includes_staged":false,"includes_unstaged":true,"included_untracked_files":0,"captured_at_ms":7,"uncovered_paths":["archive.md"]}}"#,
            serde_json::to_string(&digest).unwrap()
        );
        let source: ReviewSource = serde_json::from_str(&saved).unwrap();
        assert_eq!(source.uncovered_paths, vec![std::path::PathBuf::from("archive.md")]);
    }

    #[test]
    fn non_text_items_are_first_class_chapter_references() {
        let item = ReviewItemRef::File(FileId::from_path_lineage(None, Some("asset.bin".as_ref())));
        let json = serde_json::to_string(&item).unwrap();
        assert_eq!(serde_json::from_str::<ReviewItemRef>(&json).unwrap(), item);
    }

    #[test]
    fn pinned_artifact_builds_a_complete_review_without_worktree_access() {
        let artifact = ReviewSourceArtifact::capture(
            include_bytes!("../../tests/fixtures/guided_review/multi_file_semantic.diff").to_vec(),
        );
        let source = ReviewSource {
            base: ReviewBase::Head { oid: "0123456789abcdef0123456789abcdef01234567".into() },
            content_digest: artifact.manifest.revision_id.clone(),
            includes_staged: false,
            includes_unstaged: true,
            included_untracked_files: 0,
            captured_at_ms: 3,
            uncovered_paths: Vec::new(),
        };
        let context = ReviewContext {
            title: "Review".into(),
            goal: None,
            phase_index: Some(30),
            phase_title: Some("Guided review".into()),
            planning_excerpt: None,
            acceptance_excerpt: None,
            source_intent: None,
            source_commits: 0,
            pull_request: None,
        };

        let Ok(review) = GuidedReview::from_pinned_artifact(
            ReviewId::new(),
            ReviewTarget::WorkingTree,
            source,
            context,
            &artifact,
            &crate::guided_review::ReviewExclusions::default(),
            4,
        ) else {
            panic!("complete semantic fixture must build a pinned review");
        };

        assert_eq!(review.revision_id, artifact.manifest.revision_id);
        assert_eq!(review.stats.files, review.files.len() as u64);
        assert_eq!(review.stats.hunks, 2);
        assert_eq!(
            super::super::validate_chapter_coverage(&review.files, &review.chapters),
            Ok(())
        );
    }
}
