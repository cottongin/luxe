//! Review session record — restart-safe metadata for pinned reviews.
//!
//! The record stores presentation and provenance metadata only. Canonical diff bytes remain in the
//! content-addressed source cache, so reopening a review never consults a changed workspace.

use crate::fs_atomic;

use super::{
    validate_chapter_coverage, CheckSummary, GuidedReview, GuidedReviewBuildError, ReviewChapter,
    ReviewContext, ReviewId, ReviewRevisionId, ReviewSource, ReviewSourceArtifact, ReviewTarget,
    SourceStoreError,
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};
use thiserror::Error;

/// Schema version for persisted active-review records.
pub const ACTIVE_REVIEW_RECORD_SCHEMA_VERSION: u32 = 1;
/// Project-relative location of the active review metadata record.
pub const ACTIVE_REVIEW_RECORD_RELATIVE_PATH: &str = ".luxe/reviews/active-review.json";

/// Restart-safe metadata for an open review revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveReviewRecord {
    /// Schema used to decode this record.
    pub schema_version: u32,
    /// Logical review identity retained across refreshes.
    pub review_id: ReviewId,
    /// Immutable source artifact to reopen.
    pub revision_id: ReviewRevisionId,
    /// Requested comparison target.
    pub target: ReviewTarget,
    /// Explainable source provenance.
    pub source: ReviewSource,
    /// Bounded project and task context.
    pub context: ReviewContext,
    /// Lossless presentation metadata, without duplicate source lines.
    pub chapters: Vec<ReviewChapter>,
    /// Review-gate evidence for this revision.
    pub checks: CheckSummary,
    /// Time at which the review presentation was generated.
    pub generated_at_ms: u64,
    /// The reviewer-facing summary, so reopening a review does not silently lose it.
    #[serde(default)]
    pub brief: Option<super::ReviewBrief>,
}

impl ActiveReviewRecord {
    /// Captures the metadata needed to reopen a review from its pinned source artifact.
    pub fn from_review(review: &GuidedReview) -> Self {
        Self {
            schema_version: ACTIVE_REVIEW_RECORD_SCHEMA_VERSION,
            review_id: review.id.clone(),
            revision_id: review.revision_id.clone(),
            target: review.target.clone(),
            source: review.source.clone(),
            context: review.context.clone(),
            chapters: review.chapters.clone(),
            checks: review.checks.clone(),
            generated_at_ms: review.generated_at_ms,
            brief: review.brief.clone(),
        }
    }

    /// Rejects incompatible records and source references before reopening a pinned artifact.
    pub fn validate(&self) -> Result<(), ActiveReviewRecordError> {
        if self.schema_version != ACTIVE_REVIEW_RECORD_SCHEMA_VERSION {
            return Err(ActiveReviewRecordError::UnsupportedSchema {
                schema_version: self.schema_version,
            });
        }
        if self.revision_id != self.source.content_digest {
            return Err(ActiveReviewRecordError::SourceRevisionMismatch);
        }
        Ok(())
    }

    /// Rebuilds the review from its immutable artifact without consulting the live workspace.
    pub fn restore(
        &self,
        project_root: &Path,
        exclusions: &super::ReviewExclusions,
    ) -> Result<GuidedReview, ActiveReviewRestoreError> {
        self.validate()?;
        let artifact = ReviewSourceArtifact::load(project_root, &self.revision_id)?;
        let mut review = GuidedReview::from_pinned_artifact(
            self.review_id.clone(),
            self.target.clone(),
            self.source.clone(),
            self.context.clone(),
            &artifact,
            exclusions,
            self.generated_at_ms,
        )?;
        review.checks.clone_from(&self.checks);
        // the brief names no items, so it holds whichever chapters the review ends up with
        review.brief.clone_from(&self.brief);
        // saved chapters that name items the source no longer has give way to structural ones
        match validate_chapter_coverage(&review.files, &self.chapters) {
            Ok(()) => review.chapters.clone_from(&self.chapters),
            Err(error) => tracing::warn!(%error, "saved chapters no longer cover the review"),
        }
        Ok(review)
    }
}

/// Errors that prevent an active-review record from being reopened safely.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ActiveReviewRecordError {
    /// The record requires a newer or incompatible schema.
    #[error("active review record uses unsupported schema {schema_version}")]
    UnsupportedSchema {
        /// Schema version encountered in the record.
        schema_version: u32,
    },
    /// The record does not point to the same immutable revision as its source provenance.
    #[error("active review record source revision does not match its pinned revision")]
    SourceRevisionMismatch,
}

/// Errors that prevent a persisted review from being reconstructed from its pinned artifact.
#[derive(Debug, Error)]
pub enum ActiveReviewRestoreError {
    /// The persisted metadata was not safe to use.
    #[error(transparent)]
    Record(#[from] ActiveReviewRecordError),
    /// The immutable source artifact could not be reloaded.
    #[error(transparent)]
    SourceStore(#[from] SourceStoreError),
    /// The pinned source could not be rebuilt into a review model.
    #[error(transparent)]
    Build(#[from] GuidedReviewBuildError),
}

/// Stores one active review pointer without duplicating its immutable source artifact.
#[derive(Debug, Clone)]
pub struct ActiveReviewRecordStore {
    project_root: PathBuf,
}

impl ActiveReviewRecordStore {
    /// Opens the active-review metadata location for one project.
    pub fn new(project_root: impl Into<PathBuf>) -> Self {
        Self { project_root: project_root.into() }
    }

    /// Loads a valid record, returning no record when this project has not opened a review yet.
    pub fn load(&self) -> Result<Option<ActiveReviewRecord>, ActiveReviewRecordStoreError> {
        let path = self.path();
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(ActiveReviewRecordStoreError::Io {
                    operation: "read",
                    path,
                    error: source,
                });
            }
        };
        let record: ActiveReviewRecord = serde_json::from_slice(&bytes).map_err(|source| {
            ActiveReviewRecordStoreError::Deserialize { path: self.path(), error: source }
        })?;
        record.validate()?;
        Ok(Some(record))
    }

    /// Atomically replaces the record after refusing an unknown or invalid existing schema.
    pub fn save(&self, record: &ActiveReviewRecord) -> Result<(), ActiveReviewRecordStoreError> {
        record.validate()?;
        let _ = self.load()?;
        let path = self.path();
        let parent =
            path.parent().ok_or_else(|| ActiveReviewRecordStoreError::InvalidPath(path.clone()))?;
        fs_atomic::create_private_dir(parent).map_err(|source| {
            ActiveReviewRecordStoreError::Io {
                operation: "create directory for",
                path: parent.to_owned(),
                error: source,
            }
        })?;
        let bytes = serde_json::to_vec(record)?;
        fs_atomic::replace_private(&path, &bytes).map_err(|source| {
            ActiveReviewRecordStoreError::Io {
                operation: "atomically replace",
                path,
                error: source,
            }
        })
    }

    fn path(&self) -> PathBuf {
        self.project_root.join(ACTIVE_REVIEW_RECORD_RELATIVE_PATH)
    }
}

/// Errors produced while loading or atomically replacing active-review metadata.
#[derive(Debug, Error)]
pub enum ActiveReviewRecordStoreError {
    /// Metadata filesystem work failed.
    #[error("could not {operation} active review record at {path}: {error}")]
    Io {
        /// Filesystem operation that failed.
        operation: &'static str,
        /// Path involved in the operation.
        path: PathBuf,
        /// Underlying filesystem error.
        error: std::io::Error,
    },
    /// Metadata did not decode as the supported record schema.
    #[error("could not decode active review record at {path}: {error}")]
    Deserialize {
        /// Metadata path.
        path: PathBuf,
        /// Underlying JSON error.
        error: serde_json::Error,
    },
    /// Metadata could not be serialized before replacement.
    #[error("could not serialize active review record: {0}")]
    Serialize(serde_json::Error),
    /// The metadata path lacks a parent directory.
    #[error("active review record path has no parent: {0}")]
    InvalidPath(PathBuf),
    /// The record was unsafe to reopen.
    #[error(transparent)]
    Record(#[from] ActiveReviewRecordError),
    /// Private metadata directory or staging-file setup failed.
    #[error(transparent)]
    SourceStore(#[from] SourceStoreError),
}

crate::error_cause::from_cause!(ActiveReviewRecordStoreError {
    serde_json::Error => Serialize,
});

#[cfg(test)]
mod tests;
