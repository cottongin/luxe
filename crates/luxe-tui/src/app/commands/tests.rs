use super::*;
use crate::fixture::{seeded_app, seeded_session, FixtureScenario};
use crate::guided_review::GuidedReviewLoadState;
use luxe_core::keymap::KeyMap;
use tokio::sync::mpsc;

/// Runs a command the way the palette, the flyout and a typed `/…` all do.
pub(super) fn run(app: &mut App, command: &crate::commands::Command) {
    dispatch(app, command.clone());
}

mod answering;
mod github;
mod opening;
mod palette;
mod review_target;
mod slash;

use answering::*;
