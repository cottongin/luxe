//! Live sessions — which Luxes on this machine can be joined, and where.
//!
//! Each running luxe with an endpoint leaves a small record in the runtime folder, named for its
//! process, and takes it away when it stops. A record left by a luxe that died is found out by the
//! one test that matters: its endpoint no longer answers.

use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

/// One running luxe that can be joined.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveSession {
    /// The luxe's process id
    pub pid: u32,
    /// Session it is running
    pub session_id: String,
    /// Project folder it was opened in
    pub cwd: String,
    /// Where to [`super::connect`]
    pub address: String,
    /// The build serving it
    pub luxe_version: String,
}

/// This luxe's record in the runtime folder, removed when dropped.
pub struct Publication {
    path: PathBuf,
}

impl LiveSession {
    /// Writes this record into `dir`, readable only by this user.
    pub fn publish(&self, dir: &Path) -> io::Result<Publication> {
        crate::fs_atomic::create_private_dir(dir)?;
        let publication = Publication { path: dir.join(format!("{}.json", self.pid)) };
        publication.update(self)?;
        Ok(publication)
    }
}

impl Publication {
    /// Rewrites the record, as when the luxe moves to another session.
    pub fn update(&self, session: &LiveSession) -> io::Result<()> {
        let bytes = serde_json::to_vec_pretty(session).map_err(io::Error::other)?;
        crate::fs_atomic::replace_private(&self.path, &bytes)
    }
}

impl Drop for Publication {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Every record in `dir`, by process id. A missing folder has none, and a record that does not
/// read is skipped, since it is either being written or not one of these.
pub fn live_sessions(dir: &Path) -> Vec<LiveSession> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<LiveSession> =
        entries.filter_map(|entry| read_record(&entry.ok()?.path())).collect();
    found.sort_by_key(|session| session.pid);
    found
}

/// [`live_sessions`] whose luxe is still running: one that crashed leaves its record behind.
///
/// Checked by process id rather than by connecting, since a connection is a client joining, and the
/// terminal running the session says so each time.
pub fn running_sessions(dir: &Path) -> Vec<LiveSession> {
    let mut sessions = live_sessions(dir);
    sessions.retain(|session| crate::program::process_is_alive(session.pid));
    sessions
}

fn read_record(path: &Path) -> Option<LiveSession> {
    if path.extension()? != "json" {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes)
        .inspect_err(|error| tracing::debug!(path = %path.display(), %error, "skipped a record"))
        .ok()
}
