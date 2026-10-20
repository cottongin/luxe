//! Session store — JSONL tree with append, replay, and branching.
//!
//! Every session is a tree of [`Entry`] nodes stored as one JSON object per line in a
//! `.jsonl` file. The active position is always a leaf; **resume = walk from the leaf
//! to the root and reverse**. `parent_id` exists from day one so branching/fork is
//! possible later, but no branching UI yet.
//!
//! Append-only: never rewrite prior lines. A branch is just a new entry whose
//! `parent_id` points at an older entry.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::event::ModelRef;
use crate::message::Message;

mod claim;
mod entry;
mod jsonl;
mod links;

pub use claim::SessionClaim;
pub use entry::{latest_leaf, Entry, EntryKind};
pub use jsonl::JsonlSessionStore;
pub use links::{linked_pull_requests, session_for_pull_request, session_issue, session_worktree};

/// A session identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub String);

impl SessionId {
    /// Generate a new session ID.
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// An entry identifier within a session.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EntryId(pub String);

impl EntryId {
    /// Generate a new entry ID.
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }
}

impl Default for EntryId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for EntryId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Errors that can occur when working with sessions.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("I/O error: {0}")]
    Io(io::Error),
    #[error("JSON serialization error: {0}")]
    JsonSerialize(serde_json::Error),
    #[error("JSON parse error at line {line}: {error}")]
    JsonParse { line: usize, error: serde_json::Error },
    #[error("session not found: {0}")]
    NotFound(SessionId),
    #[error("entry not found: {0}")]
    EntryNotFound(EntryId),
    #[error("session {0} is open in another Luxe")]
    OpenElsewhere(SessionId),
    #[error("{0}")]
    Other(String),
}

crate::error_cause::from_cause!(SessionError {
    io::Error => Io,
    serde_json::Error => JsonSerialize,
});

/// Trait for session storage backends.
pub trait SessionStore: Send + Sync {
    /// Create a new session in the given working directory.
    fn create(&mut self, cwd: &str) -> Result<SessionId, SessionError>;

    /// Append an entry to a session. Returns the new entry's ID.
    fn append(
        &mut self,
        session: &SessionId,
        parent: Option<EntryId>,
        kind: EntryKind,
    ) -> Result<EntryId, SessionError>;

    /// Load all entries for a session, in write order.
    fn load(&self, session: &SessionId) -> Result<Vec<Entry>, SessionError>;

    /// Walk from leaf to root and reverse — this is resume.
    fn active_path(&self, session: &SessionId, leaf: &EntryId) -> Result<Vec<Entry>, SessionError>;

    /// Lends the entries from the root to `leaf` to `visit`, borrowed rather than cloned.
    ///
    /// [`Self::active_path`] clones every entry on the path, tool output and the history a
    /// compaction replaced included, which on a long session is tens of megabytes a call. A caller
    /// that keeps a part of it borrows instead, through [`with_active_path`]. `visit` runs once,
    /// and may run under the store's lock, so it must not block.
    fn visit_active_path(
        &self,
        session: &SessionId,
        leaf: &EntryId,
        visit: &mut dyn FnMut(&[&Entry]),
    ) -> Result<(), SessionError>;

    /// Append a compaction summary as the session's new leaf, returning its id.
    ///
    /// `parent` is the entry this compaction *follows* — the current leaf — and `first_kept` names
    /// the oldest entry it did **not** summarize. They are different entries and the distinction is
    /// the whole feature: the summary is appended after the conversation, and what it replaced is
    /// skipped when the request is assembled
    /// ([`crate::compaction::context_entries`]), not deleted. Chaining it under `first_kept`
    /// instead forks the branch — the summary lands on a twig nothing points at, and every entry
    /// committed since is stranded on the other side of the fork.
    fn compact(
        &mut self,
        session: &SessionId,
        parent: Option<EntryId>,
        summary: &str,
        first_kept: &EntryId,
        tokens_before: u64,
    ) -> Result<EntryId, SessionError> {
        self.append(
            session,
            parent,
            EntryKind::Compaction {
                summary: summary.to_string(),
                first_kept: first_kept.clone(),
                tokens_before,
            },
        )
    }

    /// List all known session IDs (for the session picker).
    fn list_sessions(&self) -> Vec<SessionId>;

    /// The most recently modified session, if any.
    ///
    /// Used by `--continue` and dashboard "resume latest" to resume where the user left off.
    fn most_recent_session(&self) -> Option<SessionId>;

    /// All sessions sorted newest-first, capped at `limit`.
    ///
    /// Returns `(SessionId, mtime)` pairs so the dashboard can display age and sort consistently.
    fn recent_sessions(&self, limit: usize) -> Vec<(SessionId, SystemTime)>;

    /// Claim `session` for this process to write, so a second Luxe can't write it at once.
    ///
    /// [`SessionError::OpenElsewhere`] when another process holds it. A store other processes
    /// can't reach, like one in memory, has nothing to guard and returns `None`.
    fn claim(&self, session: &SessionId) -> Result<Option<SessionClaim>, SessionError> {
        let _ = session;
        Ok(None)
    }
}

/// What `read` makes of the active path to `leaf`, borrowing the entries rather than cloning
/// them: [`SessionStore::visit_active_path`] with a return value.
pub fn with_active_path<T>(
    store: &(impl SessionStore + ?Sized),
    session: &SessionId,
    leaf: &EntryId,
    read: impl FnOnce(&[&Entry]) -> T,
) -> Result<T, SessionError> {
    let mut read = Some(read);
    let mut result = None;
    store.visit_active_path(session, leaf, &mut |path| {
        result = read.take().map(|read| read(path));
    })?;
    result.ok_or_else(|| SessionError::Other("the session store never lent the path".to_string()))
}

#[cfg(test)]
mod tests;
