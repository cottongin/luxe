//! Sessions — what a device is told about the sessions it can join.

use std::path::Path;

use luxe_core::local_endpoint::{self, LiveSession};
use serde::Serialize;

/// One joinable session, as a device sees it.
///
/// The endpoint's address stays on this machine: a device joins through the bridge by id, and a
/// socket path or pipe name is of no use anywhere else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct SessionSummary {
    pub(crate) session_id: String,
    pub(crate) pid: u32,
    pub(crate) cwd: String,
    pub(crate) luxe_version: String,
}

impl From<&LiveSession> for SessionSummary {
    fn from(session: &LiveSession) -> Self {
        Self {
            session_id: session.session_id.clone(),
            pid: session.pid,
            cwd: session.cwd.clone(),
            luxe_version: session.luxe_version.clone(),
        }
    }
}

/// Every session running on this machine that can be joined.
pub(crate) fn running(runtime_dir: &Path) -> Vec<SessionSummary> {
    local_endpoint::running_sessions(runtime_dir).iter().map(SessionSummary::from).collect()
}

/// The running session with exactly this id. Exact, unlike `luxe join`: a device picks from the
/// list, so a prefix would only ever be a guess.
pub(crate) fn find(runtime_dir: &Path, session_id: &str) -> Option<LiveSession> {
    local_endpoint::running_sessions(runtime_dir)
        .into_iter()
        .find(|session| session.session_id == session_id)
}
