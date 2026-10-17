//! Fixture seeds — deterministic production-app states for screenshot review.
//!
//! These scenarios avoid user configuration, sessions, and project data. The example runner
//! renders these `App` instances through the normal view path.

mod auth;
mod capture;
mod chat;
mod corpus;
mod demo;
mod demo_spec;
#[cfg(test)]
mod flagged;
mod github;
mod large;
mod mention;
mod models;
mod peek;
mod pull_request;
mod review;
pub(crate) mod scaled;
mod scenario;
mod shell;
mod workspace;

use capture::fixture_capture;
#[cfg(test)]
pub(crate) use corpus::fixture_source;
use corpus::{fixture_guided_review, fixture_guided_review_with_noise};
#[cfg(test)]
pub(crate) use flagged::flagged_review;
use models::configurable_models;
use workspace::{changes_fixture_file, dock_session_usage, external_fixture_file};

pub use github::{pull_request_overview, seed_github};
pub use scaled::fixture_guided_review_scaled;

use crate::app::{App, ConvoBlock, MidTurnDelivery, Msg, ToolItem, ViewMode};
use crate::dashboard::{DashboardState, PickerKind, PreviewRole, PreviewTurn, SessionSummary};
use crate::session_picker::{SessionEntry, SessionPickerState};
pub use auth::{seed_login_wizard, FixtureAuth, LoginFixture, UnconfiguredAuth};
pub use demo::fixture_demo_guided_review;
pub use large::fixture_large_guided_review;
use luxe_core::config::Config;
use luxe_core::event::{Event, GuidedReviewFailure, GuidedReviewFailureKind};
use luxe_core::guided_review::{
    FindingAnchor, FindingId, FindingSeverity, FindingState, ReviewFinding, ReviewTarget,
};
use luxe_core::keymap::KeyMap;
use luxe_core::theme::ThemeRegistry;
use luxe_core::{ReviewLoadRequestId, ToolActivity, ToolActivityState};
pub use mention::seed_mention_picker;
use models::fixture_model;
pub use peek::{filter_tree, focus_preview, preview_visual, seed_file_view};
pub use pull_request::fixture_pull_request_review;
pub use scenario::FixtureScenario;
use tokio::sync::mpsc;
pub(crate) use workspace::dock_workspace_snapshot;
pub use workspace::seed_plan_usage;

/// A light palette, for the light-mode paths no built-in reaches any more.
///
/// Luxe ships dark themes only, but a reader's own theme file can be light, and the code that picks
/// ink for a light canvas still has to be right for them. This is Catppuccin Latte as Luxe shipped
/// it until the light themes were retired, kept so those paths keep a test that can fail.
pub const LIGHT_THEME_JSON: &str = include_str!("fixture/light-theme.json");

/// [`LIGHT_THEME_JSON`], resolved and styled the way a `/theme` switch would style it.
///
/// Set as `app.theme_styles`, it is exactly what [`App::preview_theme`] does for a real theme.
pub fn light_theme_styles() -> crate::theme::styles::ThemeStyles {
    let theme: luxe_core::theme::Theme =
        serde_json::from_str(LIGHT_THEME_JSON).expect("the light fixture is a valid theme");
    crate::theme::styles::ThemeStyles::from_resolved(&theme.resolve_aliases(), "light-fixture")
}

/// Builds a production [`App`] seeded with stable display data for screenshot review.
///
/// The engine receiver is dropped, which is right for a still: nothing is going to answer. A tool
/// that submits a prompt wants [`seeded_session`] instead, or the composer reports a closed channel.
pub fn seeded_app(theme: &str, scenario: FixtureScenario) -> Result<App, String> {
    seeded_session(theme, scenario).map(|(app, _)| app)
}

/// [`seeded_app`], with the engine channel's receiving end handed back alive.
///
/// Submitting from the composer sends an `Action` inward, and a dropped receiver turns that into
/// "engine unavailable — channel closed" on screen. A recording that types a prompt and presses
/// Enter therefore has to keep the receiver, even though it never reads it: the point is that the
/// send succeeds and the UI does what it really does.
pub fn seeded_session(
    theme: &str,
    scenario: FixtureScenario,
) -> Result<(App, mpsc::Receiver<luxe_core::Action>), String> {
    if ThemeRegistry::new().get(theme).is_none() {
        return Err(format!("unknown built-in theme: {theme}"));
    }

    let config = Config { theme: Some(theme.into()), ..Config::default() };
    let (engine_tx, engine_rx) = mpsc::channel(64);
    let mut app = App::new_fixture(engine_tx, KeyMap::defaults(), config);
    app.headless = true;

    // Each family claims the scenarios it owns; the first to recognize this one seeds it.
    if !chat::seed(&mut app, scenario)?
        && !review::seed(&mut app, scenario)?
        && !shell::seed(&mut app, scenario)?
    {
        return Err(format!("no fixture seeds the {scenario:?} scenario"));
    }
    // Overlays that annotate against the active model (the thinking picker graying out levels it
    // can't honor) need one to describe, or a screenshot shows the every-level-available case only.
    // The Configuration surface is the exception: it is where a model gets *picked*, so it keeps
    // the catalog it seeded and has no active model yet.
    if scenario != FixtureScenario::Configuration {
        app.set_model_catalog(Vec::new(), Some(fixture_model()));
    }

    Ok((app, engine_rx))
}

#[cfg(test)]
mod tests;
