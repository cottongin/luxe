//! Launch — starting a session for a device: in a herdr pane when herdr is running, otherwise as a
//! `luxe --rpc` child of the bridge.
//!
//! Either way the new luxe shares its session like any other, so a device joins it through the
//! same route as one started at the desk. The bridge learns which session is the new one by
//! watching for its record in the runtime folder.

mod herdr;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use luxe_core::local_endpoint::{running_sessions, LiveSession};
use serde::Serialize;
use tokio::sync::watch;

use crate::folders::{self, Folder, FolderBook};

pub(crate) use herdr::Herdr;

/// How long a new luxe gets to share its session. It takes about a second.
const APPEAR_TIMEOUT: Duration = Duration::from_secs(20);
/// How often the runtime folder is reread while waiting.
const APPEAR_POLL: Duration = Duration::from_millis(250);

/// Where a started session runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Place {
    /// A herdr pane, which outlives the bridge and shows at the desk
    Herdr,
    /// A child of the bridge, which lasts while the bridge does
    Bridge,
}

/// Why a session couldn't be started.
#[derive(Debug, thiserror::Error)]
pub(crate) enum LaunchError {
    /// The device asked for something that isn't a folder here
    #[error("{0}")]
    NotAFolder(String),
    /// herdr is running but turned the request down
    #[error("herdr couldn't open a pane: {0}")]
    Herdr(String),
    /// The child couldn't be started
    #[error("couldn't start luxe: {0}")]
    Spawn(#[source] std::io::Error),
    /// luxe started but no session showed up in time
    #[error("luxe started in {0} but didn't share a session within 20 seconds")]
    NeverAppeared(String),
}

/// Starts sessions, and holds the ones it runs itself.
pub(crate) struct Launcher {
    /// The `luxe` a child runs: the bridge's own
    program: PathBuf,
    herdr: Herdr,
    folders: FolderBook,
    /// Dropped with the launcher, which has each child's watcher close its stdin: a child ends its
    /// session on that, as for any client hanging up, so the bridge stopping ends them cleanly
    held: watch::Sender<()>,
}

impl Launcher {
    /// A launcher that runs `program` when `herdr` can't open a pane, remembering folders in
    /// `folders`.
    pub(crate) fn new(program: PathBuf, herdr: Herdr, folders: FolderBook) -> Self {
        Self { program, herdr, folders, held: watch::channel(()).0 }
    }

    /// Where a session could start, and whether it would open in herdr.
    pub(crate) async fn folders(&self, runtime_dir: &Path) -> (Vec<Folder>, bool) {
        let running = running_now(runtime_dir).await;
        let running: Vec<String> = running.into_iter().map(|session| session.cwd).collect();
        let in_herdr = self.herdr.running().await;
        let mut herdr = Vec::new();
        if in_herdr {
            let panes = self.herdr.panes().await;
            let panes = panes.inspect_err(|error| tracing::warn!(%error, "no folders from herdr"));
            herdr = panes.unwrap_or_default().into_iter().map(|pane| pane.cwd).collect();
        }
        self.saw(running.clone()).await;
        let book = self.folders.clone();
        let listed = tokio::task::spawn_blocking(move || folders::listing(&running, &herdr, &book));
        (listed.await.unwrap_or_default(), in_herdr)
    }

    /// Notes the folders luxe is running in, for the list a device starts sessions from.
    pub(crate) async fn saw(&self, running: Vec<String>) {
        let book = self.folders.clone();
        let noted = tokio::task::spawn_blocking(move || book.saw(&running)).await;
        if let Ok(Err(error)) = noted {
            tracing::warn!(%error, "could not note the folders luxe is running in");
        }
    }

    /// Starts luxe in `folder` and waits for the session it shares.
    pub(crate) async fn start(
        &self,
        folder: &Path,
        runtime_dir: &Path,
    ) -> Result<(LiveSession, Place), LaunchError> {
        let before = running_pids(runtime_dir).await;
        let place = if self.herdr.running().await {
            self.herdr.open(folder).await?;
            Place::Herdr
        } else {
            self.spawn(folder)?;
            Place::Bridge
        };
        let session = appeared(runtime_dir, before, folder).await?;
        Ok((session, place))
    }

    fn spawn(&self, folder: &Path) -> Result<(), LaunchError> {
        let mut command = tokio::process::Command::new(&self.program);
        command
            .arg("--rpc")
            .current_dir(folder)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            // a bridge running in a herdr pane would have each child report into that pane
            .env_remove("HERDR_ENV")
            .env_remove("HERDR_PANE_ID");
        luxe_core::program::detach_from_terminal(&mut command);
        let child = command.spawn().map_err(LaunchError::Spawn)?;
        tokio::spawn(watch_child(child, self.held.subscribe()));
        Ok(())
    }
}

/// Waits on a child until it ends, closing its stdin once the launcher is gone.
///
/// Waited on rather than left alone because an exited child nobody waits on keeps its process id
/// alive, and its record would list a session that can't be joined.
async fn watch_child(mut child: tokio::process::Child, mut held: watch::Receiver<()>) {
    let pid = child.id();
    let stdin = child.stdin.take();
    let ended = tokio::select! {
        status = child.wait() => Some(status),
        // only ever an error: nothing is sent, and it fails once the launcher drops
        _ = held.changed() => None,
    };
    let status = match ended {
        Some(status) => status,
        None => {
            drop(stdin);
            child.wait().await
        }
    };
    match status {
        Ok(status) => tracing::info!(?pid, %status, "a session the bridge started has ended"),
        Err(error) => tracing::warn!(?pid, %error, "lost track of a session the bridge started"),
    }
}

/// The folder a device asked for, as a full path to a folder that exists. A leading `~` is the
/// home folder, since that's how people type paths.
pub(crate) fn resolve_folder(asked: &str, home: Option<&Path>) -> Result<PathBuf, LaunchError> {
    let asked = asked.trim();
    let path = match (asked.strip_prefix('~'), home) {
        (Some(rest), Some(home)) if rest.is_empty() || rest.starts_with(['/', '\\']) => {
            home.join(rest.trim_start_matches(['/', '\\']))
        }
        _ => PathBuf::from(asked),
    };
    if !path.is_absolute() {
        return Err(LaunchError::NotAFolder(format!("`{asked}` isn't a full path")));
    }
    if !path.is_dir() {
        let shown = luxe_core::paths::display(&path);
        return Err(LaunchError::NotAFolder(format!("{shown} isn't a folder on this machine")));
    }
    Ok(path)
}

async fn running_now(runtime_dir: &Path) -> Vec<LiveSession> {
    let dir = runtime_dir.to_path_buf();
    // reads a folder and, on Windows, asks `tasklist` about each pid
    tokio::task::spawn_blocking(move || running_sessions(&dir)).await.unwrap_or_default()
}

async fn running_pids(runtime_dir: &Path) -> HashSet<u32> {
    running_now(runtime_dir).await.iter().map(|session| session.pid).collect()
}

/// Waits for the session started in `folder` to be listed.
async fn appeared(
    runtime_dir: &Path,
    before: HashSet<u32>,
    folder: &Path,
) -> Result<LiveSession, LaunchError> {
    let before = std::sync::Arc::new(before);
    let deadline = tokio::time::Instant::now() + APPEAR_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        let (dir, before, wanted) = (runtime_dir.to_path_buf(), before.clone(), folder.to_owned());
        let found = tokio::task::spawn_blocking(move || {
            newcomer(&running_sessions(&dir), &before, &wanted)
        });
        if let Ok(Some(session)) = found.await {
            return Ok(session);
        }
        tokio::time::sleep(APPEAR_POLL).await;
    }
    Err(LaunchError::NeverAppeared(luxe_core::paths::display(folder)))
}

/// The session in `running` that wasn't there `before` and works on `folder`.
///
/// "Works on" rather than "is in": a luxe started in a subfolder of a repository shares the
/// repository's root as its folder.
fn newcomer(running: &[LiveSession], before: &HashSet<u32>, folder: &Path) -> Option<LiveSession> {
    let folder = folder.canonicalize().ok()?;
    running
        .iter()
        .filter(|session| !before.contains(&session.pid))
        .find(|session| {
            let cwd = Path::new(&session.cwd).canonicalize();
            cwd.is_ok_and(|cwd| luxe_core::paths::contains(&cwd, &folder))
        })
        .cloned()
}

#[cfg(test)]
mod tests;
