//! Last Turn session record — restart-safe selector for a retained baseline.
//!
//! The record points at the content-addressed baseline cache; it never duplicates source bytes.

use super::{SourceStoreError, TurnBaselineId};
use crate::fs_atomic;
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};
use thiserror::Error;

/// Schema version for persisted Last Turn selectors.
pub const LAST_TURN_RECORD_SCHEMA_VERSION: u32 = 2;
/// Project-relative location of the Last Turn selector.
pub const LAST_TURN_RECORD_RELATIVE_PATH: &str = ".luxe/reviews/last-turn.json";

/// Restart-safe identity of the most recent complete turn snapshots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastTurnRecord {
    /// Schema used to decode this record.
    pub schema_version: u32,
    /// Session that owned the captured turn.
    pub session_id: String,
    /// Turn whose before/after snapshots were retained.
    pub turn_id: String,
    /// Immutable snapshot captured before the turn began.
    pub before_snapshot_id: TurnBaselineId,
    /// Immutable snapshot captured immediately after the turn ended.
    pub after_snapshot_id: TurnBaselineId,
}

impl LastTurnRecord {
    /// Captures a selector only when both identity components are non-empty.
    pub fn new(
        session_id: impl Into<String>,
        turn_id: impl Into<String>,
    ) -> Result<Self, LastTurnRecordError> {
        let session_id = session_id.into();
        let turn_id = turn_id.into();
        let record = Self {
            schema_version: LAST_TURN_RECORD_SCHEMA_VERSION,
            before_snapshot_id: TurnBaselineId::from_session_turn(&session_id, &turn_id),
            after_snapshot_id: TurnBaselineId::post_turn_from_session_turn(&session_id, &turn_id),
            session_id,
            turn_id,
        };
        record.validate()?;
        Ok(record)
    }

    /// Returns the immutable snapshot captured before this turn.
    pub fn baseline_id(&self) -> &TurnBaselineId {
        &self.before_snapshot_id
    }

    /// Returns the immutable snapshot captured immediately after this turn.
    pub fn after_snapshot_id(&self) -> &TurnBaselineId {
        &self.after_snapshot_id
    }

    /// Rejects incompatible records and blank selectors before use.
    pub fn validate(&self) -> Result<(), LastTurnRecordError> {
        if self.schema_version != LAST_TURN_RECORD_SCHEMA_VERSION {
            return Err(LastTurnRecordError::UnsupportedSchema {
                schema_version: self.schema_version,
            });
        }
        if self.session_id.trim().is_empty() || self.turn_id.trim().is_empty() {
            return Err(LastTurnRecordError::MissingIdentity);
        }
        Ok(())
    }
}

/// Stores the Last Turn selector without duplicating the immutable baseline.
#[derive(Debug, Clone)]
pub struct LastTurnRecordStore {
    project_root: PathBuf,
}

impl LastTurnRecordStore {
    /// Opens the Last Turn selector location for one project.
    pub fn new(project_root: impl Into<PathBuf>) -> Self {
        Self { project_root: project_root.into() }
    }

    /// Loads a valid selector, returning no record until a complete baseline is captured.
    pub fn load(&self) -> Result<Option<LastTurnRecord>, LastTurnRecordStoreError> {
        let path = self.path();
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(LastTurnRecordStoreError::Io { operation: "read", path, error: source })
            }
        };
        let record: LastTurnRecord = serde_json::from_slice(&bytes).map_err(|source| {
            LastTurnRecordStoreError::Deserialize { path: self.path(), error: source }
        })?;
        record.validate()?;
        Ok(Some(record))
    }

    /// Atomically updates the selector after refusing an unsupported existing record.
    pub fn save(&self, record: &LastTurnRecord) -> Result<(), LastTurnRecordStoreError> {
        record.validate()?;
        let _ = self.load()?;
        let path = self.path();
        let parent =
            path.parent().ok_or_else(|| LastTurnRecordStoreError::InvalidPath(path.clone()))?;
        fs_atomic::create_private_dir(parent).map_err(|source| LastTurnRecordStoreError::Io {
            operation: "create directory for",
            path: parent.to_owned(),
            error: source,
        })?;
        let bytes = serde_json::to_vec(record)?;
        fs_atomic::replace_private(&path, &bytes).map_err(|source| LastTurnRecordStoreError::Io {
            operation: "atomically replace",
            path,
            error: source,
        })
    }

    fn path(&self) -> PathBuf {
        self.project_root.join(LAST_TURN_RECORD_RELATIVE_PATH)
    }
}

/// Errors that prevent a Last Turn selector from being used safely.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum LastTurnRecordError {
    /// The selector was produced by an unsupported schema.
    #[error("Last Turn record uses unsupported schema {schema_version}")]
    UnsupportedSchema {
        /// Schema version encountered in the record.
        schema_version: u32,
    },
    /// A selector must name both its session and turn.
    #[error("Last Turn record is missing its session or turn identity")]
    MissingIdentity,
}

/// Errors produced while loading or atomically replacing a Last Turn selector.
#[derive(Debug, Error)]
pub enum LastTurnRecordStoreError {
    /// Filesystem access failed.
    #[error("could not {operation} Last Turn record at {path}: {error}")]
    Io {
        /// Filesystem operation attempted.
        operation: &'static str,
        /// Path involved in the operation.
        path: PathBuf,
        /// Underlying I/O error.
        error: std::io::Error,
    },
    /// The record could not be decoded.
    #[error("could not decode Last Turn record at {path}: {error}")]
    Deserialize {
        /// Path involved in the operation.
        path: PathBuf,
        /// Underlying serialization error.
        error: serde_json::Error,
    },
    /// The record could not be serialized.
    #[error("could not serialize Last Turn record: {0}")]
    Serialize(serde_json::Error),
    /// The record path lacks a parent directory.
    #[error("Last Turn record path has no parent: {0}")]
    InvalidPath(PathBuf),
    /// The selector was unsafe to use.
    #[error(transparent)]
    Record(#[from] LastTurnRecordError),
    /// Private metadata directory or staging-file setup failed.
    #[error(transparent)]
    SourceStore(#[from] SourceStoreError),
}

crate::error_cause::from_cause!(LastTurnRecordStoreError {
    serde_json::Error => Serialize,
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_store_round_trips_a_private_selector() {
        let directory = tempfile::tempdir().unwrap();
        let record = LastTurnRecord::new("session", "turn").unwrap();
        let store = LastTurnRecordStore::new(directory.path());
        store.save(&record).unwrap();
        assert_eq!(record.baseline_id(), &TurnBaselineId::from_session_turn("session", "turn"));
        assert_eq!(
            record.after_snapshot_id(),
            &TurnBaselineId::post_turn_from_session_turn("session", "turn")
        );
        assert_eq!(store.load().unwrap(), Some(record));
    }
}
