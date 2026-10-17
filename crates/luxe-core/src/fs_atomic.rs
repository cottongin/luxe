//! fs_atomic — replace a file's contents, durably, without a reader ever seeing half of it.
//!
//! [`replace_unsynced`] is the one exception to "durably", for regenerable caches written by the
//! thousand: atomic all the same, with the syncs skipped.
//!
//! One owner for "temp file, then rename", because every copy has to agree on the details that
//! matter: whether the bytes reach the device, whether the staging name can collide with a
//! concurrent writer, whether the file is created private or made private afterwards, and whether
//! the staging file is cleaned up when the rename fails.
//!
//! Sync the handle you *wrote through*, never a reopened one: `File::open(path)` then `sync_all()`
//! works on unix and fails on Windows with `Access is denied (os error 5)`, because
//! `FlushFileBuffers` needs a handle with write access and `File::open` asks for read.
//!
//! # What "durable" means here
//!
//! Write → `sync_all` the write handle → rename over the target → `sync` the directory. The last
//! step is unix-only: it is what makes the *rename* survive a power cut, and Windows has no
//! equivalent (a directory is not openable as a file), so there it is skipped rather than faked.
//!
//! # What "private" means here
//!
//! On unix, `0600` set at open time — a credential must not be world-readable for the instant
//! between the write and a later `set_permissions`. On Windows there is no mode to set: a file under
//! `%APPDATA%`/`%LOCALAPPDATA%` inherits the profile directory's ACL, which already grants only the
//! owner (plus SYSTEM and Administrators). That is the platform's answer, and it is the same one
//! git, gh and the cloud CLIs rely on.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Replace `path` with `bytes`, durably and atomically, leaving the platform's default permissions.
///
/// For files a human or another program is meant to read: project state, caches, task lists.
pub fn replace(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_then_rename(path, bytes, None)
}

/// Replace `path` with `bytes` the same way, but readable only by the owner.
///
/// For anything that is or could become a credential, plus the review artifacts, which carry diff
/// content from a project the user may not want world-readable.
pub fn replace_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_then_rename(path, bytes, Some(OWNER_ONLY))
}

/// Replace `path` with `bytes` at an explicit unix mode, set when the staging file is created.
///
/// For the one caller that must honor a mode the *user* chose (`models.json` left group-readable on
/// purpose) while still starting a fresh file tight. `mode` is a unix concept and is ignored on
/// Windows, where the containing directory's ACL is the access control.
pub fn replace_with_mode(path: &Path, bytes: &[u8], mode: Option<u32>) -> io::Result<()> {
    write_then_rename(path, bytes, mode.or(Some(OWNER_ONLY)))
}

/// Replace `path` keeping whatever mode the file already has, or owner-only when it is new.
///
/// For a config file that may hold a secret but that the user owns: a mode they chose stays theirs,
/// and a file Luxe creates starts tight.
pub fn replace_preserving_mode(path: &Path, bytes: &[u8]) -> io::Result<()> {
    replace_with_mode(path, bytes, existing_mode(path))
}

/// Replace `path` with `bytes` atomically but without syncing: a reader never sees half of it, and
/// a power cut may lose it.
///
/// For regenerable caches written by the thousand, where two syncs each would cost more than the
/// file is worth: losing one costs a recomputation, nothing more.
pub fn replace_unsynced(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_then_rename_with(path, bytes, None, false)
}

/// The permission bits of the file at `path` on unix, or `None` when it is missing — or always, on
/// Windows, where access is the directory's ACL rather than a mode.
pub fn existing_mode(path: &Path) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path).ok().map(|meta| meta.permissions().mode() & 0o777)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

/// Owner read/write, nothing else — the mode every credential and review artifact is written at.
pub const OWNER_ONLY: u32 = 0o600;

/// Create `path` and every missing parent, each owner-only on unix, and tighten `path` itself if
/// it was already there. A parent that already existed keeps its mode.
pub fn create_private_dir(path: &Path) -> io::Result<()> {
    let missing: Vec<&Path> = path.ancestors().take_while(|dir| !dir.exists()).collect();
    for dir in missing.into_iter().rev() {
        match create_private_dir_new(dir) {
            // another process made it first, which is as good
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists && dir.is_dir() => {}
            outcome => outcome?,
        }
    }
    restrict_dir(path)
}

/// Create `path`, owner-only from the moment it exists, refusing one that is already there: a
/// directory someone else made, in a shared place like the temp directory, may hold anything.
pub(crate) fn create_private_dir_new(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    let builder = {
        let mut builder = fs::DirBuilder::new();
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        builder
    };
    #[cfg(not(unix))]
    let builder = fs::DirBuilder::new();
    builder.create(path)?;
    // the umask may have taken bits from the mode; it cannot have added any
    restrict_dir(path)
}

/// Write `bytes` to `path` owner-only, flushed to the device — but *not* atomically.
///
/// The primitive under [`replace_private`], exposed for the callers that are already building a
/// staging *directory* and will rename the whole thing into place. Staging each file individually
/// inside it would be a second rename and a second fsync for no additional guarantee.
pub fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = open_staging(path, Some(OWNER_ONLY))?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn write_then_rename(path: &Path, bytes: &[u8], mode: Option<u32>) -> io::Result<()> {
    write_then_rename_with(path, bytes, mode, true)
}

fn write_then_rename_with(
    path: &Path,
    bytes: &[u8],
    mode: Option<u32>,
    durable: bool,
) -> io::Result<()> {
    let parent = path.parent().filter(|parent| !parent.as_os_str().is_empty());
    if let Some(parent) = parent {
        fs::create_dir_all(parent)?;
    }
    let staging = staging_path(path);

    // Scoped so the handle is closed before the rename: Windows will not rename a file that is still
    // open without share-delete, and the sync has to happen on this handle either way.
    let written = (|| {
        let mut file = open_staging(&staging, mode)?;
        file.write_all(bytes)?;
        if durable {
            file.sync_all()?;
        }
        Ok(())
    })();
    if let Err(error) = written {
        let _ = fs::remove_file(&staging);
        return Err(error);
    }

    if let Err(error) = rename_over(&staging, path) {
        let _ = fs::remove_file(&staging);
        return Err(error);
    }
    if let Some(parent) = parent.filter(|_| durable) {
        sync_directory(parent);
    }
    Ok(())
}

/// A staging name beside the target that no concurrent writer can pick.
///
/// Beside it, because a rename has to stay on one filesystem. Not `with_extension("tmp")`, which
/// both collides between processes and, worse, *replaces* the real extension — `main.rs` staged as
/// `main.tmp` next to a `main.md` staged the same way.
fn staging_path(path: &Path) -> PathBuf {
    let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let unique = uuid::Uuid::new_v4();
    let staging = format!(".{name}.{unique}.tmp");
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join(staging),
        _ => PathBuf::from(staging),
    }
}

/// Rename `staging` over `target`, retrying briefly on a denial.
///
/// Both platforms replace an existing destination. Windows additionally fails with
/// `ERROR_ACCESS_DENIED` when *anything* has the target open without share-delete — an indexer, a
/// virus scanner, an editor — which is transient and common enough that rustup and cargo both retry
/// it. Unix does not produce that error here, so the loop costs nothing there.
fn rename_over(staging: &Path, target: &Path) -> io::Result<()> {
    const ATTEMPTS: u32 = 5;
    for attempt in 0..ATTEMPTS {
        match fs::rename(staging, target) {
            Ok(()) => return Ok(()),
            Err(error)
                if attempt + 1 < ATTEMPTS && error.kind() == io::ErrorKind::PermissionDenied =>
            {
                std::thread::sleep(std::time::Duration::from_millis(10 * u64::from(attempt + 1)));
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("the loop returns on its last attempt")
}

#[cfg(unix)]
fn open_staging(path: &Path, mode: Option<u32>) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;

    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    if let Some(mode) = mode {
        options.mode(mode);
    }
    options.open(path)
}

#[cfg(not(unix))]
fn open_staging(path: &Path, _mode: Option<u32>) -> io::Result<File> {
    OpenOptions::new().write(true).create(true).truncate(true).open(path)
}

/// Best-effort `fsync` of the directory, so the rename itself is on the device.
///
/// Unix only: Windows has no handle for a directory's metadata, and `MoveFileEx` is ordered against
/// the file data anyway.
#[cfg(unix)]
fn sync_directory(path: &Path) {
    if let Ok(directory) = File::open(path) {
        let _ = directory.sync_all();
    }
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) {}

#[cfg(unix)]
fn restrict_dir(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn restrict_dir(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// An unsynced replace still lands whole and leaves no staging file behind.
    #[test]
    fn an_unsynced_replace_lands_whole_and_leaves_nothing_behind() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("cache").join("shard.json");
        replace_unsynced(&target, b"first").unwrap();
        replace_unsynced(&target, b"second").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"second");
        let left: Vec<_> = fs::read_dir(target.parent().unwrap()).unwrap().flatten().collect();
        assert_eq!(left.len(), 1, "only the shard: {left:?}");
    }

    /// Every directory `create_private_dir` makes is owner-only, not just the last one; one that was
    /// already there keeps its mode.
    #[cfg(unix)]
    #[test]
    fn every_directory_created_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        let dir = tempfile::tempdir().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();
        let leaf = dir.path().join("a").join("b").join("c");
        create_private_dir(&leaf).unwrap();
        for created in [dir.path().join("a"), dir.path().join("a/b"), leaf] {
            assert_eq!(mode(&created), 0o700, "{}", created.display());
        }
        assert_eq!(mode(dir.path()), 0o755, "an existing parent is left alone");
    }

    /// A fresh private directory refuses to adopt one that exists.
    #[test]
    fn a_new_private_directory_is_never_an_existing_one() {
        let dir = tempfile::tempdir().unwrap();
        let fresh = dir.path().join("fresh");
        create_private_dir_new(&fresh).unwrap();
        let error = create_private_dir_new(&fresh).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
    }

    #[test]
    fn a_replacement_lands_and_leaves_no_staging_file_behind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        replace(&path, b"first").unwrap();
        replace(&path, b"second").unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"second");
        let strays: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name != "state.json")
            .collect();
        assert!(strays.is_empty(), "left behind: {strays:?}");
    }

    #[test]
    fn missing_parents_are_created() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a").join("b").join("c.json");
        replace(&path, b"{}").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"{}");
    }

    /// The staging name has to survive a real extension, or `main.rs` and `main.md` stage to the
    /// same file and the last writer wins a race nobody knew they were in.
    #[test]
    fn two_files_sharing_a_stem_do_not_share_a_staging_name() {
        let dir = tempfile::tempdir().unwrap();
        let rust = staging_path(&dir.path().join("main.rs"));
        let markdown = staging_path(&dir.path().join("main.md"));
        assert_ne!(rust, markdown);
        assert_eq!(rust.parent(), Some(dir.path()), "staging stays on the target's filesystem");
    }

    /// Two writers of the *same* file must not collide on the staging name either — one would
    /// truncate the other's half-written bytes and then rename them into place.
    #[test]
    fn concurrent_writers_of_one_file_get_distinct_staging_names() {
        let path = Path::new("/tmp/luxe/thing.json");
        assert_ne!(staging_path(path), staging_path(path));
    }

    #[cfg(unix)]
    #[test]
    fn a_private_replacement_is_owner_only_from_the_moment_it_exists() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        replace_private(&path, b"secret").unwrap();
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);

        let nested = dir.path().join("private");
        create_private_dir(&nested).unwrap();
        assert_eq!(fs::metadata(&nested).unwrap().permissions().mode() & 0o777, 0o700);
    }

    /// A failed write must not leave a stray staging file, and must not damage what was there.
    #[test]
    fn a_failure_leaves_the_previous_contents_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kept.json");
        replace(&path, b"original").unwrap();

        // A directory in the target's place: the rename cannot succeed.
        let blocked = dir.path().join("blocked.json");
        fs::create_dir(&blocked).unwrap();
        assert!(replace(&blocked, b"nope").is_err());

        assert_eq!(fs::read(&path).unwrap(), b"original");
        let strays: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(strays.is_empty(), "left behind: {strays:?}");
    }
}
