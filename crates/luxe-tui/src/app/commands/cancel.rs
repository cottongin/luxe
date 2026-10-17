//! Cancel — asking the engine to stop, and showing it only once it was asked.

use super::super::{App, AppState};
use luxe_core::engine::Action;

/// Interrupts the turn. The screen stops only once the engine has the request, or once there is
/// no engine left to be running anything.
pub(in crate::app) fn cancel(app: &mut App) -> bool {
    let working = app.turn_in_flight() || super::super::events::has_unfinished_tools(app);
    // a live engine that did not hear the cancel is still working, and the screen must say
    // so; a closed one is not working at all, so the screen can stop with it
    if !app.send_action(Action::Cancel, "cancel") && !app.engine_tx.is_closed() {
        return true;
    }
    // The turn is over when the engine says so; until then this is a request
    if working {
        app.interrupt_sent_at = Some(app.frame_now());
    }
    // Interrupting means stop, not "finish the animation first": show what arrived.
    app.reveal_catch_up();
    app.streaming = false;
    app.status.state = AppState::Idle;
    app.status.turn_started = None;
    app.status.active_tool = None;
    // Cancel means stop: the engine drops its buffered steering too, so nothing still held
    // can arrive. It comes back to the composer rather than the transcript — interrupting is
    // usually the first half of "let me put that differently".
    let restored = app.cancel_pending_submissions();
    if restored > 0 {
        app.status.flash_toast(match restored {
            1 => "Interrupted · 1 message back in the composer".to_string(),
            n => format!("Interrupted · {n} messages back in the composer"),
        });
    }
    true
}
