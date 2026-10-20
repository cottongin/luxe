//! Moving the running copy aside — how `luxe update` gets out of its own way on Windows.
//!
//! Windows locks a running program's file against being overwritten or deleted, but not against
//! being renamed. The running `luxe.exe` becomes `luxe.exe.old`, the owner's updater writes a new
//! `luxe.exe`, and the next launch deletes the leftover once nothing is running from it. Nothing
//! here writes a binary: it only renames the one that is already there.

use anyhow::Context;
use std::path::{Path, PathBuf};

/// The running copy, renamed out of the way of the update.
pub(super) struct MovedAside {
    from: PathBuf,
    to: PathBuf,
}

impl MovedAside {
    /// Renames `exe` to its leftover name, clearing a leftover a previous update left behind.
    pub(super) fn move_running(exe: &Path) -> anyhow::Result<Self> {
        let to = leftover_path(exe);
        remove_if_present(&to).with_context(|| {
            format!("could not remove {}, left by an earlier update", to.display())
        })?;
        std::fs::rename(exe, &to).with_context(|| {
            format!("could not move the running {} aside to update it", exe.display())
        })?;
        Ok(Self { from: exe.to_path_buf(), to })
    }

    /// Puts the running copy back when the update left nothing where it was, so a failed update
    /// never leaves the reader without a `luxe`.
    pub(super) fn restore_if_missing(&self) -> anyhow::Result<()> {
        if self.from.exists() {
            return Ok(());
        }
        std::fs::rename(&self.to, &self.from).with_context(|| {
            format!(
                "the update left no luxe behind and {} could not be moved back to {}",
                self.to.display(),
                self.from.display()
            )
        })
    }
}

/// Deletes the copy an earlier update moved aside, if there is one. Called at launch, when the
/// old copy has exited. Still running elsewhere is normal (another session), so a failure is
/// only logged and the next launch tries again.
pub(crate) fn remove_leftover() {
    let Ok(exe) = std::env::current_exe() else { return };
    let leftover = leftover_path(&exe);
    if let Err(error) = remove_if_present(&leftover) {
        tracing::debug!(%error, path = %leftover.display(), "leftover from an update is still in use");
    }
}

/// `luxe.exe` beside itself as `luxe.exe.old`.
fn leftover_path(exe: &Path) -> PathBuf {
    let mut name = exe.file_name().unwrap_or_default().to_os_string();
    name.push(".old");
    exe.with_file_name(name)
}

/// Removes `path`; already gone is success.
fn remove_if_present(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installed() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let exe = dir.path().join("luxe.exe");
        std::fs::write(&exe, b"old").expect("a stand-in binary");
        (dir, exe)
    }

    #[test]
    fn the_leftover_sits_beside_the_binary() {
        assert_eq!(leftover_path(Path::new("/bin/luxe.exe")), PathBuf::from("/bin/luxe.exe.old"));
    }

    /// The update writes a new file where the old one was, and the old one is left to delete.
    #[test]
    fn an_update_that_wrote_a_new_copy_keeps_it() {
        let (_dir, exe) = installed();
        let aside = MovedAside::move_running(&exe).expect("moved");
        assert!(!exe.exists(), "the name is free for the updater");
        std::fs::write(&exe, b"new").expect("the updater's copy");
        aside.restore_if_missing().expect("nothing to restore");
        assert_eq!(std::fs::read(&exe).expect("read"), b"new");
        assert_eq!(std::fs::read(leftover_path(&exe)).expect("read"), b"old");
    }

    /// A failed update puts the running copy back rather than leaving no `luxe` at all.
    #[test]
    fn an_update_that_wrote_nothing_puts_the_old_copy_back() {
        let (_dir, exe) = installed();
        let aside = MovedAside::move_running(&exe).expect("moved");
        aside.restore_if_missing().expect("restored");
        assert_eq!(std::fs::read(&exe).expect("read"), b"old");
        assert!(!leftover_path(&exe).exists());
    }

    /// A leftover from the last update is cleared, not a reason to refuse this one.
    #[test]
    fn a_leftover_from_last_time_is_replaced() {
        let (_dir, exe) = installed();
        std::fs::write(leftover_path(&exe), b"older").expect("a leftover");
        MovedAside::move_running(&exe).expect("moved");
        assert_eq!(std::fs::read(leftover_path(&exe)).expect("read"), b"old");
    }
}
