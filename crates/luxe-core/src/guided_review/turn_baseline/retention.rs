//! Retention — what a new turn baseline shares with the last one, and which old ones go.
//!
//! Every agent turn captures the whole workspace twice, and few files change between turns, so a
//! baseline hard-links each payload the newest earlier one already holds instead of writing and
//! syncing it again. Only the last turn's pair is ever read back, so the rest are deleted once
//! they're an hour old; the hour spares a turn that another session in this project is still in.

use super::{TurnBaselineId, TURN_BASELINE_CACHE_RELATIVE_DIR, TURN_BASELINE_PAYLOAD_DIR};
use crate::guided_review::{source_store::write_private_file, SourceStoreError};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};
use tracing::{debug, warn};

/// How old a baseline the last-turn record doesn't name must be before pruning deletes it.
pub(crate) const UNREFERENCED_BASELINE_AGE: Duration = Duration::from_secs(60 * 60);

/// The payload directory of the newest committed baseline under `root`, to link unchanged files
/// from; none when there isn't one yet.
pub(super) fn newest_payload_dir(root: &Path) -> Option<PathBuf> {
    fs::read_dir(root)
        .ok()?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
        .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, path)| path.join(TURN_BASELINE_PAYLOAD_DIR))
}

/// Puts one payload at `destination`: a hard link to `earlier`'s copy of the same digest when it
/// has a plain file of the right size, else a fresh synced write. A link that fails (another
/// filesystem, one that has none) falls back to the write.
pub(super) fn place_payload(
    destination: &Path,
    earlier: Option<&Path>,
    digest: &str,
    bytes: &[u8],
) -> Result<(), SourceStoreError> {
    if let Some(source) = earlier.map(|dir| dir.join(digest)) {
        let same = fs::symlink_metadata(&source)
            .is_ok_and(|meta| meta.is_file() && meta.len() == bytes.len() as u64);
        if same && fs::hard_link(&source, destination).is_ok() {
            return Ok(());
        }
    }
    write_private_file(destination, bytes)
}

/// Deletes every baseline under the project's cache but `keep` once it's older than `age`, along
/// with staging a crashed capture left behind. A symlink is never followed, and one that can't be
/// deleted is logged and skipped, since a stale baseline costs only disk. Returns how many went.
pub(crate) fn prune_turn_baselines(
    project_root: &Path,
    keep: &[&TurnBaselineId],
    age: Duration,
) -> usize {
    let root = project_root.join(TURN_BASELINE_CACHE_RELATIVE_DIR);
    let Some(entries) = baseline_entries(&root) else {
        return 0;
    };
    let kept: HashSet<&str> = keep.iter().map(|id| id.cache_key()).collect();
    let cutoff = SystemTime::now().checked_sub(age).unwrap_or(SystemTime::UNIX_EPOCH);
    let pruned = entries
        .filter(|entry| is_stale(entry, &kept, cutoff))
        .filter(|entry| remove_baseline(&entry.path()))
        .count();
    debug!(pruned, "pruned turn baselines");
    pruned
}

/// The cache's entries; none when it doesn't exist yet, or can't be listed, which is logged.
fn baseline_entries(root: &Path) -> Option<impl Iterator<Item = fs::DirEntry>> {
    match fs::read_dir(root) {
        Ok(entries) => Some(entries.filter_map(Result::ok)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            warn!(path = %root.display(), %error, "could not list turn baselines to prune");
            None
        }
    }
}

/// A directory, not a symlink, that `kept` doesn't name and that was last written before `cutoff`.
fn is_stale(entry: &fs::DirEntry, kept: &HashSet<&str>, cutoff: SystemTime) -> bool {
    entry.file_type().is_ok_and(|kind| kind.is_dir())
        && !kept.contains(entry.file_name().to_string_lossy().as_ref())
        && entry.metadata().and_then(|meta| meta.modified()).is_ok_and(|at| at < cutoff)
}

fn remove_baseline(path: &Path) -> bool {
    fs::remove_dir_all(path)
        .inspect_err(|error| warn!(path = %path.display(), %error, "could not prune turn baseline"))
        .is_ok()
}

#[cfg(test)]
mod tests;
