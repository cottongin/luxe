//! Credential store — `~/.config/luxe/auth.json`, `0600`, and never lossy.
//!
//! The file is a superset of the one Luxe already read: a few reserved keys keep their
//! [`crate::config::ModelsConfig`] meaning, and every *other* top-level key is one provider's
//! [`Credential`]. An existing hand-written file keeps working, and a file written by another tool
//! that uses the same convention can be dropped in.
//!
//! Losslessness and locking come from [`JsonFile`]; this module owns the schema on top of it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use luxe_core::auth::StoredCredentialInfo;

use super::credential::Credential;
use super::json_file::{FileError, FileMode, JsonFile};

/// Top-level keys that are configuration, not credentials.
const RESERVED_KEYS: &[&str] =
    &["providers", "models", "default_provider", "default_model", "$schema"];

/// Reading or writing the auth file failed.
pub type StoreError = FileError;

/// File-backed credential store.
#[derive(Debug, Clone)]
pub struct AuthStore {
    file: JsonFile,
}

impl AuthStore {
    /// Store at an explicit path (tests, alternate profiles).
    pub fn new(path: PathBuf) -> Self {
        Self { file: JsonFile::new(path, FileMode::OwnerOnly) }
    }

    /// The default store: `auth.json` in Luxe's config directory, or where an older build left it.
    pub fn default_path() -> Self {
        Self::new(luxe_core::user_dirs::luxe_config_file("auth.json"))
    }

    /// Where this store reads and writes.
    pub fn path(&self) -> &Path {
        self.file.path()
    }

    /// Every credential in the file, keyed by provider id. Unparseable entries are skipped with a
    /// warning and left on disk.
    ///
    /// Never resolves `$ENV` / `!command` key specs — enumerating credentials must not run a user's
    /// shell command.
    pub fn credentials(&self) -> BTreeMap<String, Credential> {
        let Some(root) = self.file.read() else { return BTreeMap::new() };
        self.credentials_in(&root)
    }

    /// The whole file, parsed once, for a caller that reads it more than one way.
    pub(crate) fn read(&self) -> Option<serde_json::Map<String, serde_json::Value>> {
        self.file.read()
    }

    /// [`Self::credentials`] over a document [`Self::read`] already returned.
    pub(crate) fn credentials_in(
        &self,
        root: &serde_json::Map<String, serde_json::Value>,
    ) -> BTreeMap<String, Credential> {
        let mut out = BTreeMap::new();
        for (key, value) in root {
            if RESERVED_KEYS.contains(&key.as_str()) {
                continue;
            }
            match Credential::from_json(value) {
                Some(credential) => {
                    out.insert(key.clone(), credential);
                }
                None => tracing::warn!(
                    provider = %key,
                    path = %self.path().display(),
                    "skipping unrecognized auth.json entry (left untouched)"
                ),
            }
        }
        out
    }

    /// One provider's credential, if present.
    pub fn credential(&self, provider_id: &str) -> Option<Credential> {
        if RESERVED_KEYS.contains(&provider_id) {
            return None;
        }
        Credential::from_json(self.file.read()?.get(provider_id)?)
    }

    /// Non-secret metadata for every stored credential.
    pub fn summaries(&self) -> BTreeMap<String, StoredCredentialInfo> {
        self.credentials()
            .into_iter()
            .map(|(provider, credential)| (provider, credential.info()))
            .collect()
    }

    /// Whether the file holds any credential at all.
    pub fn is_empty(&self) -> bool {
        self.credentials().is_empty()
    }

    /// Store (or replace) one provider's credential.
    pub fn put(&self, provider_id: &str, credential: &Credential) -> Result<(), StoreError> {
        self.modify(provider_id, |_| Ok(Some(credential.clone()))).map(|_| ())
    }

    /// Remove one provider's credential. Returns whether there was one.
    pub fn remove(&self, provider_id: &str) -> Result<bool, StoreError> {
        let mut existed = false;
        self.modify(provider_id, |current| {
            existed = current.is_some();
            Ok(None)
        })?;
        Ok(existed)
    }

    /// Read-modify-write one entry under the cross-process lock.
    ///
    /// `update` sees the credential as it is *right now* — re-read inside the lock, so a decision
    /// like "is this token still expiring?" is made against the truth rather than a stale copy — and
    /// returns the replacement, or `None` to delete the entry.
    pub fn modify<F>(&self, provider_id: &str, update: F) -> Result<Option<Credential>, StoreError>
    where
        F: FnOnce(Option<Credential>) -> Result<Option<Credential>, StoreError>,
    {
        if RESERVED_KEYS.contains(&provider_id) {
            return Err(StoreError::Rejected(format!(
                "'{provider_id}' is a reserved config key, not a provider"
            )));
        }
        self.file.modify(|root| {
            let current = root.get(provider_id).and_then(Credential::from_json);
            let next = update(current)?;
            match &next {
                Some(credential) => root.insert(provider_id.to_string(), credential.to_json()),
                None => root.remove(provider_id),
            };
            Ok(next)
        })
    }
}

#[cfg(test)]
mod tests;
