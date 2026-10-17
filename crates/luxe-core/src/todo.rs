//! Todo store — the durable checklist behind the `todo` tool.
//!
//! One JSON file per session at `.luxe/todos/<session-id>.json`, so a list survives compaction,
//! restart and resume, and two Luxe instances open on the same repository can never write each
//! other's list.
//!
//! The session **is** the scope: no named envelope around it, and no index mapping one to the
//! other, because the session id already answers both questions. Durable prose is not kept here
//! either — a plan or a finding is a file in the repository, where the next reader will find it.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Error type for todo store operations.
#[derive(Debug, Error)]
pub enum TodoError {
    /// A session id that cannot be a filename — see `session_file_stem`.
    #[error("unusable session id: {0:?}")]
    BadSessionId(String),

    /// The checklist could not be read from or written to disk.
    #[error("IO error: {0}")]
    Io(std::io::Error),

    /// The stored checklist is not the JSON this build expects.
    #[error("JSON error: {0}")]
    Json(serde_json::Error),
}

crate::error_cause::from_cause!(TodoError {
    std::io::Error => Io,
    serde_json::Error => Json,
});

/// One item on a session's checklist.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Todo {
    /// Unique id within the session.
    pub id: u32,
    /// Short subject line.
    pub subject: String,
    /// Current status.
    pub status: TodoStatus,
    /// Present-continuous label for the spinner (optional).
    pub active_form: Option<String>,
    /// Ids this item is blocked by.
    pub blocked_by: Vec<u32>,
}

impl Todo {
    /// The blockers still standing: the ids in `blocked_by` that are not done. A blocker that was
    /// completed, deleted or never existed holds nothing up, which is what the `todo` tool's own
    /// list says; anything else reading the checklist asks here so it says the same.
    pub fn open_blockers(&self, all: &[Todo]) -> Vec<u32> {
        self.blocked_by
            .iter()
            .copied()
            .filter(|id| all.iter().any(|todo| todo.id == *id && todo.status.is_open()))
            .collect()
    }
}

/// Todo status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    #[default]
    Pending,
    InProgress,
    Completed,
    Deleted,
}

impl TodoStatus {
    /// Whether the item still represents outstanding work.
    pub fn is_open(self) -> bool {
        matches!(self, Self::Pending | Self::InProgress)
    }

    /// Whether the item still counts toward the list at all (a tombstone does not).
    pub fn is_live(self) -> bool {
        self != Self::Deleted
    }
}

/// The todo store — one checklist per session, on disk.
#[derive(Debug)]
pub struct TodoStore {
    /// Root directory holding the per-session files (e.g. `.luxe/todos`).
    root: PathBuf,
}

impl TodoStore {
    /// Create a store rooted at `root`. The directory is created on first write.
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Where a session's checklist lives.
    pub fn path_for(&self, session_id: &str) -> Result<PathBuf, TodoError> {
        Ok(self.root.join(format!("{}.json", session_file_stem(session_id)?)))
    }

    /// Read a session's checklist. A session that has never written one reads as empty.
    pub fn read(&self, session_id: &str) -> Result<Vec<Todo>, TodoError> {
        let path = self.path_for(session_id)?;
        match std::fs::read_to_string(&path) {
            Ok(body) => Ok(serde_json::from_str(&body)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(error) => Err(error.into()),
        }
    }

    /// Replace a session's checklist, atomically.
    pub fn write(&self, session_id: &str, todos: &[Todo]) -> Result<(), TodoError> {
        let path = self.path_for(session_id)?;
        std::fs::create_dir_all(&self.root)?;
        let mut bytes = serde_json::to_vec_pretty(todos)?;
        bytes.push(b'\n');
        crate::fs_atomic::replace(&path, &bytes)?;
        Ok(())
    }
}

/// The filename a session id maps to, rejecting anything that is not a plain name.
///
/// Session ids are UUIDs, but they reach here as strings from a tool call and from `--resume`, so
/// this is the choke point that keeps one out of a path it has no business writing. Nothing is
/// escaped or rewritten: a rejected id is a bug worth surfacing, not a filename to guess at.
fn session_file_stem(session_id: &str) -> Result<&str, TodoError> {
    let usable = !session_id.is_empty()
        && session_id != "."
        && session_id != ".."
        && !session_id.contains(['/', '\\', '\0'])
        && Path::new(session_id).components().count() == 1;
    usable.then_some(session_id).ok_or_else(|| TodoError::BadSessionId(session_id.to_string()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn todo(id: u32, subject: &str) -> Todo {
        Todo {
            id,
            subject: subject.to_string(),
            status: TodoStatus::Pending,
            active_form: None,
            blocked_by: Vec::new(),
        }
    }

    /// A blocker that is done, deleted or missing holds nothing up; one still open does.
    #[test]
    fn only_open_work_blocks() {
        let todo = |id, status, blocked_by: Vec<u32>| Todo {
            id,
            subject: format!("#{id}"),
            status,
            active_form: None,
            blocked_by,
        };
        let all = vec![
            todo(1, TodoStatus::Completed, vec![]),
            todo(2, TodoStatus::InProgress, vec![]),
            todo(3, TodoStatus::Deleted, vec![]),
            todo(4, TodoStatus::Pending, vec![1, 2, 3, 9]),
            todo(5, TodoStatus::Pending, vec![1]),
        ];
        assert_eq!(all[3].open_blockers(&all), [2]);
        assert!(all[4].open_blockers(&all).is_empty(), "its blocker is done");
    }

    #[test]
    fn an_unwritten_session_reads_as_an_empty_list() {
        let tmp = TempDir::new().unwrap();
        let store = TodoStore::new(tmp.path().join("todos"));

        assert!(store.read("session-a").unwrap().is_empty());
        // Reading must not conjure the directory: a session that never plans leaves no trace.
        assert!(!tmp.path().join("todos").exists());
    }

    #[test]
    fn todos_persist_across_instances() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("todos");

        {
            let store = TodoStore::new(root.clone());
            store
                .write(
                    "session-a",
                    &[
                        todo(1, "Do thing A"),
                        Todo {
                            status: TodoStatus::InProgress,
                            active_form: Some("doing thing B".into()),
                            blocked_by: vec![1],
                            ..todo(2, "Do thing B")
                        },
                    ],
                )
                .unwrap();
        }

        // A restart reads the same list back.
        let reopened = TodoStore::new(root).read("session-a").unwrap();
        assert_eq!(reopened.len(), 2);
        assert_eq!(reopened[0].subject, "Do thing A");
        assert_eq!(reopened[1].status, TodoStatus::InProgress);
        assert_eq!(reopened[1].blocked_by, vec![1]);
        assert_eq!(reopened[1].active_form.as_deref(), Some("doing thing B"));
    }

    /// Two sessions in one repository are two lists: a shared index file would have both write the
    /// same bytes and the loser's plan vanish.
    #[test]
    fn each_session_owns_its_own_list() {
        let tmp = TempDir::new().unwrap();
        let store = TodoStore::new(tmp.path().join("todos"));

        store.write("session-a", &[todo(1, "mine")]).unwrap();
        store.write("session-b", &[todo(1, "yours"), todo(2, "also yours")]).unwrap();

        assert_eq!(store.read("session-a").unwrap().len(), 1);
        assert_eq!(store.read("session-a").unwrap()[0].subject, "mine");
        assert_eq!(store.read("session-b").unwrap().len(), 2);
    }

    #[test]
    fn a_write_replaces_the_whole_list() {
        let tmp = TempDir::new().unwrap();
        let store = TodoStore::new(tmp.path().join("todos"));

        store.write("session-a", &[todo(1, "first"), todo(2, "second")]).unwrap();
        store.write("session-a", &[todo(1, "first")]).unwrap();

        let read = store.read("session-a").unwrap();
        assert_eq!(read.len(), 1, "the second write is the whole truth");
    }

    #[test]
    fn a_session_id_that_is_not_a_filename_is_rejected() {
        let tmp = TempDir::new().unwrap();
        let store = TodoStore::new(tmp.path().join("todos"));

        for hostile in ["", ".", "..", "../escape", "nested/id", "back\\slash"] {
            assert!(
                matches!(store.read(hostile), Err(TodoError::BadSessionId(_))),
                "read {hostile:?}"
            );
            assert!(
                matches!(store.write(hostile, &[]), Err(TodoError::BadSessionId(_))),
                "write {hostile:?}"
            );
        }

        // The shape session ids actually arrive in still works.
        assert!(store.read("4776e2d7-c0cf-4fa8-8d59-a5539526c023").is_ok());
    }

    #[test]
    fn status_answers_open_and_live_for_every_variant() {
        assert!(TodoStatus::Pending.is_open() && TodoStatus::Pending.is_live());
        assert!(TodoStatus::InProgress.is_open() && TodoStatus::InProgress.is_live());
        assert!(!TodoStatus::Completed.is_open() && TodoStatus::Completed.is_live());
        assert!(!TodoStatus::Deleted.is_open() && !TodoStatus::Deleted.is_live());
    }

    /// The file is machine-local, but a person opens it — and every other status enum that
    /// crosses a wire in this codebase is snake_case, so this one has no business being the
    /// exception.
    #[test]
    fn status_is_written_in_the_same_case_as_every_other_status_enum() {
        let tmp = TempDir::new().unwrap();
        let store = TodoStore::new(tmp.path().join("todos"));
        store
            .write("session-a", &[Todo { status: TodoStatus::InProgress, ..todo(1, "running") }])
            .unwrap();

        let raw = std::fs::read_to_string(store.path_for("session-a").unwrap()).unwrap();
        assert!(raw.contains("\"in_progress\""), "{raw}");
        assert_eq!(store.read("session-a").unwrap()[0].status, TodoStatus::InProgress);
    }
}
