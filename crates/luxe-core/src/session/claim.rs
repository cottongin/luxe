//! Session claims — one luxe process writes a session's log at a time.
//!
//! Two processes appending to one log interleave entries that each parent on their own leaf, so a
//! resume walks one branch and silently drops the other's turns. A claim is an OS lock on a
//! `.lock` file beside the log: the system lets go of it when the process ends, however it ends,
//! so a crash never leaves a session stuck. Within one process the claim is counted instead,
//! since the TUI starts the next engine on a session before the last one has let go of it.

use std::collections::HashMap;
use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

/// This process's claim on one session, released when the last one for that session drops.
#[derive(Debug)]
pub struct SessionClaim {
    lock_path: PathBuf,
}

/// The locks this process holds, by lock file, with how many claims share each.
struct Held {
    _file: File,
    claims: usize,
}

fn held() -> MutexGuard<'static, HashMap<PathBuf, Held>> {
    static HELD: OnceLock<Mutex<HashMap<PathBuf, Held>>> = OnceLock::new();
    // A panic mid-update leaves a count off by one at worst, which a restart clears
    HELD.get_or_init(Default::default).lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Claim the session whose lock file is `lock_path`. `None` when another process holds it.
pub(crate) fn claim(lock_path: &Path) -> io::Result<Option<SessionClaim>> {
    let mut held = held();
    if let Some(shared) = held.get_mut(lock_path) {
        shared.claims += 1;
        return Ok(Some(SessionClaim { lock_path: lock_path.to_path_buf() }));
    }
    let file = OpenOptions::new().create(true).truncate(false).write(true).open(lock_path)?;
    match file.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => return Ok(None),
        Err(TryLockError::Error(error)) => return Err(error),
    }
    held.insert(lock_path.to_path_buf(), Held { _file: file, claims: 1 });
    Ok(Some(SessionClaim { lock_path: lock_path.to_path_buf() }))
}

impl Drop for SessionClaim {
    fn drop(&mut self) {
        let mut held = held();
        let Some(shared) = held.get_mut(&self.lock_path) else { return };
        shared.claims -= 1;
        if shared.claims == 0 {
            // Closing the file is what releases the lock; the empty file stays, since removing it
            // would race a process that just opened it to lock
            held.remove(&self.lock_path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whether some other holder could take the lock right now, as another process would.
    fn free_for_another(lock_path: &Path) -> bool {
        let file = File::open(lock_path).expect("the lock file exists");
        file.try_lock().is_ok()
    }

    /// The break this catches: a claim that takes no OS lock, so a second luxe writes beside it.
    #[test]
    fn a_claim_keeps_other_holders_out_until_it_drops() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("s.lock");
        let claimed = claim(&path).expect("claim").expect("free");
        assert!(!free_for_another(&path));
        drop(claimed);
        assert!(free_for_another(&path));
    }

    /// The break this catches: ignoring a lock someone else holds.
    #[test]
    fn a_session_held_elsewhere_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("s.lock");
        let other = File::create(&path).expect("create");
        other.try_lock().expect("the other holder locks first");
        assert!(claim(&path).expect("claim").is_none());
    }

    /// The break this catches: releasing on the first drop, so the engine that's still running
    /// loses its claim when the one it replaced shuts down.
    #[test]
    fn claims_within_one_process_are_counted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("s.lock");
        let first = claim(&path).expect("claim").expect("free");
        let second = claim(&path).expect("claim").expect("this process already holds it");
        drop(first);
        assert!(!free_for_another(&path), "still claimed by the second");
        drop(second);
        assert!(free_for_another(&path));
    }
}
