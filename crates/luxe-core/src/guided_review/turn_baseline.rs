//! Turn baseline — retained pre-turn content manifest.
//!
//! A LastTurn review must compare against the actual pre-turn workspace, including eligible
//! untracked files. This manifest records that snapshot independently of mutable Git state,
//! naming any file too large or unreadable to hold; payload persistence is added by the loader.

use super::{
    source_store::{create_private_dir, write_private_file},
    ReviewBase, ReviewRevisionId, SourceStoreError, TurnBaselineId,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::{Component, Path, PathBuf},
};
use thiserror::Error;
use uuid::Uuid;

/// Schema version for retained pre-turn baseline manifests.
pub const TURN_BASELINE_SCHEMA_VERSION: u32 = 2;
/// Project-relative cache directory containing immutable LastTurn baseline artifacts.
pub const TURN_BASELINE_CACHE_RELATIVE_DIR: &str = ".luxe/reviews/turn-baselines";
const TURN_BASELINE_MANIFEST_FILE: &str = "manifest.json";
const TURN_BASELINE_PAYLOAD_DIR: &str = "files";

/// One retained pre-turn workspace manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnBaselineManifest {
    /// Schema used to decode the baseline manifest.
    pub schema_version: u32,
    /// Stable session-turn identity for this snapshot.
    pub id: TurnBaselineId,
    /// Resolved committed base, or empty-tree provenance for an unborn repository.
    pub base: ReviewBase,
    /// Time at which the pre-turn snapshot was captured.
    pub captured_at_ms: u64,
    /// Whether the Git index differed from `HEAD` when this snapshot was captured.
    pub includes_staged: bool,
    /// Whether the worktree differed from the Git index when this snapshot was captured.
    pub includes_unstaged: bool,
    /// Number of eligible untracked files retained in this snapshot.
    pub included_untracked_files: u64,
    /// Every file the snapshot holds.
    pub files: Vec<TurnBaselineFile>,
    /// Files left out because they were too large or could not be read.
    ///
    /// A comparison drops these from both sides, so a file one snapshot could not hold never
    /// reads as added or deleted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub excluded: Vec<TurnBaselineExclusion>,
}

/// A file a snapshot left out, with what its metadata said, so a comparison can tell an untouched
/// one from one the turn changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnBaselineExclusion {
    /// Normal repository-relative file path.
    pub path: PathBuf,
    /// Its size, when it could be read
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub byte_len: Option<u64>,
    /// Its modification time in milliseconds since the epoch, when it could be read
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified_ms: Option<u64>,
}

impl TurnBaselineExclusion {
    /// Whether `other` describes the same file, unchanged; unknown metadata never matches.
    pub(crate) fn unchanged_in(&self, other: &Self) -> bool {
        self.path == other.path
            && self.byte_len.is_some()
            && self.modified_ms.is_some()
            && (self.byte_len, self.modified_ms) == (other.byte_len, other.modified_ms)
    }
}

impl TurnBaselineManifest {
    /// Builds a validated manifest for one workspace snapshot.
    pub fn new(
        id: TurnBaselineId,
        base: ReviewBase,
        captured_at_ms: u64,
        includes_staged: bool,
        includes_unstaged: bool,
        included_untracked_files: u64,
        files: Vec<TurnBaselineFile>,
    ) -> Result<Self, TurnBaselineManifestError> {
        let manifest = Self {
            schema_version: TURN_BASELINE_SCHEMA_VERSION,
            id,
            base,
            captured_at_ms,
            includes_staged,
            includes_unstaged,
            included_untracked_files,
            files,
            excluded: Vec::new(),
        };
        manifest.validate()?;
        Ok(manifest)
    }

    /// Rejects malformed paths, duplicate entries, and unsupported schemas before use.
    pub fn validate(&self) -> Result<(), TurnBaselineManifestError> {
        if self.schema_version != TURN_BASELINE_SCHEMA_VERSION {
            return Err(TurnBaselineManifestError::UnsupportedSchema {
                schema_version: self.schema_version,
            });
        }
        let mut paths = HashSet::with_capacity(self.files.len());
        for file in &self.files {
            if !is_normal_relative_path(&file.path) {
                return Err(TurnBaselineManifestError::InvalidPath { path: file.path.clone() });
            }
            if !paths.insert(&file.path) {
                return Err(TurnBaselineManifestError::DuplicatePath { path: file.path.clone() });
            }
            if !is_sha256_hex(&file.content_digest) {
                return Err(TurnBaselineManifestError::InvalidDigest { path: file.path.clone() });
            }
        }
        Ok(())
    }
}

/// One file entry in a pre-turn workspace snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnBaselineFile {
    /// Normal repository-relative file path.
    pub path: PathBuf,
    /// SHA-256 digest of the retained file payload.
    pub content_digest: String,
    /// Exact retained payload length in bytes.
    pub byte_len: u64,
}

/// Errors that prevent a baseline manifest from representing a complete safe snapshot.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum TurnBaselineManifestError {
    /// The manifest requires a newer or incompatible schema.
    #[error("turn baseline uses unsupported schema {schema_version}")]
    UnsupportedSchema {
        /// Schema version encountered in the manifest.
        schema_version: u32,
    },
    /// A file path is not a normal repository-relative path.
    #[error("turn baseline contains invalid path {path}")]
    InvalidPath {
        /// Rejected path.
        path: PathBuf,
    },
    /// Multiple entries claim the same file path.
    #[error("turn baseline contains duplicate path {path}")]
    DuplicatePath {
        /// Duplicate path.
        path: PathBuf,
    },
    /// An entry lacks a valid SHA-256 payload digest.
    #[error("turn baseline contains invalid digest for {path}")]
    InvalidDigest {
        /// File with the invalid digest.
        path: PathBuf,
    },
}

/// Immutable baseline manifest plus exact private file payloads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnBaselineArtifact {
    /// Validated manifest describing every retained file.
    pub manifest: TurnBaselineManifest,
    payloads: BTreeMap<PathBuf, Vec<u8>>,
}

impl TurnBaselineArtifact {
    /// Captures pre-turn file payloads into a validated immutable artifact.
    pub fn capture(
        id: TurnBaselineId,
        base: ReviewBase,
        captured_at_ms: u64,
        includes_staged: bool,
        includes_unstaged: bool,
        included_untracked_files: u64,
        files: Vec<(PathBuf, Vec<u8>)>,
    ) -> Result<Self, TurnBaselineManifestError> {
        let entries = files
            .iter()
            .map(|(path, bytes)| TurnBaselineFile {
                path: path.clone(),
                content_digest: payload_digest(bytes),
                byte_len: bytes.len() as u64,
            })
            .collect();
        let manifest = TurnBaselineManifest::new(
            id,
            base,
            captured_at_ms,
            includes_staged,
            includes_unstaged,
            included_untracked_files,
            entries,
        )?;
        let payloads: BTreeMap<_, _> = files.into_iter().collect();
        Ok(Self { manifest, payloads })
    }

    /// Derives the immutable source revision for this exact baseline content.
    pub fn revision_id(&self) -> ReviewRevisionId {
        let mut canonical = Vec::new();
        for (path, bytes) in &self.payloads {
            let path = path.as_os_str().as_encoded_bytes();
            canonical.extend_from_slice(&(path.len() as u64).to_be_bytes());
            canonical.extend_from_slice(path);
            canonical.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
            canonical.extend_from_slice(bytes);
        }
        ReviewRevisionId::from_canonical_bytes(&canonical)
    }

    /// Iterates the verified repository-relative payloads in deterministic path order.
    pub fn files(&self) -> impl Iterator<Item = (&Path, &[u8])> {
        self.payloads.iter().map(|(path, bytes)| (path.as_path(), bytes.as_slice()))
    }

    /// Returns the pinned payload for one repository-relative path.
    pub fn file(&self, path: &Path) -> Option<&[u8]> {
        self.payloads.get(path).map(Vec::as_slice)
    }

    /// Materializes the verified immutable snapshot in a new private directory.
    ///
    /// This is deliberately separate from the worktree so LastTurn comparison can use only
    /// retained bytes and a separately captured post-turn snapshot.
    pub fn materialize_into(&self, root: &Path) -> Result<(), TurnBaselineStoreError> {
        self.materialize_except(root, &[])
    }

    /// Records the files this snapshot had to leave out.
    pub(crate) fn excluding(mut self, excluded: Vec<TurnBaselineExclusion>) -> Self {
        self.manifest.excluded = excluded;
        self
    }

    /// Like [`Self::materialize_into`], leaving out `skip`: what another snapshot could not hold.
    pub(crate) fn materialize_except(
        &self,
        root: &Path,
        skip: &[PathBuf],
    ) -> Result<(), TurnBaselineStoreError> {
        self.validate_payloads(root)?;
        if root.exists() {
            return Err(TurnBaselineStoreError::Integrity {
                path: root.to_owned(),
                reason: "snapshot directory already exists",
            });
        }
        create_private_dir(root)?;
        for (path, bytes) in &self.payloads {
            let entry =
                self.manifest.files.iter().find(|entry| entry.path == *path).ok_or_else(|| {
                    TurnBaselineStoreError::Integrity {
                        path: root.to_owned(),
                        reason: "payload path missing from manifest",
                    }
                })?;
            if entry.content_digest != payload_digest(bytes) || entry.byte_len != bytes.len() as u64
            {
                return Err(TurnBaselineStoreError::Integrity {
                    path: root.to_owned(),
                    reason: "payload metadata differs",
                });
            }
            if skip.contains(path) {
                continue;
            }
            let destination = root.join(path);
            let parent = destination.parent().ok_or_else(|| TurnBaselineStoreError::Integrity {
                path: destination.clone(),
                reason: "payload has no parent directory",
            })?;
            create_private_dir(parent)?;
            write_private_file(&destination, bytes)?;
        }
        Ok(())
    }

    /// Persists the complete baseline atomically under a project-local private cache.
    pub fn persist(&self, project_root: &Path) -> Result<PathBuf, TurnBaselineStoreError> {
        validate_baseline_id(&self.manifest.id)?;
        self.validate_payloads(project_root)?;
        let root = project_root.join(TURN_BASELINE_CACHE_RELATIVE_DIR);
        create_private_dir(&root)?;
        let target = root.join(self.manifest.id.cache_key());
        if target.exists() {
            let stored = Self::load(project_root, &self.manifest.id)?;
            return if stored == *self {
                Ok(target)
            } else {
                Err(TurnBaselineStoreError::Integrity {
                    path: target,
                    reason: "existing baseline differs",
                })
            };
        }
        let staging = root.join(format!(".staging-{}", Uuid::new_v4()));
        create_private_dir(&staging)?;
        let result = self.write_staging(&staging).and_then(|()| {
            fs::rename(&staging, &target).map_err(|source| TurnBaselineStoreError::Io {
                operation: "commit",
                path: target.clone(),
                error: source,
            })
        });
        match result {
            Ok(()) => Ok(target),
            Err(_) if target.exists() => {
                let _ = fs::remove_dir_all(&staging);
                let stored = Self::load(project_root, &self.manifest.id)?;
                if stored == *self {
                    Ok(target)
                } else {
                    Err(TurnBaselineStoreError::Integrity {
                        path: target,
                        reason: "racing baseline differs",
                    })
                }
            }
            Err(error) => {
                let _ = fs::remove_dir_all(&staging);
                Err(error)
            }
        }
    }

    /// Loads and verifies every pinned baseline payload without reading the workspace.
    pub fn load(project_root: &Path, id: &TurnBaselineId) -> Result<Self, TurnBaselineStoreError> {
        validate_baseline_id(id)?;
        let target = project_root.join(TURN_BASELINE_CACHE_RELATIVE_DIR).join(id.cache_key());
        let manifest_path = target.join(TURN_BASELINE_MANIFEST_FILE);
        let manifest: TurnBaselineManifest =
            serde_json::from_slice(&fs::read(&manifest_path).map_err(|source| {
                TurnBaselineStoreError::Io {
                    operation: "read",
                    path: manifest_path.clone(),
                    error: source,
                }
            })?)
            .map_err(|source| TurnBaselineStoreError::Deserialize {
                path: manifest_path,
                error: source,
            })?;
        if &manifest.id != id {
            return Err(TurnBaselineStoreError::Integrity {
                path: target,
                reason: "manifest identity differs",
            });
        }
        manifest.validate()?;
        let mut payloads = BTreeMap::new();
        for file in &manifest.files {
            let path = target.join(TURN_BASELINE_PAYLOAD_DIR).join(&file.content_digest);
            let bytes = fs::read(&path).map_err(|source| TurnBaselineStoreError::Io {
                operation: "read",
                path: path.clone(),
                error: source,
            })?;
            if bytes.len() as u64 != file.byte_len || payload_digest(&bytes) != file.content_digest
            {
                return Err(TurnBaselineStoreError::Integrity {
                    path,
                    reason: "payload does not match manifest",
                });
            }
            payloads.insert(file.path.clone(), bytes);
        }
        Ok(Self { manifest, payloads })
    }

    fn validate_payloads(&self, root: &Path) -> Result<(), TurnBaselineStoreError> {
        self.manifest.validate()?;
        if self.payloads.len() != self.manifest.files.len() {
            return Err(TurnBaselineStoreError::Integrity {
                path: root.to_owned(),
                reason: "manifest and payload entry counts differ",
            });
        }
        for entry in &self.manifest.files {
            let Some(bytes) = self.payloads.get(&entry.path) else {
                return Err(TurnBaselineStoreError::Integrity {
                    path: root.to_owned(),
                    reason: "manifest path has no payload",
                });
            };
            if entry.content_digest != payload_digest(bytes) || entry.byte_len != bytes.len() as u64
            {
                return Err(TurnBaselineStoreError::Integrity {
                    path: root.to_owned(),
                    reason: "payload metadata differs",
                });
            }
        }
        Ok(())
    }

    fn write_staging(&self, staging: &Path) -> Result<(), TurnBaselineStoreError> {
        let payload_dir = staging.join(TURN_BASELINE_PAYLOAD_DIR);
        create_private_dir(&payload_dir)?;
        for (path, bytes) in &self.payloads {
            let digest = payload_digest(bytes);
            let entry =
                self.manifest.files.iter().find(|entry| entry.path == *path).ok_or_else(|| {
                    TurnBaselineStoreError::Integrity {
                        path: staging.to_owned(),
                        reason: "payload path missing from manifest",
                    }
                })?;
            if entry.content_digest != digest || entry.byte_len != bytes.len() as u64 {
                return Err(TurnBaselineStoreError::Integrity {
                    path: staging.to_owned(),
                    reason: "payload metadata differs",
                });
            }
            let payload_path = payload_dir.join(digest);
            if !payload_path.exists() {
                write_private_file(&payload_path, bytes)?;
            }
        }
        let manifest = serde_json::to_vec(&self.manifest)?;
        write_private_file(&staging.join(TURN_BASELINE_MANIFEST_FILE), &manifest)?;
        Ok(())
    }
}

/// Errors produced while retaining or reloading a pre-turn baseline.
#[derive(Debug, Error)]
pub enum TurnBaselineStoreError {
    /// Filesystem work failed while handling a baseline artifact.
    #[error("could not {operation} turn baseline at {path}: {error}")]
    Io { operation: &'static str, path: PathBuf, error: std::io::Error },
    /// The manifest could not be serialized or decoded.
    #[error("could not process turn baseline manifest: {0}")]
    Serialize(serde_json::Error),
    /// A persisted manifest could not be decoded.
    #[error("could not decode turn baseline manifest at {path}: {error}")]
    Deserialize { path: PathBuf, error: serde_json::Error },
    /// The baseline manifest is unsafe or incomplete.
    #[error(transparent)]
    Manifest(#[from] TurnBaselineManifestError),
    /// A baseline identifier cannot safely name a cache directory.
    #[error("turn baseline has an invalid identifier")]
    InvalidBaselineId,
    /// Manifest and pinned payloads differ.
    #[error("turn baseline integrity check failed at {path}: {reason}")]
    Integrity { path: PathBuf, reason: &'static str },
    /// Shared private-store operation failed.
    #[error(transparent)]
    SourceStore(#[from] SourceStoreError),
}

crate::error_cause::from_cause!(TurnBaselineStoreError {
    serde_json::Error => Serialize,
});

fn validate_baseline_id(id: &TurnBaselineId) -> Result<(), TurnBaselineStoreError> {
    let Some(digest) = id.as_str().strip_prefix("turn-baseline:") else {
        return Err(TurnBaselineStoreError::InvalidBaselineId);
    };
    if is_sha256_hex(digest) {
        Ok(())
    } else {
        Err(TurnBaselineStoreError::InvalidBaselineId)
    }
}

fn payload_digest(bytes: &[u8]) -> String {
    base16ct::lower::encode_string(&Sha256::digest(bytes))
}

fn is_normal_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path.components().all(|component| matches!(component, Component::Normal(_)))
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests;
