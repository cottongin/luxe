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
            match std::fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if lock_is_stale(&path) {
                        tracing::warn!(lock = %path.display(), "breaking a stale config lock");
                        let _ = std::fs::remove_dir(&path);
                        continue;
                    }
                    if SystemTime::now() >= deadline {
                        return Err(FileError::LockTimeout(path));
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(error) => {
                    return Err(FileError::io(format!("locking {}", path.display()), error))
                }
            }
        }
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
mod tests {
    use super::*;

    /// A broken file is moved aside under the lock, and a later broken file does not overwrite the
    /// first one set aside.
    #[test]
    fn a_broken_file_is_set_aside_and_never_over_an_earlier_one() {
        let (dir, file) = file(FileMode::OwnerOnly);
        for broken in ["{ first", "{ second"] {
            std::fs::write(file.path(), broken).unwrap();
            let ((), aside) = file
                .modify_setting_aside(|root| {
                    root.insert("fresh".into(), Value::Bool(true));
                    Ok(())
                })
                .unwrap();
            let aside = aside.expect("the broken file was moved");
            assert_eq!(std::fs::read_to_string(&aside).unwrap(), broken);
        }
        assert_eq!(std::fs::read_to_string(dir.path().join("thing.json.bad")).unwrap(), "{ first");
        assert_eq!(file.read().unwrap().get("fresh"), Some(&Value::Bool(true)));
    }

    /// The OS error is said once, whether a caller prints the message alone or walks the chain.
    #[test]
    fn an_io_failure_names_its_cause_once() {
        let error = FileError::io("reading thing.json", std::io::Error::other("disk on fire"));
        assert_eq!(error.to_string(), "reading thing.json: disk on fire");
        let chain = crate::error_cause::chain(&error);
        assert_eq!(chain.matches("disk on fire").count(), 1, "{chain}");
    }

    fn file(mode: FileMode) -> (tempfile::TempDir, JsonFile) {
        let dir = tempfile::tempdir().unwrap();
        let file = JsonFile::new(dir.path().join("thing.json"), mode);
        (dir, file)
    }

    #[test]
    fn an_absent_or_empty_file_reads_as_nothing() {
        let (_dir, file) = file(FileMode::OwnerOnly);
        assert!(file.read().is_none());
        std::fs::write(file.path(), "  \n").unwrap();
        assert!(file.read().is_none());
    }

    #[test]
    fn modify_preserves_every_key_it_did_not_touch() {
        let (_dir, file) = file(FileMode::OwnerOnly);
        std::fs::write(file.path(), r#"{"keep": {"deep": 1}, "also": "me"}"#).unwrap();
        file.modify(|root| {
            root.insert("added".into(), Value::Bool(true));
            Ok(())
        })
        .unwrap();
        let root = file.read().unwrap();
        assert_eq!(root["keep"]["deep"], 1);
        assert_eq!(root["also"], "me");
        assert_eq!(root["added"], true);
    }

    #[test]
    fn a_malformed_file_blocks_the_write_and_is_left_alone() {
        let (_dir, file) = file(FileMode::OwnerOnly);
        std::fs::write(file.path(), "{ nope").unwrap();
        let error = file
            .modify(|root| {
                root.insert("added".into(), Value::Bool(true));
                Ok(())
            })
            .expect_err("must refuse");
        assert!(matches!(error, FileError::Unparseable { .. }), "got {error:?}");
        assert_eq!(std::fs::read_to_string(file.path()).unwrap(), "{ nope");
    }

    #[test]
    fn a_rejecting_edit_leaves_the_file_untouched() {
        let (_dir, file) = file(FileMode::OwnerOnly);
        std::fs::write(file.path(), r#"{"a": 1}"#).unwrap();
        let error = file
            .modify(|_| Err::<(), _>(FileError::Rejected("no".into())))
            .expect_err("propagates");
        assert!(matches!(error, FileError::Rejected(_)));
        assert_eq!(file.read().unwrap()["a"], 1);
        assert!(!file.path().with_extension("lock").exists(), "the lock must be released");
    }

    #[cfg(unix)]
    #[test]
    fn owner_only_mode_is_enforced_and_preserve_mode_respects_the_user() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, secret) = file(FileMode::OwnerOnly);
        secret.modify(|root| Ok(root.insert("k".into(), Value::Bool(true)))).unwrap();
        assert_eq!(crate::fs_atomic::existing_mode(secret.path()), Some(0o600));

        let dir = tempfile::tempdir().unwrap();
        let config = JsonFile::new(dir.path().join("models.json"), FileMode::PreserveExisting);
        // A fresh file still starts tight.
        config.modify(|root| Ok(root.insert("a".into(), Value::Bool(true)))).unwrap();
        assert_eq!(crate::fs_atomic::existing_mode(config.path()), Some(0o600));
        // A mode the user chose survives a rewrite.
        std::fs::set_permissions(config.path(), std::fs::Permissions::from_mode(0o644)).unwrap();
        config.modify(|root| Ok(root.insert("b".into(), Value::Bool(true)))).unwrap();
        assert_eq!(crate::fs_atomic::existing_mode(config.path()), Some(0o644));
    }

    #[test]
    fn a_stale_lock_is_broken_instead_of_blocking_forever() {
        let (_dir, file) = file(FileMode::OwnerOnly);
        let lock = file.path().with_extension("lock");
        std::fs::create_dir(&lock).unwrap();
        backdate(&lock, LOCK_STALE + Duration::from_secs(5));
        file.modify(|root| Ok(root.insert("a".into(), Value::Bool(true)))).unwrap();
        assert!(file.read().is_some());
    }

    #[test]
    fn a_fresh_lock_makes_a_writer_time_out_rather_than_corrupt() {
        let (_dir, file) = file(FileMode::OwnerOnly);
        let lock = file.path().with_extension("lock");
        std::fs::create_dir(&lock).unwrap();
        let error = file.modify(|_| Ok(())).expect_err("must not write through a live lock");
        assert!(matches!(error, FileError::LockTimeout(_)), "got {error:?}");
        std::fs::remove_dir(&lock).unwrap();
    }

    /// Set an mtime in the past.
    ///
    /// Not `touch`: `touch -d @<epoch>` is a GNU extension BSD rejects. Not `std` either, because
    /// the lock is a *directory*, and opening one to set its times needs `FILE_FLAG_BACKUP_SEMANTICS`
    /// on Windows, so it has to work there or the test it serves asserts that nothing happens.
    fn backdate(path: &Path, by: Duration) {
        let when = filetime::FileTime::from_system_time(SystemTime::now() - by);
        filetime::set_file_mtime(path, when).expect("set mtime");
    }
}
