//! Guided Review surface value types — enums, drafts, and the triage ordering.
//!
//! Pure presentation state referenced by the workspace, view, and diff layers; no behavior
//! that touches the pinned review lives here.

mod finding_draft;
mod opening;
mod outcomes;
mod post;

pub(crate) use finding_draft::FindingDraft;
use luxe_core::event::GuidedReviewFailure;
use luxe_core::guided_review::{
    ChapterId, FileId, FindingSeverity, FindingState, HunkId, ReviewFinding, ReviewTarget,
};
pub use opening::OpeningField;
pub(crate) use opening::{
    AnswerRow, PreparedOpening, PullRequestFix, PullRequestOpening, StackOpening, ThreadAnswers,
};
pub(crate) use outcomes::{CommitPreview, FinishReviewChoice};
pub(crate) use post::{GithubPost, PreparedPost, SendPreview};

/// Returns findings in the stable triage order used by the inspector.
pub(crate) fn ordered_findings(findings: &[ReviewFinding]) -> Vec<&ReviewFinding> {
    let mut ordered = findings.iter().collect::<Vec<_>>();
    ordered.sort_by(|left, right| {
        finding_state_rank(left)
            .cmp(&finding_state_rank(right))
            .then_with(|| right.severity.cmp(&left.severity))
            .then_with(|| left.title.cmp(&right.title))
            .then_with(|| left.id.as_str().cmp(right.id.as_str()))
    });
    ordered
}

fn finding_state_rank(finding: &ReviewFinding) -> u8 {
    match (finding.state, finding.severity) {
        (FindingState::Open, FindingSeverity::Blocking) => 0,
        (FindingState::Open, _) => 1,
        (FindingState::NeedsVerification, _) => 2,
        (FindingState::Draft, _) => 3,
        (FindingState::Resolved, _) => 4,
        (FindingState::Stale, _) => 5,
    }
}

/// Runtime state for the full-screen Guided Review surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GuidedReviewLoadState {
    /// The surface has not requested a review yet.
    Idle,
    /// A target is being captured while any pinned revision remains available.
    Loading { target: ReviewTarget },
    /// The current pinned review is ready to present.
    Ready,
    /// The requested target failed without discarding an existing pinned revision.
    Failed { target: ReviewTarget, error: GuidedReviewFailure },
}

/// Selectable source kinds in the local review-source picker.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum ReviewSourcePickerSelection {
    /// Capture the current working tree.
    #[default]
    WorkingTree,
    /// Compare the most recently completed agent turn.
    LastTurn,
    /// Prompt for a merge-base reference.
    Base,
    /// Prompt for a commit object identifier.
    Commit,
}

impl ReviewSourcePickerSelection {
    /// Returns source kinds in the picker presentation order.
    pub(crate) const fn picker_order() -> [Self; 4] {
        [Self::WorkingTree, Self::LastTurn, Self::Base, Self::Commit]
    }
}

/// Reference kinds that need typed input before a review can load.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReviewSourceReferenceKind {
    /// A merge-base reference.
    Base,
    /// A commit object identifier.
    Commit,
}

/// Local in-progress reference input for the review-source picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReviewSourceReferenceEntry {
    /// Reference kind that will receive the text.
    pub(crate) kind: ReviewSourceReferenceKind,
    /// Unvalidated user input.
    pub(crate) value: String,
    /// Validation message for the current input.
    pub(crate) error: Option<&'static str>,
}

/// Local visibility and selection state for the review-source picker.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum ReviewSourcePickerState {
    /// The picker is not shown.
    #[default]
    Closed,
    /// The picker is open with one source kind selected.
    Open { selection: ReviewSourcePickerSelection },
}

/// Stable semantic location inside a pinned review source.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ReviewPosition {
    /// Selected semantic chapter, if the source provides one.
    pub(crate) chapter_id: Option<ChapterId>,
    /// Selected file lineage, if the review contains text hunks.
    pub(crate) file_id: Option<FileId>,
    /// Selected hunk within the selected file.
    pub(crate) hunk_id: Option<HunkId>,
    /// First visible unified-diff row within the selected file.
    pub(crate) line_offset: usize,
}

impl ReviewPosition {
    /// A cheap value identifying where the cursor is, for comparing against a remembered one.
    pub(crate) fn fingerprint(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.chapter_id.as_ref().map(|id| id.as_str()).hash(&mut hasher);
        self.file_id.as_ref().map(|id| id.as_str()).hash(&mut hasher);
        self.hunk_id.as_ref().map(|id| id.as_str()).hash(&mut hasher);
        hasher.finish()
    }
}
