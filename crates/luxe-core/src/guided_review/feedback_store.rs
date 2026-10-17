//! Review feedback store — atomic, optimistic persistence for review feedback sessions.
//!
//! Sessions live beside the active-review pointer but never replace it. Each record names a
//! logical review and points at an immutable source artifact rather than duplicating diff bytes.

use super::{ReviewId, ReviewSession, ReviewSessionValidationError};
use crate::fs_atomic;
use std::{
    fs, io,
    path::{Path, PathBuf},
};
use thiserror::Error;
use uuid::Uuid;

/// Project-relative directory containing review feedback sessions.
pub const REVIEW_SESSION_RELATIVE_DIR: &str = ".luxe/reviews";

/// Atomic project-local persistence for feedback sessions.
#[derive(Debug, Clone)]
pub struct ReviewSessionStore {
    project_root: PathBuf,
}

impl ReviewSessionStore {
    /// Opens the feedback-session directory for one project.
    pub fn new(project_root: impl Into<PathBuf>) -> Self {
        Self { project_root: project_root.into() }
    }

    /// Loads a validated session, returning `None` when this review has no saved feedback yet.
    pub fn load(
        &self,
        review_id: &ReviewId,
    ) -> Result<Option<ReviewSession>, ReviewSessionStoreError> {
        let path = self.path(review_id)?;
        let bytes = match read_regular_file(&path) {
            Ok(bytes) => bytes,
            Err(ReviewSessionStoreError::Io { error: source, .. })
                if source.kind() == io::ErrorKind::NotFound =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        let mut session: ReviewSession = serde_json::from_slice(&bytes)
            .map_err(|source| ReviewSessionStoreError::Deserialize { path, error: source })?;
        session.validate()?;
        // Sessions written before findings carried a stable `R<n>` reference get one now, in review
        // order, so the reviewer and the agent can talk about the same comment.
        session.backfill_finding_labels();
        if session.id != *review_id {
            return Err(ReviewSessionStoreError::ReviewIdMismatch {
                expected: review_id.as_str().to_owned(),
                found: session.id.as_str().to_owned(),
            });
        }
        Ok(Some(session))
    }

    /// Writes a new session or atomically replaces an expected revision.
    ///
    /// A mismatch leaves both the persisted session and the caller's input untouched. The returned
    /// copy includes the assigned revision and timestamp, so callers can use it for their next
    /// expected-revision mutation.
    pub fn save(
        &self,
        expected_revision: Option<u64>,
        session: &ReviewSession,
        updated_at_ms: u64,
    ) -> Result<ReviewSession, ReviewSessionStoreError> {
        session.validate()?;
        let path = self.path(&session.id)?;
        let existing = self.load(&session.id)?;
        let actual_revision = existing.as_ref().map(|existing| existing.revision);
        if actual_revision != expected_revision {
            return Err(ReviewSessionStoreError::RevisionConflict {
                expected: expected_revision,
                actual: actual_revision,
            });
        }

        let mut persisted = session.clone();
        persisted.revision = actual_revision.map_or(0, |revision| revision.saturating_add(1));
        persisted.updated_at_ms = updated_at_ms;
        persisted.validate()?;

        let parent =
            path.parent().ok_or_else(|| ReviewSessionStoreError::InvalidPath(path.clone()))?;
        fs_atomic::create_private_dir(parent).map_err(|source| ReviewSessionStoreError::Io {
            operation: "create directory for",
            path: parent.to_owned(),
            error: source,
        })?;
        let bytes = serde_json::to_vec(&persisted)?;
        fs_atomic::replace_private(&path, &bytes).map_err(|source| {
            ReviewSessionStoreError::Io { operation: "atomically replace", path, error: source }
        })?;
        Ok(persisted)
    }

    /// Returns the session record path after rejecting unsafe serialized identifiers.
    pub fn session_path(&self, review_id: &ReviewId) -> Result<PathBuf, ReviewSessionStoreError> {
        self.path(review_id)
    }

    fn path(&self, review_id: &ReviewId) -> Result<PathBuf, ReviewSessionStoreError> {
        let key = review_id
            .as_str()
            .strip_prefix("review:")
            .and_then(|raw| Uuid::parse_str(raw).ok())
            .ok_or_else(|| {
                ReviewSessionStoreError::InvalidReviewId(review_id.as_str().to_owned())
            })?;
        Ok(self.project_root.join(REVIEW_SESSION_RELATIVE_DIR).join(format!("{key}.json")))
    }
}

/// Errors produced while reading or atomically replacing feedback-session records.
#[derive(Debug, Error)]
pub enum ReviewSessionStoreError {
    /// Session filesystem work failed.
    #[error("could not {operation} review session at {path}: {error}")]
    Io {
        /// Filesystem operation that failed.
        operation: &'static str,
        /// Path involved in the operation.
        path: PathBuf,
        /// Underlying filesystem error.
        error: io::Error,
    },
    /// Session JSON could not be decoded; the original record remains untouched.
    #[error("could not decode review session at {path}: {error}")]
    Deserialize {
        /// Existing session path.
        path: PathBuf,
        /// Underlying JSON error.
        error: serde_json::Error,
    },
    /// Session JSON could not be encoded for staging.
    #[error("could not serialize review session: {0}")]
    Serialize(serde_json::Error),
    /// The session path lacks a parent directory.
    #[error("review session path has no parent: {0}")]
    InvalidPath(PathBuf),
    /// A deserialized review ID cannot safely become a filename.
    #[error("review session has an invalid review ID: {0}")]
    InvalidReviewId(String),
    /// The record filename and JSON body name different logical reviews.
    #[error("review session ID mismatch: expected {expected}, found {found}")]
    ReviewIdMismatch {
        /// Logical review named by the requested path.
        expected: String,
        /// Logical review declared by the session body.
        found: String,
    },
    /// Another writer changed the session after the caller last read it.
    #[error("review session revision conflict: expected {expected:?}, found {actual:?}")]
    RevisionConflict {
        /// Revision held by the caller, or `None` for a new session.
        expected: Option<u64>,
        /// Current persisted revision, or `None` when no record exists.
        actual: Option<u64>,
    },
    /// The session was malformed or requires a newer schema; it is never overwritten.
    #[error(transparent)]
    Session(#[from] ReviewSessionValidationError),
}

crate::error_cause::from_cause!(ReviewSessionStoreError {
    serde_json::Error => Serialize,
});

fn read_regular_file(path: &Path) -> Result<Vec<u8>, ReviewSessionStoreError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| ReviewSessionStoreError::Io {
        operation: "inspect",
        path: path.to_owned(),
        error: source,
    })?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(ReviewSessionStoreError::Io {
            operation: "read non-regular file",
            path: path.to_owned(),
            error: io::Error::other("review session is not a regular file"),
        });
    }
    fs::read(path).map_err(|source| ReviewSessionStoreError::Io {
        operation: "read",
        path: path.to_owned(),
        error: source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guided_review::{ReviewBase, ReviewSource, ReviewTarget};
    use tempfile::tempdir;

    fn session() -> ReviewSession {
        let revision_id =
            super::super::ReviewRevisionId::from_canonical_bytes(b"diff --git a/a b/a\n");
        ReviewSession::new(
            ReviewId::new(),
            ReviewTarget::WorkingTree,
            ReviewSource {
                base: ReviewBase::EmptyTree,
                content_digest: revision_id,
                includes_staged: false,
                includes_unstaged: true,
                included_untracked_files: 0,
                captured_at_ms: 42,
                uncovered_paths: Vec::new(),
            },
            42,
        )
    }

    #[test]
    fn store_round_trips_and_replaces_an_expected_revision() {
        let project = tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let store = ReviewSessionStore::new(project.path());
        let first = session();
        let first = store
            .save(None, &first, 50)
            .unwrap_or_else(|error| panic!("first save failed: {error}"));
        let second = store
            .save(Some(first.revision), &first, 60)
            .unwrap_or_else(|error| panic!("replacement failed: {error}"));

        assert_eq!(first.revision, 0);
        assert_eq!(second.revision, 1);
        assert_eq!(second.updated_at_ms, 60);
        assert_eq!(store.load(&first.id).unwrap_or_default(), Some(second));
    }

    #[test]
    fn store_rejects_conflicts_and_preserves_malformed_records() {
        let project = tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let store = ReviewSessionStore::new(project.path());
        let first =
            store.save(None, &session(), 50).unwrap_or_else(|error| panic!("save failed: {error}"));

        assert!(matches!(
            store.save(None, &first, 60),
            Err(ReviewSessionStoreError::RevisionConflict { expected: None, actual: Some(0) })
        ));

        let path =
            store.session_path(&first.id).unwrap_or_else(|error| panic!("path failed: {error}"));
        fs::write(&path, b"not json")
            .unwrap_or_else(|error| panic!("corrupt write failed: {error}"));
        assert!(matches!(store.load(&first.id), Err(ReviewSessionStoreError::Deserialize { .. })));
        assert!(matches!(
            store.save(Some(0), &first, 60),
            Err(ReviewSessionStoreError::Deserialize { .. })
        ));
        assert_eq!(fs::read(&path).unwrap_or_default(), b"not json");

        let different = session();
        fs::write(
            &path,
            serde_json::to_vec(&different)
                .unwrap_or_else(|error| panic!("serialize mismatched session failed: {error}")),
        )
        .unwrap_or_else(|error| panic!("mismatched write failed: {error}"));
        assert!(matches!(
            store.load(&first.id),
            Err(ReviewSessionStoreError::ReviewIdMismatch { .. })
        ));
    }

    #[test]
    fn store_rejects_newer_schema_without_replacing_the_draft() {
        let project = tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let store = ReviewSessionStore::new(project.path());
        let first =
            store.save(None, &session(), 50).unwrap_or_else(|error| panic!("save failed: {error}"));
        let path =
            store.session_path(&first.id).unwrap_or_else(|error| panic!("path failed: {error}"));
        let mut newer = first.clone();
        newer.schema_version += 1;
        let raw = serde_json::to_vec(&newer)
            .unwrap_or_else(|error| panic!("serialize newer session failed: {error}"));
        fs::write(&path, &raw).unwrap_or_else(|error| panic!("newer write failed: {error}"));

        assert!(matches!(
            store.load(&first.id),
            Err(ReviewSessionStoreError::Session(
                ReviewSessionValidationError::UnsupportedSchema { .. }
            ))
        ));
        assert!(matches!(
            store.save(Some(first.revision), &first, 60),
            Err(ReviewSessionStoreError::Session(
                ReviewSessionValidationError::UnsupportedSchema { .. }
            ))
        ));
        assert_eq!(fs::read(&path).unwrap_or_default(), raw);
    }

    #[cfg(unix)]
    #[test]
    fn store_writes_private_session_files() {
        use std::os::unix::fs::PermissionsExt;

        let project = tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let store = ReviewSessionStore::new(project.path());
        let saved =
            store.save(None, &session(), 50).unwrap_or_else(|error| panic!("save failed: {error}"));
        let path =
            store.session_path(&saved.id).unwrap_or_else(|error| panic!("path failed: {error}"));
        let parent = path.parent().unwrap_or_else(|| panic!("session path has no parent"));

        assert_eq!(fs::metadata(parent).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(fs::metadata(path).unwrap().permissions().mode() & 0o777, 0o600);
    }
}
