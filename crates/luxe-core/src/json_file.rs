//! JSON files — lossless, locked, atomic read-modify-write for a JSON object file.
//!
//! The provider credential store, the config and permissions writers and the MCP token store all
//! edit a file a human owns, which sets the rules: never lose a key we don't understand, never
//! leave a half-written file behind, and never race another Luxe process (a token refresh in one
//! and a `/login` in another).
//!
//! The lock is a directory. `create_dir` is atomic everywhere Luxe runs and needs no dependency; a
//! lock older than `LOCK_STALE` is assumed to belong to a killed process and is broken, because
//! refusing to ever write again is the worse failure.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::{Map, Value};

/// How long a lock directory may sit before it's treated as abandoned.
const LOCK_STALE: Duration = Duration::from_secs(30);

/// Longest a writer waits for the lock before giving up.
const LOCK_TIMEOUT: Duration = Duration::from_secs(5);

/// Something went wrong reading or writing a JSON config file.
#[derive(Debug, thiserror::Error)]
pub enum FileError {
    /// Filesystem failure (permissions, missing parent, full disk).
    ///
    /// The OS error is part of the message and not also the `source()`, so a caller that prints
    /// the chain says it once.
    #[error("{context}: {error}")]
    Io {
        /// What was being attempted
        context: String,
        /// What the filesystem said
        error: std::io::Error,
    },
    /// The file exists but isn't a JSON object, or isn't JSON at all.
    #[error("{} is {reason} — fix or move it, then try again", path.display())]
    Unparseable {
        /// The file
        path: PathBuf,
        /// What is wrong with it: `not a JSON object`, or `not valid JSON (<parser message>)`
        reason: String,
    },
    /// Another process held the lock for too long.
    #[error("timed out waiting for a lock on {0}")]
    LockTimeout(PathBuf),
    /// The caller's own error, passed through `modify`.
    #[error("{0}")]
    Rejected(String),
}

impl FileError {
    /// An IO failure with the operation named.
    pub fn io(context: impl Into<String>, error: std::io::Error) -> Self {
        Self::Io { context: context.into(), error }
    }
}

/// How strictly a file's permissions are enforced on write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileMode {
    /// Always `0600` — the file holds secrets.
    OwnerOnly,
    /// `0600` when creating it, but keep whatever mode an existing file has. For a file a user
    /// maintains by hand, silently tightening their chosen permissions is rude; starting tight is
    /// not.
    PreserveExisting,
}

/// A JSON object file that is edited in place without losing anything.
#[derive(Debug, Clone)]
pub struct JsonFile {
    path: PathBuf,
    mode: FileMode,
}

impl JsonFile {
    /// A file at `path`, written under `mode`.
    pub fn new(path: PathBuf, mode: FileMode) -> Self {
        Self { path, mode }
    }

    /// The path being edited.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Parse as a JSON object, treating an absent, empty, or malformed file as "nothing yet".
    ///
    /// Reads are forgiving on purpose: a syntax error in a config file must not stop Luxe from
    /// starting, only from writing.
    pub fn read(&self) -> Option<Map<String, Value>> {
        let text = std::fs::read_to_string(&self.path).ok()?;
        if text.trim().is_empty() {
            return None;
        }
        match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(map)) => Some(map),
            Ok(_) => {
                tracing::warn!(path = %self.path.display(), "config file is not a JSON object");
                None
            }
            Err(error) => {
                tracing::warn!(path = %self.path.display(), %error, "config file is not valid JSON");
                None
            }
        }
    }

    /// Read-modify-write under the lock. `edit` mutates the object in place.
    ///
    /// A malformed existing file is an error rather than an empty start, so a write can never
    /// discard a file it merely failed to parse.
    pub fn modify<T, F>(&self, edit: F) -> Result<T, FileError>
    where
        F: FnOnce(&mut Map<String, Value>) -> Result<T, FileError>,
    {
        self.ensure_parent()?;
        let _lock = FileLock::acquire(&self.path)?;
        let mut root = self.read_strict()?.unwrap_or_default();
        let outcome = edit(&mut root)?;
        self.write(&root)?;
        Ok(outcome)
    }

    /// [`Self::modify`] for a file Luxe writes for itself, where one that does not parse is moved
    /// aside rather than refused: to `<name>.bad`, or `<name>.bad-<millis>` when that is taken, so an
    /// earlier one survives. The move happens under the lock, so two writers meeting the same broken
    /// file cannot set each other's work aside. Returns where the broken file went.
    pub fn modify_setting_aside<T, F>(&self, edit: F) -> Result<(T, Option<PathBuf>), FileError>
    where
        F: FnOnce(&mut Map<String, Value>) -> Result<T, FileError>,
    {
        self.ensure_parent()?;
        let _lock = FileLock::acquire(&self.path)?;
        let (mut root, aside) = match self.read_strict() {
            Ok(root) => (root.unwrap_or_default(), None),
            Err(FileError::Unparseable { .. }) => (Map::new(), Some(self.set_aside()?)),
            Err(error) => return Err(error),
        };
        let outcome = edit(&mut root)?;
        self.write(&root)?;
        Ok((outcome, aside))
    }

    fn set_aside(&self) -> Result<PathBuf, FileError> {
        let name = self.path.file_name().unwrap_or_default().to_string_lossy();
        let mut aside = self.path.with_file_name(format!("{name}.bad"));
        if aside.exists() {
            let millis = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_millis())
                .unwrap_or_default();
            aside = self.path.with_file_name(format!("{name}.bad-{millis}"));
        }
        std::fs::rename(&self.path, &aside).map_err(|error| {
            FileError::io(format!("moving {} aside", self.path.display()), error)
        })?;
        Ok(aside)
    }

    fn read_strict(&self) -> Result<Option<Map<String, Value>>, FileError> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(FileError::io(format!("reading {}", self.path.display()), error))
            }
        };
        if text.trim().is_empty() {
            return Ok(None);
        }
        match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(map)) => Ok(Some(map)),
            Ok(_) => Err(FileError::Unparseable {
                path: self.path.clone(),
                reason: "not a JSON object".into(),
            }),
            Err(error) => Err(FileError::Unparseable {
                path: self.path.clone(),
                reason: format!("not valid JSON ({error})"),
            }),
        }
    }

    fn ensure_parent(&self) -> Result<(), FileError> {
        let Some(parent) = self.path.parent() else { return Ok(()) };
        if parent.as_os_str().is_empty() || parent.exists() {
            return Ok(());
        }
        crate::fs_atomic::create_private_dir(parent)
            .map_err(|error| FileError::io(format!("creating {}", parent.display()), error))
    }

    /// Write the whole object atomically, so a reader never sees a partial file.
    ///
    /// A `PreserveExisting` file keeps whatever mode the user gave it; a fresh one still starts at
    /// `0600`, because a config file that will later hold a key must never be world-readable for the
    /// window between creation and the user noticing.
    fn write(&self, root: &Map<String, Value>) -> Result<(), FileError> {
        let mut json = serde_json::to_string_pretty(root).map_err(|error| {
            FileError::io(
                "serializing config",
                std::io::Error::new(std::io::ErrorKind::InvalidData, error),
            )
        })?;
        json.push('\n');

        let mode = match self.mode {
            FileMode::OwnerOnly => None,
            FileMode::PreserveExisting => crate::fs_atomic::existing_mode(&self.path),
        };
        crate::fs_atomic::replace_with_mode(&self.path, json.as_bytes(), mode)
            .map_err(|error| FileError::io(format!("replacing {}", self.path.display()), error))
    }
}

/// A cross-process mutex built from an atomically-created directory.
pub struct FileLock {
    path: PathBuf,
}

impl FileLock {
    /// Take the lock for `target`, waiting up to `LOCK_TIMEOUT`.
    pub fn acquire(target: &Path) -> Result<Self, FileError> {
        let path = target.with_extension("lock");
        let deadline = SystemTime::now() + LOCK_TIMEOUT;
        loop {
            let error = match std::fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(error) => error,
            };
            let held = error.kind() == std::io::ErrorKind::AlreadyExists;
            if held && lock_is_stale(&path) {
                tracing::warn!(lock = %path.display(), "breaking a stale config lock");
                let _ = std::fs::remove_dir(&path);
                continue;
            }
            let past_deadline = SystemTime::now() >= deadline;
            match lock_wait(error.kind(), cfg!(windows)) {
                LockWait::Wait if !past_deadline => std::thread::sleep(Duration::from_millis(25)),
                LockWait::Wait if held => return Err(FileError::LockTimeout(path)),
                _ => return Err(FileError::io(format!("locking {}", path.display()), error)),
            }
        }
    }
}

/// What a failed `create_dir` of the lock means for the writer.
#[derive(Debug, PartialEq, Eq)]
enum LockWait {
    /// Another writer has it, or is a moment from letting it go
    Wait,
    /// Waiting cannot help
    Fail,
}

/// Whether a failed attempt at the lock is worth waiting out.
///
/// On Windows a directory being removed lingers, delete-pending, until its last handle closes, and
/// creating it then fails with access denied rather than already exists. That is the previous
/// holder letting go, so it waits too; a real permission problem still surfaces, as itself, once
/// the deadline passes. Elsewhere access denied is only ever the real thing.
fn lock_wait(kind: std::io::ErrorKind, windows: bool) -> LockWait {
    match kind {
        std::io::ErrorKind::AlreadyExists => LockWait::Wait,
        std::io::ErrorKind::PermissionDenied if windows => LockWait::Wait,
        _ => LockWait::Fail,
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir(&self.path);
    }
}

fn lock_is_stale(path: &Path) -> bool {
    let Ok(modified) = std::fs::metadata(path).and_then(|meta| meta.modified()) else {
        // No usable timestamp: treat it as fresh rather than stomping a live lock.
        return false;
    };
    SystemTime::now().duration_since(modified).is_ok_and(|age| age > LOCK_STALE)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;
