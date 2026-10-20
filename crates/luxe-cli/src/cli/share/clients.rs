//! One joined client at a time: checked, caught up, then served like any RPC client.

use super::super::rpc;
use super::guest::{self, Guest};
use super::Shared;
use luxe_core::engine::Action;
use luxe_core::local_endpoint::{EndpointListener, Joined, LocalStream};
use luxe_core::Event;
use tokio::sync::broadcast::{self, error::RecvError};
use tokio::sync::mpsc;
use tokio::task::JoinSet;

/// Actions a client may queue before its reads wait on the engine.
const ACTION_BUFFER: usize = 32;

/// Frames queued for one client's socket; past this, the shared backlog takes the slack.
const CLIENT_WRITE_BUFFER: usize = 256;

/// Bytes of a client's input held between reading it and the protocol taking it.
const SIFTED_BUFFER: usize = 64 * 1024;

/// How long to wait before accepting again after an accept failed, so a lasting failure (out of
/// file handles) logs a few lines a second instead of spinning a core.
const ACCEPT_RETRY: std::time::Duration = std::time::Duration::from_millis(250);

/// Take clients until the task is aborted, which disconnects every one of them.
pub(super) async fn accept(mut listener: EndpointListener, shared: Shared) {
    let mut clients = JoinSet::new();
    loop {
        match listener.accept().await {
            Ok(joined) => {
                clients.spawn(serve_client(joined, shared.clone()));
            }
            Err(error) => {
                tracing::warn!(%error, "could not accept a client of the shared session");
                tokio::time::sleep(ACCEPT_RETRY).await;
            }
        }
        // Reap finished clients so a long session doesn't keep one entry per visit
        while clients.try_join_next().is_some() {}
    }
}

/// Serve one client from its `ready` frame until it leaves or the session ends.
async fn serve_client(joined: Joined, shared: Shared) {
    let (guest, named) = Guest::new(joined.peer);
    if guest.descends_from(shared.agent_pid).await {
        tracing::warn!(peer = ?joined.peer, "refused a client started by this session's agent");
        let notice = format!("Refused {guest}: the agent started it, and can't approve itself");
        shared.tell_terminal(notice).await;
        return;
    }
    let served = serve_rpc(joined.stream, &shared, guest.clone(), named);
    let (arrival, ()) = tokio::join!(announce(&shared, guest.clone()), served);
    tracing::info!(peer = ?joined.peer, %guest, "a client left the shared session");
    let Some(arrival) = arrival else { return };
    shared.guests.leave(arrival.seat);
    shared.tell_guests().await;
    if arrival.told {
        shared.tell_terminal(format!("{guest} left this session")).await;
    }
}

/// How a client was announced: its seat in the dock's list, and whether the transcript was told.
struct Arrival {
    seat: u64,
    told: bool,
}

/// Seats the client in the dock's list once it has had a moment to say who it is, unless it opened
/// as watch-only. Only someone else's client earns a line in the transcript: the person at the
/// terminal knows their own phone joined, and a phone reconnects often.
async fn announce(shared: &Shared, mut guest: Guest) -> Option<Arrival> {
    guest.introduced().await;
    if guest.watch_only() {
        tracing::info!(%guest, "a watch-only client joined the shared session");
        return None;
    }
    let seat = shared.guests.arrive(guest.to_string());
    shared.tell_guests().await;
    let told = !guest.yours();
    if told {
        shared.tell_terminal(format!("{guest} joined this session")).await;
    }
    Some(Arrival { seat, told })
}

/// The RPC protocol over `stream`, fed from the shared session rather than an engine of its own.
async fn serve_rpc(
    stream: LocalStream,
    shared: &Shared,
    guest: Guest,
    named: tokio::sync::watch::Sender<Option<guest::Introduction>>,
) {
    let (opening, frames, live) = catch_up(shared);
    let (events_tx, events_rx) = mpsc::channel(CLIENT_WRITE_BUFFER);
    let _feed = AbortOnDrop(tokio::spawn(feed(frames, live, events_tx)));
    let (actions_tx, actions_rx) = mpsc::channel(ACTION_BUFFER);
    let _relay = AbortOnDrop(tokio::spawn(relay(actions_rx, shared.clone(), guest)));
    let (read, write) = tokio::io::split(stream);
    let (sifted, input) = tokio::io::duplex(SIFTED_BUFFER);
    let _sift = AbortOnDrop(tokio::spawn(guest::sift(read, sifted, named)));
    let (policy, models) = (shared.policy.clone(), shared.models.clone());
    let served = rpc::serve(opening, events_rx, actions_tx, input, write, models, policy);
    if let Err(error) = served.await {
        tracing::warn!(%error, "a shared session's client ended with an error");
    }
}

/// The `ready` frame and transcript, what else the client missed, and the live stream picking up
/// right after it.
///
/// Taken under the lock `publish` holds, so no event falls between the catch-up and the stream.
fn catch_up(shared: &Shared) -> (rpc::Opening, Vec<Event>, broadcast::Receiver<Event>) {
    let snapshot = shared.snapshot();
    let ready = Event::Ready {
        protocol_version: luxe_core::PROTOCOL_VERSION,
        luxe_version: env!("CARGO_PKG_VERSION").to_string(),
        session_id: snapshot.session_id().to_string(),
        cwd: shared.cwd.clone(),
        github_writes: shared.policy.github_writes.allowed(),
    };
    let opening = rpc::Opening { ready, transcript: Some(snapshot.transcript()) };
    (opening, snapshot.frames(), shared.live.subscribe())
}

/// Hand the client its catch-up and then the live stream, until it falls too far behind.
async fn feed(frames: Vec<Event>, mut live: broadcast::Receiver<Event>, out: mpsc::Sender<Event>) {
    for frame in frames {
        if out.send(frame).await.is_err() {
            return;
        }
    }
    loop {
        let event = match live.recv().await {
            Ok(event) => event,
            Err(RecvError::Lagged(missed)) => {
                let notice = format!("This client fell {missed} events behind: join again");
                let _ = out.send(Event::Notice(notice)).await;
                return;
            }
            Err(RecvError::Closed) => return,
        };
        if out.send(event).await.is_err() {
            return;
        }
    }
}

/// Pass the client's actions to the engine, telling the terminal about each answer it gives.
async fn relay(mut actions: mpsc::Receiver<Action>, shared: Shared, guest: Guest) {
    while let Some(action) = actions.recv().await {
        let answer = answer_notice(&shared, &action, &guest);
        if shared.engine_tx.send(action).await.is_err() {
            return;
        }
        if let Some(notice) = answer {
            shared.tell_terminal(notice).await;
        }
    }
}

/// What to tell the terminal about `action`, when it answers a prompt that's still waiting.
///
/// A prompt the terminal already answered is a no-op for the engine, so it says nothing here.
fn answer_notice(shared: &Shared, action: &Action, guest: &Guest) -> Option<String> {
    use luxe_core::permission::PermissionReply;
    let (id, verb) = match action {
        Action::RespondPermission { id, reply: PermissionReply::Reject } => (id.as_str(), "Denied"),
        Action::RespondPermission { id, .. } => (id.as_str(), "Approved"),
        Action::AnswerQuestion { id, .. } => (id.as_str(), "Answered"),
        _ => return None,
    };
    let waiting = shared.snapshot().is_waiting(id);
    waiting.then(|| format!("{verb} from {guest}"))
}

/// Aborts its task when dropped, so a client's helpers end with it.
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}
