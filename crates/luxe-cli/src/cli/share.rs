//! Session sharing — the terminal's session, offered to other front-ends on this machine.
//!
//! "The terminal" is whichever front-end started the session: the TUI, or the client on the other
//! end of `luxe --rpc`. The terminal stays in charge. A client that connects to the local endpoint
//! (a Unix socket or a named pipe only this user can open) speaks the protocol `luxe --rpc` does:
//! it gets a `ready` frame, then what it missed, then every event as it happens, and its actions
//! reach the same engine the terminal drives. A joined client can't switch or close the session,
//! and leaving closes nothing. The terminal is told when one joins, leaves, or answers a prompt.

mod clients;
mod guest;
mod guests;
mod snapshot;

use super::rpc::{GithubWrites, Policy};
use luxe_core::engine::Action;
use luxe_core::local_endpoint::{EndpointListener, LiveSession, Publication};
use luxe_core::Event;
use snapshot::Snapshot;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;

/// Events the terminal hasn't drawn yet, as deep as the stream it reads without sharing.
const TERMINAL_BUFFER: usize = 256;

/// How far a joined client may fall behind before it's dropped and has to join again.
///
/// Deep because token deltas are one event each, and a phone on a slow link reads in bursts.
const CLIENT_BACKLOG: usize = 4096;

/// The session on offer. Dropping it takes the offer down and disconnects every client.
pub(crate) struct Sharing {
    tasks: Vec<JoinHandle<()>>,
}

impl Drop for Sharing {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

/// What every joined client shares: the session's state, its live events, and the terminal.
#[derive(Clone)]
struct Shared {
    snapshot: Arc<Mutex<Snapshot>>,
    live: broadcast::Sender<Event>,
    engine_tx: mpsc::Sender<Action>,
    /// Weak, so the terminal's stream ends with the engine's while clients are still connected:
    /// `--rpc` exits on that end
    terminal: mpsc::WeakSender<Event>,
    cwd: String,
    policy: Policy,
    /// What `select_model` resolves through and `list_models` lists
    models: super::rpc::Models,
    /// The process whose descendants are refused: this one, outside tests
    agent_pid: u32,
    /// Who's joined now, for the terminal's dock
    guests: guests::GuestList,
}

impl Shared {
    /// The snapshot, even if a panic elsewhere poisoned the lock: it holds no invariant a
    /// half-finished update could break that a client couldn't recover from by joining again.
    fn snapshot(&self) -> MutexGuard<'_, Snapshot> {
        self.snapshot.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Fold `event` into the snapshot and hand it to every client, as one step so a client
    /// joining in between sees it exactly once.
    fn publish(&self, event: &Event) {
        let mut snapshot = self.snapshot();
        snapshot.observe(event);
        // No receivers is the normal state with nobody joined
        let _ = self.live.send(event.clone());
    }

    /// Put a line in front of the person at the terminal.
    async fn tell_terminal(&self, notice: String) {
        self.to_terminal(Event::Notice(notice)).await;
    }

    /// Tell the terminal who's joined now, for its dock.
    async fn tell_guests(&self) {
        self.to_terminal(Event::GuestsChanged(self.guests.names())).await;
    }

    async fn to_terminal(&self, event: Event) {
        // The terminal gone means the session is ending, and nobody is left to tell
        let Some(terminal) = self.terminal.upgrade() else { return };
        let _ = terminal.send(event).await;
    }
}

/// Offer the session `events` belongs to, and return the stream the terminal reads instead.
///
/// With `joinable` off nothing listens. When the endpoint can't be opened the terminal runs
/// exactly as it would unshared, and the reason goes to the log: sharing is a convenience,
/// never a reason to refuse to start.
pub(crate) fn share(
    events: mpsc::Receiver<Event>,
    engine_tx: mpsc::Sender<Action>,
    settings: &crate::engine_setup::AgentSettings,
    session_id: &str,
) -> (mpsc::Receiver<Event>, Option<Sharing>) {
    let config = luxe_core::config::Config::load(&settings.state_root);
    if !config.joinable() {
        tracing::info!("not sharing the session: joinable is off");
        return (events, None);
    }
    let Some((listener, record)) = open_endpoint(settings, session_id) else {
        return (events, None);
    };
    let (terminal, terminal_rx) = mpsc::channel(TERMINAL_BUFFER);
    let shared = Shared {
        snapshot: Arc::new(Mutex::new(Snapshot::new(session_id.to_string()))),
        live: broadcast::channel(CLIENT_BACKLOG).0,
        engine_tx,
        terminal: terminal.downgrade(),
        cwd: settings.work_root.to_string_lossy().into_owned(),
        policy: guest_policy(settings, &config),
        models: Arc::new(super::rpc::CatalogResolver { settings: settings.clone() }),
        agent_pid: std::process::id(),
        guests: guests::GuestList::default(),
    };
    let tasks = vec![
        tokio::spawn(clients::accept(listener, shared.clone())),
        tokio::spawn(tee(events, shared, terminal, record)),
    ];
    tracing::info!(%session_id, "sharing the session on the local endpoint");
    (terminal_rx, Some(Sharing { tasks }))
}

/// Listen on the local endpoint and publish the record that lists it.
fn open_endpoint(
    settings: &crate::engine_setup::AgentSettings,
    session_id: &str,
) -> Option<(EndpointListener, Option<Record>)> {
    let dir = luxe_core::user_dirs::luxe_runtime_dir();
    let listener = match EndpointListener::bind(&dir) {
        Ok(listener) => listener,
        Err(error) => {
            tracing::warn!(%error, dir = %dir.display(), "not sharing the session");
            return None;
        }
    };
    let record = publish_record(&dir, &listener, settings, session_id);
    Some((listener, record))
}

/// What a joined client may do: what the terminal may, with GitHub writes left off, since
/// `--github-writes` is a choice made for one `--rpc` client and nobody made it for this one.
fn guest_policy(
    settings: &crate::engine_setup::AgentSettings,
    config: &luxe_core::config::Config,
) -> Policy {
    let mode = crate::engine_setup::effective_permission_mode(
        &settings.state_root,
        settings.permission_mode,
        settings.trust_project,
    );
    Policy {
        github_writes: GithubWrites::for_launch(false, mode),
        github_sections: config.github.sections(),
        joined: true,
    }
}

/// Copy every engine event to the clients and on to the terminal, until either end is gone.
///
/// Owns the terminal's only strong sender, so the terminal's stream ends when this does.
async fn tee(
    mut events: mpsc::Receiver<Event>,
    shared: Shared,
    terminal: mpsc::Sender<Event>,
    mut record: Option<Record>,
) {
    while let Some(event) = events.recv().await {
        shared.publish(&event);
        if let (Event::SessionLoaded { session_id, .. }, Some(record)) = (&event, record.as_mut()) {
            record.follow(session_id);
        }
        if terminal.send(event).await.is_err() {
            break;
        }
    }
}

/// The published record of this session, which `luxe join` and `luxe remote` list.
struct Record {
    session: LiveSession,
    publication: Publication,
}

impl Record {
    /// Keep the record naming the session the terminal shows.
    fn follow(&mut self, session_id: &str) {
        if self.session.session_id == session_id {
            return;
        }
        self.session.session_id = session_id.to_string();
        if let Err(error) = self.publication.update(&self.session) {
            tracing::warn!(%error, "could not update the shared session's record");
        }
    }
}

/// Write the record that lists this session, or log why it couldn't be written: the endpoint
/// still works for a client that knows its address.
fn publish_record(
    dir: &Path,
    listener: &EndpointListener,
    settings: &crate::engine_setup::AgentSettings,
    session_id: &str,
) -> Option<Record> {
    let session = LiveSession {
        pid: std::process::id(),
        session_id: session_id.to_string(),
        cwd: settings.work_root.to_string_lossy().into_owned(),
        address: listener.address().to_string(),
        luxe_version: env!("CARGO_PKG_VERSION").to_string(),
    };
    match session.publish(dir) {
        Ok(publication) => Some(Record { session, publication }),
        Err(error) => {
            tracing::warn!(%error, "could not publish the shared session's record");
            None
        }
    }
}

#[cfg(test)]
mod tests;
