//! Review source store — pinned artifact schema and cache layout.
//!
//! A review revision owns canonical source bytes independently of the mutable working tree.
//! Persistence uses a private staging directory and atomic rename so readers only observe complete
//! immutable artifacts.

use super::ReviewRevisionId;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use thiserror::Error;
use uuid::Uuid;

/// Project-relative directory containing immutable review source artifacts.
pub const SOURCE_CACHE_RELATIVE_DIR: &str = ".luxe/reviews/sources";
/// Schema version for persisted review source manifests.
pub const SOURCE_ARTIFACT_SCHEMA_VERSION: u32 = 1;
/// Name of the manifest stored beside a canonical source payload.
pub const SOURCE_MANIFEST_FILE: &str = "manifest.json";
/// Name of the canonical source payload stored beside its manifest.
pub const SOURCE_PAYLOAD_FILE: &str = "source.diff";

/// Bounded retention policy for unprotected source artifacts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceRetentionPolicy {
    /// Maximum age before an unprotected, timestamped artifact becomes eligible for eviction.
    pub max_age_ms: Option<u64>,
    /// Maximum cache size considered by a future byte-budget cleanup pass.
    pub max_total_bytes: u64,
}

impl SourceRetentionPolicy {
    /// Returns whether age-based cleanup may remove this unprotected artifact at `now_ms`.
    pub fn may_evict(
        &self,
        manifest: &SourceArtifactManifest,
        now_ms: u64,
        protected_revisions: &HashSet<ReviewRevisionId>,
    ) -> bool {
        let (Some(max_age_ms), Some(captured_at_ms)) = (self.max_age_ms, manifest.captured_at_ms)
        else {
            return false;
        };
        if !self.may_evict_for_budget(manifest, protected_revisions) {
            return false;
        };
        now_ms.saturating_sub(captured_at_ms) >= max_age_ms
    }

    fn may_evict_for_budget(
        &self,
        manifest: &SourceArtifactManifest,
        protected_revisions: &HashSet<ReviewRevisionId>,
    ) -> bool {
        !protected_revisions.contains(&manifest.revision_id) && manifest.captured_at_ms.is_some()
    }
}

impl Default for SourceRetentionPolicy {
    fn default() -> Self {
        Self { max_age_ms: Some(30 * 24 * 60 * 60 * 1_000), max_total_bytes: 1_073_741_824 }
    }
}

/// Errors produced while persisting a pinned review source artifact.
#[derive(Debug, Error)]
pub enum SourceStoreError {
    /// Filesystem work failed before an artifact could be committed.
    #[error("could not {operation} review source artifact at {path}: {error}")]
    Io {
        /// Filesystem operation that failed.
        operation: &'static str,
        /// Path involved in the failed operation.
        path: PathBuf,
        /// Underlying filesystem error.
        error: io::Error,
    },
    /// The manifest could not be serialized for persistence.
    #[error("could not serialize review source manifest: {0}")]
    Serialize(serde_json::Error),
    /// A persisted manifest could not be decoded.
    #[error("could not decode review source manifest at {path}: {error}")]
    Deserialize {
        /// Manifest path that could not be decoded.
        path: PathBuf,
        /// Underlying JSON error.
        error: serde_json::Error,
    },
    /// The artifact uses a schema this luxe version does not understand.
    #[error("review source artifact uses unsupported schema {schema_version}")]
    UnsupportedSchema {
        /// Schema version found in the manifest.
        schema_version: u32,
    },
    /// A revision identifier cannot safely name a source-cache directory.
    #[error("review source artifact has an invalid revision identifier")]
    InvalidRevisionId,
    /// An artifact's manifest and payload do not describe the same immutable revision.
    #[error("review source artifact integrity check failed at {path}: {reason}")]
    Integrity {
        /// Artifact directory or file that failed verification.
        path: PathBuf,
        /// Non-secret explanation of the failed invariant.
        reason: &'static str,
    },
}

crate::error_cause::from_cause!(SourceStoreError {
    serde_json::Error => Serialize,
});

/// Metadata persisted with one immutable canonical source payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceArtifactManifest {
    /// Schema used to decode this artifact.
    pub schema_version: u32,
    /// Content-derived revision that identifies the payload.
    pub revision_id: ReviewRevisionId,
    /// Exact canonical payload length in bytes.
    pub source_len: u64,
    /// Capture time for age-based retention, absent in artifacts written before this field existed.
    #[serde(default)]
    pub captured_at_ms: Option<u64>,
}

impl SourceArtifactManifest {
    /// Describes canonical bytes using the current artifact schema.
    pub fn from_canonical_bytes(bytes: &[u8]) -> Self {
        Self::from_canonical_bytes_at(bytes, capture_time_ms())
    }

    /// Describes canonical bytes captured at a supplied timestamp for deterministic callers.
    pub fn from_canonical_bytes_at(bytes: &[u8], captured_at_ms: Option<u64>) -> Self {
        Self {
            schema_version: SOURCE_ARTIFACT_SCHEMA_VERSION,
            revision_id: ReviewRevisionId::from_canonical_bytes(bytes),
            source_len: bytes.len() as u64,
            captured_at_ms,
        }
    }
}

/// Canonical source bytes paired with their immutable manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewSourceArtifact {
    /// Persisted metadata for this source payload.
    pub manifest: SourceArtifactManifest,
    /// Exact captured bytes, never regenerated from a later workspace state.
    pub canonical_bytes: Vec<u8>,
}

impl ReviewSourceArtifact {
    /// Captures canonical source bytes for a single pinned revision.
    pub fn capture(canonical_bytes: Vec<u8>) -> Self {
        let manifest = SourceArtifactManifest::from_canonical_bytes(&canonical_bytes);
        Self { manifest, canonical_bytes }
    }

    /// Captures canonical source bytes with a deterministic retention timestamp.
    pub fn capture_at(canonical_bytes: Vec<u8>, captured_at_ms: Option<u64>) -> Self {
        let manifest =
            SourceArtifactManifest::from_canonical_bytes_at(&canonical_bytes, captured_at_ms);
        Self { manifest, canonical_bytes }
    }

    /// Returns the project-local path where this artifact belongs.
    pub fn cache_dir(&self, project_root: &Path) -> PathBuf {
        project_root.join(SOURCE_CACHE_RELATIVE_DIR).join(self.manifest.revision_id.cache_key())
    }

    /// Loads and verifies a pinned artifact without consulting the mutable workspace.
    pub fn load(
        project_root: &Path,
        revision_id: &ReviewRevisionId,
    ) -> Result<Self, SourceStoreError> {
        validate_revision_id(revision_id)?;
        let target = cache_dir_for(project_root, revision_id);
        verify_real_directory(&target)?;
        let manifest_path = target.join(SOURCE_MANIFEST_FILE);
        let manifest_bytes = read_file(&manifest_path)?;
        let manifest: SourceArtifactManifest =
            serde_json::from_slice(&manifest_bytes).map_err(|source| {
                SourceStoreError::Deserialize { path: manifest_path, error: source }
            })?;
        if manifest.schema_version != SOURCE_ARTIFACT_SCHEMA_VERSION {
            return Err(SourceStoreError::UnsupportedSchema {
                schema_version: manifest.schema_version,
            });
        }
        if &manifest.revision_id != revision_id {
            return Err(SourceStoreError::Integrity {
                path: target,
                reason: "manifest revision does not match requested revision",
            });
        }

        let payload_path = cache_dir_for(project_root, revision_id).join(SOURCE_PAYLOAD_FILE);
        let canonical_bytes = read_file(&payload_path)?;
        if manifest.source_len != canonical_bytes.len() as u64 {
            return Err(SourceStoreError::Integrity {
                path: payload_path,
                reason: "manifest byte length does not match payload",
            });
        }
        if ReviewRevisionId::from_canonical_bytes(&canonical_bytes) != manifest.revision_id {
            return Err(SourceStoreError::Integrity {
                path: cache_dir_for(project_root, revision_id),
                reason: "payload digest does not match manifest revision",
            });
        }

        Ok(Self { manifest, canonical_bytes })
    }

    /// Removes only verified, expired artifacts that no caller protects.
    pub fn prune_expired(
        project_root: &Path,
        retention: &SourceRetentionPolicy,
        now_ms: u64,
        protected_revisions: &HashSet<ReviewRevisionId>,
    ) -> Result<Vec<ReviewRevisionId>, SourceStoreError> {
        let candidates = verified_artifacts(project_root)?;
        evict_candidates(candidates.into_iter().filter(|candidate| {
            retention.may_evict(&candidate.manifest, now_ms, protected_revisions)
        }))
    }

    /// Enforces the canonical-payload byte budget using deterministic oldest-first eviction.
    pub fn prune_to_byte_budget(
        project_root: &Path,
        retention: &SourceRetentionPolicy,
        protected_revisions: &HashSet<ReviewRevisionId>,
    ) -> Result<Vec<ReviewRevisionId>, SourceStoreError> {
        let candidates = verified_artifacts(project_root)?;
        let mut total_bytes: u64 =
            candidates.iter().map(|candidate| candidate.manifest.source_len).sum();
        let mut evictable: Vec<_> = candidates
            .into_iter()
            .filter(|candidate| {
                retention.may_evict_for_budget(&candidate.manifest, protected_revisions)
            })
            .collect();
        evictable.sort_by(|left, right| {
            left.manifest.captured_at_ms.cmp(&right.manifest.captured_at_ms).then_with(|| {
                left.manifest.revision_id.as_str().cmp(right.manifest.revision_id.as_str())
            })
        });

        let mut removed = Vec::new();
        for candidate in evictable {
            if total_bytes <= retention.max_total_bytes {
                break;
            }
            remove_artifact(&candidate)?;
            total_bytes = total_bytes.saturating_sub(candidate.manifest.source_len);
            removed.push(candidate.manifest.revision_id);
        }
        Ok(removed)
    }

    /// Atomically persists this immutable artifact under the project-local source cache.
    pub fn persist(&self, project_root: &Path) -> Result<PathBuf, SourceStoreError> {
        validate_revision_id(&self.manifest.revision_id)?;
        let target = self.cache_dir(project_root);
        self.validate_for_persistence(&target)?;
        let cache_root = project_root.join(SOURCE_CACHE_RELATIVE_DIR);
        create_private_dir(&cache_root)?;

        if target.exists() {
            return self.verify_existing(project_root, target);
        }

        let staging = cache_root.join(format!(".staging-{}", Uuid::new_v4()));
        create_private_dir(&staging)?;
        let result = self.write_staging(&staging).and_then(|()| {
            fs::rename(&staging, &target).map_err(|source| SourceStoreError::Io {
                operation: "commit",
                path: target.clone(),
                error: source,
            })
        });

        match result {
            Ok(()) => Ok(target),
            Err(_error) if target.exists() => {
                let _ = fs::remove_dir_all(&staging);
                self.verify_existing(project_root, target)
            }
            Err(error) => {
                let _ = fs::remove_dir_all(&staging);
                Err(error)
            }
        }
    }

    fn validate_for_persistence(&self, target: &Path) -> Result<(), SourceStoreError> {
        if self.manifest.source_len != self.canonical_bytes.len() as u64 {
            return Err(SourceStoreError::Integrity {
                path: target.to_owned(),
                reason: "manifest byte length does not match payload",
            });
        }
        if ReviewRevisionId::from_canonical_bytes(&self.canonical_bytes)
            != self.manifest.revision_id
        {
            return Err(SourceStoreError::Integrity {
                path: target.to_owned(),
                reason: "payload digest does not match manifest revision",
            });
        }
        Ok(())
    }

    fn verify_existing(
        &self,
        project_root: &Path,
        target: PathBuf,
    ) -> Result<PathBuf, SourceStoreError> {
        let stored = Self::load(project_root, &self.manifest.revision_id)?;
        if stored.canonical_bytes == self.canonical_bytes {
            Ok(target)
        } else {
            Err(SourceStoreError::Integrity {
                path: target,
                reason: "existing payload differs from the requested canonical bytes",
            })
        }
    }

    fn write_staging(&self, staging: &Path) -> Result<(), SourceStoreError> {
        let manifest = serde_json::to_vec(&self.manifest)?;
        write_private_file(&staging.join(SOURCE_PAYLOAD_FILE), &self.canonical_bytes)?;
        write_private_file(&staging.join(SOURCE_MANIFEST_FILE), &manifest)
    }
}

struct StoredArtifact {
    path: PathBuf,
    manifest: SourceArtifactManifest,
}

fn verified_artifacts(project_root: &Path) -> Result<Vec<StoredArtifact>, SourceStoreError> {
    let cache_root = project_root.join(SOURCE_CACHE_RELATIVE_DIR);
    if !cache_root.exists() {
        return Ok(Vec::new());
    }
    let entries = fs::read_dir(&cache_root).map_err(|source| SourceStoreError::Io {
        operation: "scan",
        path: cache_root.clone(),
        error: source,
    })?;
    let mut artifacts = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| SourceStoreError::Io {
            operation: "scan",
            path: cache_root.clone(),
            error: source,
        })?;
        let path = entry.path();
        if !entry
            .file_type()
            .map_err(|source| SourceStoreError::Io {
                operation: "inspect",
                path: path.clone(),
                error: source,
            })?
            .is_dir()
        {
            continue;
        }
        let manifest_path = path.join(SOURCE_MANIFEST_FILE);
        let Ok(manifest_bytes) = fs::read(&manifest_path) else {
            continue;
        };
        let Ok(manifest) = serde_json::from_slice::<SourceArtifactManifest>(&manifest_bytes) else {
            continue;
        };
        if manifest.schema_version == SOURCE_ARTIFACT_SCHEMA_VERSION
            && path == cache_dir_for(project_root, &manifest.revision_id)
            && ReviewSourceArtifact::load(project_root, &manifest.revision_id).is_ok()
        {
            artifacts.push(StoredArtifact { path, manifest });
        }
    }
    Ok(artifacts)
}

fn evict_candidates(
    candidates: impl IntoIterator<Item = StoredArtifact>,
) -> Result<Vec<ReviewRevisionId>, SourceStoreError> {
    let mut removed = Vec::new();
    for candidate in candidates {
        remove_artifact(&candidate)?;
        removed.push(candidate.manifest.revision_id);
    }
    Ok(removed)
}

fn remove_artifact(candidate: &StoredArtifact) -> Result<(), SourceStoreError> {
    fs::remove_dir_all(&candidate.path).map_err(|source| SourceStoreError::Io {
        operation: "evict",
        path: candidate.path.clone(),
        error: source,
    })
}

fn capture_time_ms() -> Option<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| duration.as_millis().try_into().ok())
}

fn cache_dir_for(project_root: &Path, revision_id: &ReviewRevisionId) -> PathBuf {
    project_root.join(SOURCE_CACHE_RELATIVE_DIR).join(revision_id.cache_key())
}

fn validate_revision_id(revision_id: &ReviewRevisionId) -> Result<(), SourceStoreError> {
    let Some(digest) = revision_id.as_str().strip_prefix("revision:") else {
        return Err(SourceStoreError::InvalidRevisionId);
    };
    if digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(SourceStoreError::InvalidRevisionId)
    }
}

fn verify_real_directory(path: &Path) -> Result<(), SourceStoreError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| SourceStoreError::Io {
        operation: "inspect",
        path: path.to_owned(),
        error: source,
    })?;
    if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
        Ok(())
    } else {
        Err(SourceStoreError::Integrity {
            path: path.to_owned(),
            reason: "artifact directory is not a real directory",
        })
    }
}

fn read_file(path: &Path) -> Result<Vec<u8>, SourceStoreError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| SourceStoreError::Io {
        operation: "inspect",
        path: path.to_owned(),
        error: source,
    })?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(SourceStoreError::Integrity {
            path: path.to_owned(),
            reason: "artifact file is not a regular file",
        });
    }
    fs::read(path).map_err(|source| SourceStoreError::Io {
        operation: "read",
        path: path.to_owned(),
        error: source,
    })
}

pub(crate) fn create_private_dir(path: &Path) -> Result<(), SourceStoreError> {
    crate::fs_atomic::create_private_dir(path).map_err(|source| SourceStoreError::Io {
        operation: "create directory",
        path: path.to_owned(),
        error: source,
    })
}

pub(crate) fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), SourceStoreError> {
    crate::fs_atomic::write_private(path, bytes).map_err(|source| SourceStoreError::Io {
        operation: "write",
        path: path.to_owned(),
        error: source,
    })
}

#[cfg(test)]
mod tests;
