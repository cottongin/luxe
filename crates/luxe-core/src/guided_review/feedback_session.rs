//! Review feedback session — persisted user findings for one logical review.
//!
//! This model references the immutable source artifact through [`ReviewSource`]. It stores
//! user review state and feedback history without copying canonical diff bytes.

use super::{
    ChapterId, FileId, FindingId, HunkId, RemediationReport, RemediationStatus, ReviewChapter,
    ReviewFile, ReviewId, ReviewRevisionId, ReviewSource, ReviewTarget, SubmissionId,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use thiserror::Error;

mod anchors;
mod answers;
mod finding;
mod findings;
mod fix_request;
mod submissions;

pub use anchors::*;
pub use finding::*;
pub use submissions::*;

/// Schema version for persisted review feedback sessions.
pub const REVIEW_SESSION_SCHEMA_VERSION: u32 = 1;

/// Project-local review state retained across source refreshes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewSession {
    /// Schema used to decode this session.
    pub schema_version: u32,
    /// Optimistic revision incremented after each persisted mutation.
    pub revision: u64,
    /// Logical identity retained across immutable source refreshes.
    pub id: ReviewId,
    /// Comparison target used to load the active immutable source.
    pub target: ReviewTarget,
    /// Provenance for the current immutable source revision.
    pub source: ReviewSource,
    /// User review progress for the active source.
    pub progress: ReviewProgress,
    /// Mutable user findings and their threads.
    pub findings: Vec<ReviewFinding>,
    /// Immutable feedback snapshots submitted to the agent.
    pub submissions: Vec<ReviewSubmission>,
    /// Explicit local review outcome, if the user has finished the review.
    pub outcome: Option<ReviewOutcome>,
    /// Creation time in milliseconds since Unix epoch.
    pub created_at_ms: u64,
    /// Last successful mutation time in milliseconds since Unix epoch.
    pub updated_at_ms: u64,
}

impl ReviewSession {
    /// Creates an empty session for one pinned source revision.
    pub fn new(id: ReviewId, target: ReviewTarget, source: ReviewSource, now_ms: u64) -> Self {
        Self {
            schema_version: REVIEW_SESSION_SCHEMA_VERSION,
            revision: 0,
            id,
            target,
            source,
            progress: ReviewProgress::default(),
            findings: Vec::new(),
            submissions: Vec::new(),
            outcome: None,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        }
    }

    /// Rejects persisted state that cannot safely represent one logical review.
    pub fn validate(&self) -> Result<(), ReviewSessionValidationError> {
        if self.schema_version != REVIEW_SESSION_SCHEMA_VERSION {
            return Err(ReviewSessionValidationError::UnsupportedSchema {
                schema_version: self.schema_version,
            });
        }
        ensure_unique(self.findings.iter().map(|finding| &finding.id), "finding")?;
        ensure_unique(self.submissions.iter().map(|submission| &submission.id), "submission")?;
        for finding in &self.findings {
            finding.validate()?;
        }
        for submission in &self.submissions {
            submission.validate()?;
        }
        Ok(())
    }
}

/// What a reviewer has been through, for the active pinned source.
///
/// Deliberately *not* a checklist. Ticking off hunks made review an inspection task with a target no
/// reviewer can meet on agent-scale changes; what actually needs remembering is which sections have
/// been walked (so a resumed review reopens where the reading stopped) and which items were on screen
/// when a review was finished (so the next one can lead with what is new).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReviewProgress {
    /// Chapters the reviewer has walked through.
    pub walked_chapter_ids: Vec<ChapterId>,
    /// Item IDs present when this review was last finished, for "new since your last review".
    pub seen_item_ids: Vec<String>,
    /// Revision those seen items came from.
    pub seen_revision: Option<super::ReviewRevisionId>,
}

/// Errors raised when durable feedback state is internally inconsistent.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ReviewSessionValidationError {
    /// The session requires a newer or incompatible schema.
    #[error("review session uses unsupported schema {schema_version}")]
    UnsupportedSchema {
        /// Schema version encountered in persisted state.
        schema_version: u32,
    },
    /// A finding or submission identity appeared more than once.
    #[error("review session has duplicate {kind} IDs")]
    DuplicateId {
        /// Duplicated record category.
        kind: &'static str,
    },
    /// User feedback needs a concise summary.
    #[error("review finding title cannot be empty")]
    EmptyFindingTitle,
    /// User feedback needs an actionable explanation.
    #[error("review finding body cannot be empty")]
    EmptyFindingBody,
    /// A line anchor used an impossible zero source line.
    #[error("review finding line anchor must use a one-based line number")]
    InvalidLineAnchor,
    /// A line anchor cannot be remapped safely without its context digest.
    #[error("review finding line anchor is missing its context hash")]
    MissingContextHash,
}

/// Errors raised while mutating an in-memory review session before it is persisted.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ReviewSessionMutationError {
    /// The requested finding is absent from this session.
    #[error("review finding does not exist")]
    FindingNotFound,
    /// A reply with no text says nothing.
    #[error("review reply cannot be empty")]
    EmptyReply,
    /// Adding a second finding with the same durable ID would make refresh ambiguous.
    #[error("review session already contains this finding ID")]
    DuplicateFindingId,
    /// Only an unsubmitted draft can be deleted without losing audit history.
    #[error("review finding is not a draft: {state:?}")]
    FindingNotDraft {
        /// Current lifecycle state preventing deletion.
        state: FindingState,
    },
    /// A submission already owns an immutable copy of this finding.
    #[error("review finding is retained by immutable submission history")]
    FindingAlreadySubmitted,
    /// Reanchoring is reserved for an explicitly stale finding.
    #[error("review finding is not stale: {state:?}")]
    FindingNotStale {
        /// Current lifecycle state preventing a stale-anchor correction.
        state: FindingState,
    },
    /// A review submission requires at least one selected finding.
    #[error("review submission has no selected findings")]
    EmptySubmission,
    /// A submission ID can only describe one durable agent dispatch.
    #[error("review submission ID already exists")]
    DuplicateSubmissionId,
    /// The requested durable feedback submission is absent from this session.
    #[error("review submission does not exist")]
    SubmissionNotFound,
    /// Only queued feedback may be cancelled before it reaches the agent loop.
    #[error("review submission is not queued: {state:?}")]
    SubmissionNotQueued {
        /// Current lifecycle state preventing cancellation
        state: SubmissionDispatch,
    },
    /// Only queued or retryable feedback may be accepted at a turn boundary.
    #[error("review submission cannot be dispatched from {state:?}")]
    SubmissionNotDispatchable {
        /// Current lifecycle state preventing dispatch
        state: SubmissionDispatch,
    },
    /// Only an active agent turn may fail or complete a feedback submission.
    #[error("review submission is not dispatched: {state:?}")]
    SubmissionNotDispatched {
        /// Current lifecycle state preventing this update
        state: SubmissionDispatch,
    },
    /// An agent report must belong to the same logical review.
    #[error("remediation report belongs to a different review")]
    RemediationReportWrongReview,
    /// An agent report may only annotate findings submitted at its pinned revision.
    #[error("remediation report does not match a submitted feedback snapshot")]
    RemediationReportNotSubmitted,
    /// The finding content or anchor is unsafe to retain.
    #[error(transparent)]
    Validation(#[from] ReviewSessionValidationError),
}

fn ensure_unique<'a, T>(
    mut values: impl Iterator<Item = &'a T>,
    kind: &'static str,
) -> Result<(), ReviewSessionValidationError>
where
    T: Eq + std::hash::Hash + 'a,
{
    let mut seen = HashSet::new();
    if values.all(|value| seen.insert(value)) {
        Ok(())
    } else {
        Err(ReviewSessionValidationError::DuplicateId { kind })
    }
}

#[cfg(test)]
mod tests;
