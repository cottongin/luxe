//! Read tracker — "have you seen this file, and is it still current?" state.
//!
//! Backs the `edit`/`write` safety guards: an edit must follow a read (read-before-edit), and the
//! file must not have changed on disk since that read (staleness). Shared via an `Arc` across the
//! `read`/`edit`/`write` tools, the LSP funnel reads (`read_symbol`, `module_report`), `grep`, and
//! `bash`, which records a file a plain `cat`, `head`, `tail`, `sed -n` or search printed, keyed by
//! `(session_id, canonical path)` so concurrent sessions in one process stay isolated. It lives in
//! `core` (not `luxe-tools`) precisely so `luxe-lsp` — which can't depend on `luxe-tools` — can
//! share the same instance. Fingerprint is `(mtime, size)` — a cheap stat, no content hashing.
//! When no tracker is wired (e.g. unit tests), the guards simply don't run.
//!
//! **The record is per process, not per transcript.** A resumed session restores the conversation
//! but not this map, so a file read before the restart is `Unseen` again — deliberately, because
//! the fingerprint that made the earlier read trustworthy is gone with it. Seeding from the
//! transcript would re-record whatever is on disk *now* and call it "seen", which is precisely the
//! blind mutation the guard exists to prevent. The tools' rejections say so, so the model re-reads
//! instead of concluding the guard is broken.
//!
//! **What this does not prove.** A record means "you read this path", not "you read the region you
//! are about to change": a one-line `read` with `offset`/`limit` satisfies it. That is on purpose —
//! reading only the region you intend to edit is normal, and demanding a whole-file read would
//! punish it. The protection against editing text you have not actually seen is `edit`'s
//! exact-unique-match requirement, with staleness catching the file moving underneath you.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

/// The spelling a path is recorded under, so every spelling of one file is one record.
///
/// [`Path::canonicalize`] is the whole answer while the file exists: it resolves `.` and `..`, a
/// symlinked directory, and the file's own symlink, so reading `src/a.rs` and editing
/// `/proj/./src/a.rs` are the same record rather than two.
///
/// It fails the moment the file does not, and the raw path is the wrong fallback: a record has to
/// outlive the file it describes. Deleting a recorded file and then editing it through a
/// non-canonical spelling — a project reached through a symlinked directory, which is every path
/// under a macOS temp dir — would land on a different key and report a file that vanished as one
/// that was never read. Canonicalizing the *parent* keeps the two spellings together, and the
/// full-path attempt comes first because only it can resolve a symlinked file.
///
/// Never wrong in the permissive direction either way: a key that misses reads as `Unseen`, and
/// every guard treats that as "not safe to mutate".
fn identity(path: &Path) -> PathBuf {
    if let Ok(resolved) = path.canonicalize() {
        return resolved;
    }
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => match parent.canonicalize() {
            Ok(parent) => parent.join(name),
            Err(_) => path.to_path_buf(),
        },
        _ => path.to_path_buf(),
    }
}

/// Cheap file identity: modification time + byte length.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Fingerprint {
    mtime: Option<SystemTime>,
    size: u64,
}

/// A file's state relative to what the model last read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadStatus {
    /// The file was never read this session (or the tracker has no record of it).
    Unseen,
    /// The file was read, but its on-disk contents changed since (formatter, external edit, …).
    Stale,
    /// The file matches what was last read/written — safe to mutate.
    Current,
}

/// Records which files a session has read and detects out-of-band changes.
#[derive(Default)]
pub struct ReadTracker {
    seen: Mutex<HashMap<(String, PathBuf), Fingerprint>>,
}

impl ReadTracker {
    /// Create an empty tracker.
    pub fn new() -> Self {
        Self::default()
    }

    fn key(session: &str, path: &Path) -> (String, PathBuf) {
        (session.to_string(), identity(path))
    }

    fn fingerprint(path: &Path) -> Option<Fingerprint> {
        let meta = std::fs::metadata(path).ok()?;
        Some(Fingerprint { mtime: meta.modified().ok(), size: meta.len() })
    }

    /// Records the current on-disk fingerprint for a path — call after a successful read or write.
    ///
    /// A no-op if the file can't be stat'd (e.g. it was just deleted); the next `status` then
    /// reports `Unseen`/`Stale` rather than a false `Current`.
    pub fn record(&self, session: &str, path: &Path) {
        if let Some(fingerprint) = Self::fingerprint(path) {
            if let Ok(mut seen) = self.seen.lock() {
                seen.insert(Self::key(session, path), fingerprint);
            }
        }
    }

    /// Reports whether a path is safe to mutate: unseen, stale, or current.
    pub fn status(&self, session: &str, path: &Path) -> ReadStatus {
        let recorded =
            self.seen.lock().ok().and_then(|seen| seen.get(&Self::key(session, path)).copied());
        match recorded {
            None => ReadStatus::Unseen,
            Some(recorded) => match Self::fingerprint(path) {
                Some(current) if current == recorded => ReadStatus::Current,
                _ => ReadStatus::Stale,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unseen_then_current_then_stale() {
        let dir = std::env::temp_dir().join(format!("luxe-readtracker-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("f.txt");
        std::fs::write(&file, "one").unwrap();

        let tracker = ReadTracker::new();
        assert_eq!(tracker.status("s", &file), ReadStatus::Unseen);

        tracker.record("s", &file);
        assert_eq!(tracker.status("s", &file), ReadStatus::Current);

        // A different session hasn't seen it.
        assert_eq!(tracker.status("other", &file), ReadStatus::Unseen);

        // Change on disk (size differs) → stale. Sleep briefly so mtime can differ too.
        std::thread::sleep(std::time::Duration::from_millis(5));
        std::fs::write(&file, "one-two-three").unwrap();
        assert_eq!(tracker.status("s", &file), ReadStatus::Stale);

        // Re-record clears staleness.
        tracker.record("s", &file);
        assert_eq!(tracker.status("s", &file), ReadStatus::Current);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// One file, several spellings, one record — including after it is deleted.
    ///
    /// The symlinked directory is the case that matters: a key from `canonicalize` alone fails once
    /// the file is gone, so a deleted file asked about through the link would come back `Unseen` —
    /// "you never read this" — for a file that had been read. Both answers refuse the edit; the
    /// model should be told the right reason.
    #[cfg(unix)]
    #[test]
    fn every_spelling_of_one_file_is_one_record_even_after_it_is_deleted() {
        let dir = tempfile::tempdir().expect("temp dir");
        let real = dir.path().join("real");
        std::fs::create_dir_all(&real).expect("real dir");
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).expect("symlink");

        let via_link = link.join("a.rs");
        let via_real = real.join("a.rs");
        std::fs::write(&via_link, "x").expect("seed");

        let tracker = ReadTracker::new();
        tracker.record("s", &via_link);
        assert_eq!(tracker.status("s", &via_link), ReadStatus::Current);
        assert_eq!(
            tracker.status("s", &via_real),
            ReadStatus::Current,
            "the real path is the same file, and canonicalize is what proves it"
        );
        assert_eq!(
            tracker.status("s", &link.join(".").join("a.rs")),
            ReadStatus::Current,
            "and so is a dotted spelling of it"
        );

        std::fs::remove_file(&via_real).expect("delete");
        for spelling in [&via_link, &via_real] {
            assert_eq!(
                tracker.status("s", spelling),
                ReadStatus::Stale,
                "a file that was read and then vanished changed; it is not one nobody read"
            );
        }
    }
}
