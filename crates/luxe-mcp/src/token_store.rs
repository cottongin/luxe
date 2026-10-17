//! OAuth token store — per-server tokens persisted with `0600` perms.
//!
//! Access and refresh tokens for hosted MCP servers live in the **state** directory —
//! `$XDG_STATE_HOME/luxe/mcp-auth.json`, `~/.local/state/…`, or `%LOCALAPPDATA%` on Windows —
//! readable only by the owner. They are machine-written and refreshed every hour, which is state by
//! the line `runtime_state` draws, and the config directory is the wrong home for it twice over:
//! it is what people keep in a dotfiles repository, and on Windows it is `%APPDATA%`, which roams
//! between machines. A store found in the config-directory location is moved on first read.
//!
//! Tokens are zeroized on drop, redacted from `Debug`, and never logged.

use std::collections::HashMap;
use std::path::PathBuf;

use luxe_core::json_file::{FileError, FileMode, JsonFile};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use zeroize::Zeroize;

/// Why the token store could not be written.
#[derive(Debug, thiserror::Error)]
pub enum TokenStoreError {
    /// A token that would not serialize
    #[error(transparent)]
    Encode(#[from] serde_json::Error),
    /// The file could not be locked, read or replaced
    #[error(transparent)]
    File(#[from] FileError),
}

/// A stored token set for one server. `Zeroize` clears the secret material on drop.
#[derive(Clone, Serialize, Deserialize, Default)]
pub struct StoredToken {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// Unix seconds when the access token expires (`None` = no known expiry).
    #[serde(default)]
    pub expires_at_secs: Option<u64>,
    /// The (possibly dynamically-registered) client id this token belongs to.
    #[serde(default)]
    pub client_id: Option<String>,
    /// A confidential client's secret, when one was issued at registration. A configured client's
    /// secret is never copied here: it stays in the config, or the variable the config names.
    #[serde(default)]
    pub client_secret: Option<String>,
    /// The token endpoint that issued it, so a refresh goes back to the same place
    #[serde(default)]
    pub token_endpoint: Option<String>,
    /// The resource indicator it was issued for (RFC 8707)
    #[serde(default)]
    pub resource: Option<String>,
}

impl std::fmt::Debug for StoredToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let shown = |present: bool| if present { "[redacted]" } else { "none" };
        formatter
            .debug_struct("StoredToken")
            .field("access_token", &shown(!self.access_token.is_empty()))
            .field("refresh_token", &shown(self.refresh_token.is_some()))
            .field("expires_at_secs", &self.expires_at_secs)
            .field("client_id", &self.client_id)
            .field("client_secret", &shown(self.client_secret.is_some()))
            .field("token_endpoint", &self.token_endpoint)
            .field("resource", &self.resource)
            .finish()
    }
}

impl StoredToken {
    /// Whether the access token is expired (or within a 60s safety margin).
    pub fn is_expired(&self) -> bool {
        match self.expires_at_secs {
            Some(exp) => now_secs() + 60 >= exp,
            None => false,
        }
    }
}

impl Drop for StoredToken {
    fn drop(&mut self) {
        self.access_token.zeroize();
        if let Some(t) = self.refresh_token.as_mut() {
            t.zeroize();
        }
        if let Some(s) = self.client_secret.as_mut() {
            s.zeroize();
        }
    }
}

/// File-backed map of `server_id -> StoredToken`.
pub struct TokenStore {
    path: PathBuf,
    /// Where a store written before the move lives, read once and removed
    legacy: Option<PathBuf>,
}

impl TokenStore {
    /// Store at an explicit path.
    pub fn new(path: PathBuf) -> Self {
        Self { path, legacy: None }
    }

    /// Store at `path`, moving one found at `legacy` into it on first read.
    pub fn migrating(path: PathBuf, legacy: PathBuf) -> Self {
        Self { path, legacy: Some(legacy) }
    }

    /// The default store in the state directory, taking over one left in the config directory.
    pub fn default_path() -> Self {
        Self::migrating(
            luxe_core::user_dirs::luxe_state_dir().join("mcp-auth.json"),
            luxe_core::user_dirs::luxe_config_dir().join("mcp-auth.json"),
        )
    }

    /// Where the store is written.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Load the token for one server.
    pub fn get(&self, server_id: &str) -> Option<StoredToken> {
        self.load().remove(server_id)
    }

    /// The servers a token is stored for, read without migrating and without holding one secret.
    ///
    /// For a status line that is redrawn on every snapshot: it must not write — the migration in
    /// [`Self::get`] is for a sign-in or a call — and has no business decoding a token to count it,
    /// so the values are skipped unparsed. Stored is not valid: an expired token with no refresh
    /// still counts, which is why the word the tab uses is "signed in" and not "working".
    pub fn signed_in_ids(&self) -> std::collections::HashSet<String> {
        let ids = |path: &std::path::Path| {
            Some(file(path).read()?.into_iter().map(|(id, _)| id).collect())
        };
        ids(&self.path).or_else(|| self.legacy.as_deref().and_then(ids)).unwrap_or_default()
    }

    /// Insert/replace the token for one server and persist (`0600`).
    pub fn put(&self, server_id: &str, token: StoredToken) -> Result<(), TokenStoreError> {
        let value = serde_json::to_value(&token)?;
        self.modify(|all| {
            all.insert(server_id.to_string(), value.clone());
        })
    }

    /// Remove one server's token (e.g. on logout).
    pub fn remove(&self, server_id: &str) -> Result<(), TokenStoreError> {
        self.modify(|all| {
            all.remove(server_id);
        })
    }

    /// Change the store under its lock, keeping every entry this version cannot read as written.
    ///
    /// A file that does not parse at all is moved aside with a warning, never written over:
    /// emptying it would sign the reader out of every server for one bad edit.
    fn modify(&self, edit: impl FnOnce(&mut Map<String, Value>)) -> Result<(), TokenStoreError> {
        let ((), aside) = file(&self.path).modify_setting_aside(|all| {
            edit(all);
            Ok(())
        })?;
        if let Some(aside) = aside {
            tracing::warn!(kept = %aside.display(), "mcp: the token store did not parse; moved it aside");
        }
        Ok(())
    }

    /// Every server's token that parses. When only the config-directory location has a store, it is
    /// written to the state directory and the old file removed — only after the write, so a failure
    /// loses nothing. With both present the new one wins and the old is left for the reader to
    /// inspect.
    fn load(&self) -> HashMap<String, StoredToken> {
        if let Some(all) = read(&self.path) {
            return tokens(all);
        }
        let Some(legacy) = self.legacy.as_ref() else {
            return HashMap::new();
        };
        let Some(all) = read(legacy) else {
            return HashMap::new();
        };
        match self.modify(|stored| stored.extend(all.clone())) {
            Ok(()) => {
                if let Err(error) = std::fs::remove_file(legacy) {
                    tracing::warn!(%error, path = %legacy.display(), "mcp: could not remove the old token store");
                } else {
                    tracing::info!(to = %self.path.display(), "mcp: moved the token store to the state directory");
                }
            }
            Err(error) => {
                tracing::warn!(%error, "mcp: could not move the token store; reading it in place")
            }
        }
        tokens(all)
    }
}

/// The store at `path`: one JSON object, owner-only, edited under a cross-process lock.
fn file(path: &std::path::Path) -> JsonFile {
    JsonFile::new(path.to_path_buf(), FileMode::OwnerOnly)
}

/// The store's object, or `None` when it is missing or does not parse. Quiet, since it runs on
/// every request and every status redraw; the write that moves a broken file aside is what warns.
fn read(path: &std::path::Path) -> Option<Map<String, Value>> {
    let text = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str(&text) {
        Ok(all) => Some(all),
        Err(error) => {
            tracing::debug!(path = %path.display(), %error, "mcp: the token store does not parse");
            None
        }
    }
}

/// The entries that parse as tokens; one that does not is skipped, not allowed to hide the rest.
fn tokens(all: Map<String, Value>) -> HashMap<String, StoredToken> {
    all.into_iter()
        .filter_map(|(id, value)| match serde_json::from_value(value) {
            Ok(token) => Some((id, token)),
            Err(error) => {
                tracing::debug!(server = %id, %error, "mcp: a stored token does not parse; skipping it");
                None
            }
        })
        .collect()
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests;
