//! Chat fixtures — the transcript surface in each of its states.
//!
//! Idle, reasoning, streaming, tool cards, questions, images, the two paste/pending states, and
//! the guide's pictures.
//! Each seeds only what its screenshot is about, so a change to one demo cannot quietly restage
//! another.

mod cards;
mod conversation;
mod guide;
mod tool_run;

use super::*;

/// Seeds `app` for a chat scenario, reporting whether this family owns it.
///
/// `Ok(false)` means another family does — the caller tries the next one. An `Err` is a fixture
/// that claimed the scenario and then could not build it, which is a bug in the fixture.
pub(super) fn seed(app: &mut App, scenario: FixtureScenario) -> Result<bool, String> {
    match scenario {
        FixtureScenario::ChatIdle => conversation::seed_idle(app),
        FixtureScenario::ChatThinking => conversation::seed_thinking(app),
        FixtureScenario::ChatStreaming => conversation::seed_streaming(app),
        FixtureScenario::ChatTools => cards::seed_tool_cards(app),
        FixtureScenario::ChatCardBodies => cards::seed_card_bodies(app)?,
        FixtureScenario::ChatWideProse => conversation::seed_wide_prose(app),
        FixtureScenario::ChatToolError => cards::seed_tool_error(app),
        FixtureScenario::ChatToolRun => tool_run::seed_tool_run(app),
        FixtureScenario::ChatQuestion => conversation::seed_question(app),
        FixtureScenario::ChatPlan => conversation::seed_plan(app)?,
        FixtureScenario::ChatScreenshot => conversation::seed_screenshot(app)?,
        FixtureScenario::ChatPaste => conversation::seed_paste(app),
        FixtureScenario::ChatPending => conversation::seed_pending(app),
        FixtureScenario::ChatTurn => guide::seed_turn(app),
        FixtureScenario::ChatPermission => guide::seed_permission(app),
        FixtureScenario::ChatPalette => guide::seed_palette(app),
        FixtureScenario::ChatPlanReady => guide::seed_plan_ready(app),
        _ => return Ok(false),
    }
    Ok(true)
}
