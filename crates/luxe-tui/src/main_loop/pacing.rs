//! Frame pacing — how many queued events join a frame, and whether an iteration paints.

use super::keys::collect_exit;
use super::paste_burst::apply_folding_pastes;
use super::TuiExit;
use crate::app::{App, Msg};
use crossterm::event::{Event, EventStream};
use futures::StreamExt;
use std::time::Duration;
use tokio::sync::mpsc;

/// Engine events applied in one pass before the next draw. High enough that a burst of token
/// deltas collapses into a single frame, low enough that key handling stays responsive.
const MAX_COALESCED_EVENTS: usize = 256;

/// Terminal events applied in one pass before the next draw.
///
/// Key repeat and pointer motion arrive far faster than a frame — 40 keys a second is the desktop
/// default, and a pointer streams motion as fast as the compositor will report it. Drawing one
/// frame each is what makes a held key look like a freeze: every event demands a repaint whether
/// or not the last one finished, so the loop falls behind by however long the burst lasts and only
/// catches up once the key comes back up. Whatever is already waiting is therefore applied first,
/// and the burst costs one frame instead of one apiece.
///
/// Smaller than the engine's budget because these are a *person's* events: sixty swallows any
/// human burst, and a cap that low keeps a stuck terminal from starving the animation tick.
pub(super) const MAX_COALESCED_INPUT: usize = 60;

/// The terminal's next event, or never while the terminal is handed to a launcher.
pub(super) async fn next_event(events: &mut Option<EventStream>) -> Option<std::io::Result<Event>> {
    match events.as_mut() {
        Some(events) => events.next().await,
        None => std::future::pending().await,
    }
}

/// The next terminal event, if one is already waiting.
///
/// Polled with the loop's own waker rather than a throwaway one, so a stream that answers with
/// nothing here still wakes the loop's `select!` when its event does land.
pub(super) async fn queued_event(events: Option<&mut EventStream>) -> Option<Event> {
    let events = events?;
    std::future::poll_fn(|context| {
        std::task::Poll::Ready(match events.poll_next_unpin(context) {
            std::task::Poll::Ready(Some(Ok(event))) => Some(event),
            _ => None,
        })
    })
    .await
}

/// Applies `first` and the engine events already queued behind it, up to
/// [`MAX_COALESCED_EVENTS`], then collects any exit they parked.
///
/// Deltas arrive far faster than a frame, so a fast turn costs one relayout per frame instead of
/// one per token; the bound keeps a pathological producer from starving input.
pub(super) fn apply_engine_events(
    app: &mut App,
    event_rx: &mut mpsc::Receiver<luxe_core::event::Event>,
    first: luxe_core::event::Event,
    exit: &mut TuiExit,
) {
    app.update(Msg::Engine(first));
    for _ in 0..MAX_COALESCED_EVENTS {
        match event_rx.try_recv() {
            Ok(event) => app.update(Msg::Engine(event)),
            Err(_) => break,
        }
    }
    collect_exit(app, exit);
}

/// Applies `first` and whatever the terminal has already queued behind it, up to
/// [`MAX_COALESCED_INPUT`].
///
/// Answers whether *every* event was pointer motion that resolved to nothing, which is the only
/// batch that skips its frame, and how many joined the first.
pub(super) async fn apply_terminal_batch(
    app: &mut App,
    events: &mut Option<EventStream>,
    first: Event,
    exit: &mut TuiExit,
) -> (bool, usize) {
    let mut idle_motion = apply_folding_pastes(app, events, first, exit).await;
    let mut coalesced = 0;
    while may_coalesce(app, coalesced) {
        let Some(event) = queued_event(events.as_mut()).await else {
            break;
        };
        idle_motion &= apply_folding_pastes(app, events, event, exit).await;
        coalesced += 1;
    }
    (idle_motion, coalesced)
}

/// Sleeps out `wait`, or forever when the reveal owes the screen nothing.
pub(super) async fn reveal_sleep(wait: Option<Duration>) {
    match wait {
        Some(wait) => tokio::time::sleep(wait).await,
        None => std::future::pending().await,
    }
}

/// Whether a batch of terminal events may take another one.
///
/// Two reasons to stop early. A copy and a paste request are single slots the loop body reads once
/// an iteration, so a second event that sets either would drop the first on the floor; and an app
/// on its way out has nothing left to apply them to.
pub(super) fn may_coalesce(app: &App, taken: usize) -> bool {
    taken < MAX_COALESCED_INPUT
        && !app.should_quit
        && app.outbox.copy.is_none()
        && !app.outbox.clipboard_paste
}

/// Shortest gap between engine-driven repaints (~30fps, matching the tick).
pub(super) const FRAME_BUDGET: Duration = Duration::from_millis(33);

/// Frame gap while the reveal owes the screen text (~83fps).
///
/// Faster than the tick because this is the one animation whose *step size* the frame rate sets: at
/// 30fps a 250 char/s model would advance the cursor eight characters at a time, which is the
/// lumpiness the reveal exists to remove. The clock is only armed while text is actually pending, so
/// an idle session still costs nothing.
pub(super) const REVEAL_FRAME: Duration = Duration::from_millis(12);

/// How long to wait before the next reveal frame, or `None` when nothing is owed.
pub(super) fn reveal_wait(pending: bool, since_draw: Duration) -> Option<Duration> {
    pending.then(|| REVEAL_FRAME.saturating_sub(since_draw))
}

/// What woke one iteration of the event loop, as far as painting is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Wake {
    /// A key, paste, resize, or a mouse event that did something.
    Input,
    /// An engine event, a clipboard result — anything that changed state off the input path.
    Event,
    /// The animation tick, which changes no state of its own.
    Tick,
    /// Pointer motion that resolved to nothing: no hover target, no selection.
    IdleMotion,
}

/// What to do about the frame for one iteration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FrameDecision {
    /// Paint now.
    Draw,
    /// Something changed, but too soon after the last frame — the next tick paints it.
    Defer,
    /// Nothing to paint.
    Skip,
}

/// Whether this iteration paints, defers, or does nothing.
///
/// Three rules, in order. Pointer motion that resolved to nothing never costs a relayout. Input is
/// drawn on the spot, because a keystroke that waits for a frame feels broken. Everything else is
/// capped at `budget` — and a bare tick only earns a frame when something moves by itself or when
/// the budget deferred a real change onto it, so an idle session paints nothing at all.
pub(crate) fn frame_decision(
    wake: Wake,
    animating: bool,
    pending_frame: bool,
    since_draw: Duration,
    budget: Duration,
) -> FrameDecision {
    match wake {
        Wake::IdleMotion => FrameDecision::Skip,
        Wake::Input => FrameDecision::Draw,
        Wake::Tick if !animating && !pending_frame => FrameDecision::Skip,
        Wake::Tick | Wake::Event => {
            if since_draw < budget {
                FrameDecision::Defer
            } else {
                FrameDecision::Draw
            }
        }
    }
}
