use super::{keys::apply_route_action, pacing::MAX_COALESCED_INPUT, *};
use crate::fixture::{seeded_app, FixtureScenario};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use luxe_core::keymap::UiAction;
use std::collections::HashMap;

mod batches;
mod frames;
mod routing;
