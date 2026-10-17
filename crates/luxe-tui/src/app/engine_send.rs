//! Engine send — the one way the TUI hands the engine an action.
//!
//! Every send goes through [`App::send_action`]: logged once, told to the reader, and reported back
//! so a caller changes its own state only after the engine has the action — `/permissions` must not
//! change the screen before it knows the engine heard.

use super::App;
use luxe_core::engine::Action;

/// What the reader is told when the engine did not take an action.
pub(crate) const ENGINE_BUSY: &str = "The engine is busy — try again in a moment.";

impl App {
    /// Hands `action` to the engine and says whether it went.
    ///
    /// The channel is bounded, so a send fails on a full queue as well as a closed one. Either
    /// way it is logged once, with what was being done, and said where the reader is looking;
    /// a caller changes its own state only after `true`.
    pub(crate) fn send_action(&mut self, action: Action, doing: &'static str) -> bool {
        match self.engine_tx.try_send(action) {
            Ok(()) => true,
            Err(error) => {
                tracing::warn!(%error, doing, "the engine did not take an action");
                self.status.flash_toast(ENGINE_BUSY.to_string());
                false
            }
        }
    }

    /// [`Self::send_action`] without the toast: for a send the reader did not ask for — a focus
    /// report, a refresh after something else finished — where "try again" would name nothing they
    /// did, and for one whose caller says something more specific itself. Logged, not said.
    pub(crate) fn send_quietly(&mut self, action: Action, doing: &'static str) -> bool {
        match self.engine_tx.try_send(action) {
            Ok(()) => true,
            Err(error) => {
                tracing::debug!(%error, doing, "the engine did not take a background action");
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::{seeded_app, seeded_session, FixtureScenario};

    /// A send the engine took is `true` and leaves no toast; one it did not is `false` and says so.
    #[test]
    fn a_send_says_whether_it_went() {
        let (mut app, mut engine) = seeded_session("nord", FixtureScenario::ChatIdle).unwrap();
        assert!(app.send_action(Action::RefreshSessions, "refresh"));
        assert!(matches!(engine.try_recv(), Ok(Action::RefreshSessions)));

        let mut unheard = seeded_app("nord", FixtureScenario::ChatIdle).unwrap();
        assert!(!unheard.send_quietly(Action::RefreshSessions, "refresh"));
        assert_eq!(unheard.status.toast(), None, "a background send says nothing");
        assert!(!unheard.send_action(Action::RefreshSessions, "refresh"));
        assert_eq!(unheard.status.toast(), Some(ENGINE_BUSY));
    }

    /// `/permissions` changes what the screen says only once the engine has been told.
    ///
    /// The break this catches: the flag flipped first and the send's result was dropped, so a lost
    /// "off" left the bar saying prompts were back while the engine still approved every call.
    #[test]
    fn a_permission_change_the_engine_did_not_take_changes_nothing() {
        let mut app = seeded_app("nord", FixtureScenario::ChatIdle).unwrap();
        let registry = crate::commands::CommandRegistry::all();
        let command = registry.parse_slash("/permissions on").unwrap();
        super::super::commands::dispatch(&mut app, command);
        assert!(!app.permission_auto_approve, "the send failed, so nothing changed");
    }

    /// A cancel a live engine did not take leaves the screen saying the turn is still running.
    ///
    /// The break this catches: the screen went idle whatever the send did, so on a full queue the
    /// engine kept running tools while the reader was told it had stopped.
    #[test]
    fn a_cancel_the_engine_did_not_take_leaves_the_turn_running() {
        let mut app = seeded_app("nord", FixtureScenario::ChatIdle).unwrap();
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        tx.try_send(Action::RefreshSessions).unwrap();
        app.engine_tx = tx;
        app.status.state = crate::app::AppState::Streaming;
        super::super::commands::dispatch_action(&mut app, luxe_core::keymap::UiAction::Cancel);
        assert!(matches!(app.status.state, crate::app::AppState::Streaming), "still running");
    }

    /// With no engine left to run anything, a cancel stops the screen anyway.
    #[test]
    fn a_cancel_with_no_engine_left_stops_the_screen() {
        let mut app = seeded_app("nord", FixtureScenario::ChatIdle).unwrap();
        app.status.state = crate::app::AppState::Streaming;
        super::super::commands::dispatch_action(&mut app, luxe_core::keymap::UiAction::Cancel);
        assert!(matches!(app.status.state, crate::app::AppState::Idle), "nothing is running");
    }
}
