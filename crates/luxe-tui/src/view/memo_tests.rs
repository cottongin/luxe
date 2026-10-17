//! Memo tests — the transcript memo never serves a frame a cold layout would draw differently.
//!
//! Everything here drives the production render path and compares the memoized frame against the
//! same app laid out from nothing. A stale row is the one way this optimization can hurt, so every
//! state the transcript can be put into gets asked the question.

use super::tests::{assert_memo_agrees_across, assert_memo_agrees_with_a_cold_layout};
use super::*;
use crate::app::{ConvoBlock, Msg};
use crate::fixture::{seeded_app, FixtureScenario};

const SIZE: (u16, u16) = (140, 44);

fn chat(scenario: FixtureScenario) -> App {
    let mut app = seeded_app("luxe", scenario).expect("the fixture");
    app.update(Msg::TerminalResized { width: SIZE.0, height: SIZE.1 });
    app
}

/// Every chat fixture, drawn twice, has to look the same both times.
#[test]
fn every_chat_fixture_survives_being_drawn_from_the_memo() {
    for scenario in [
        FixtureScenario::ChatIdle,
        FixtureScenario::ChatThinking,
        FixtureScenario::ChatStreaming,
        FixtureScenario::ChatTools,
        FixtureScenario::ChatCardBodies,
        FixtureScenario::ChatWideProse,
        FixtureScenario::ChatToolError,
        FixtureScenario::ChatToolRun,
        FixtureScenario::ChatPending,
        FixtureScenario::ChatPaste,
        FixtureScenario::ChatScreenshot,
    ] {
        let mut app = chat(scenario);
        assert_memo_agrees_with_a_cold_layout(&mut app, SIZE, &format!("{scenario:?}"));
    }
}

/// Text arriving into the tail block, which is what a turn does for minutes at a time.
#[test]
fn a_growing_tail_is_redrawn_and_the_history_above_it_is_not() {
    let mut app = chat(FixtureScenario::ChatIdle);
    app.push_convo(ConvoBlock::Assistant(String::new()));
    for chunk in ["The answer ", "arrives in ", "pieces, and\n\neach ", "one re-wraps."] {
        if let Some(ConvoBlock::Assistant(tail)) =
            app.convo.last_mut().map(crate::app::ConversationItem::block_mut)
        {
            tail.push_str(chunk);
        }
        assert_memo_agrees_with_a_cold_layout(&mut app, SIZE, "a streamed delta");
    }
}

/// Opening a card changes one unit; nothing else may move.
#[test]
fn expanding_a_tool_card_redraws_it() {
    let mut app = chat(FixtureScenario::ChatTools);
    assert_memo_agrees_with_a_cold_layout(&mut app, SIZE, "the fixture");
    let card = app
        .convo
        .iter()
        .position(|item| matches!(item.block(), ConvoBlock::Tool(_)))
        .expect("a tool card");
    if let ConvoBlock::Tool(tool) = app.convo[card].block_mut() {
        tool.expanded = true;
    }
    assert_memo_agrees_with_a_cold_layout(&mut app, SIZE, "expanding a card");

    app.tool_output_expanded = true;
    assert_memo_agrees_with_a_cold_layout(&mut app, SIZE, "expanding every card at once");
}

/// A folded run and the click that opens it.
#[test]
fn opening_a_folded_run_redraws_it() {
    let mut app = chat(FixtureScenario::ChatToolRun);
    assert_memo_agrees_with_a_cold_layout(&mut app, SIZE, "a folded run");
    let fold = app.tool_run_folds().first().map(|fold| fold.range.start);
    if let Some(first) = fold {
        app.update(Msg::ToggleConversationBlockDetails(first));
        assert_memo_agrees_with_a_cold_layout(&mut app, SIZE, "opening a folded run");
    }
}

/// Selection paints a band over whichever block holds it, so moving it has to move the band.
#[test]
fn moving_the_block_selection_moves_its_band() {
    let mut app = chat(FixtureScenario::ChatTools);
    for index in 0..app.convo.len().min(4) {
        app.selected_block = Some(index);
        assert_memo_agrees_with_a_cold_layout(&mut app, SIZE, "selecting a block");
    }
    app.selected_block = None;
    assert_memo_agrees_with_a_cold_layout(&mut app, SIZE, "clearing the selection");
}

/// A resize re-wraps every row, so nothing measured at the old width may survive it.
#[test]
fn a_resize_rewraps_everything() {
    let mut app = chat(FixtureScenario::ChatWideProse);
    for width in [140u16, 100, 72, 180] {
        app.update(Msg::TerminalResized { width, height: 44 });
        assert_memo_agrees_with_a_cold_layout(&mut app, (width, 44), "a resize");
    }
}

/// A theme restyles every span.
#[test]
fn a_theme_change_restyles_everything() {
    let mut app = chat(FixtureScenario::ChatWideProse);
    assert_memo_agrees_with_a_cold_layout(&mut app, SIZE, "the starting theme");
    for theme in ["nord", "catppuccin", "everforest"] {
        app.preview_theme(theme);
        assert_memo_agrees_with_a_cold_layout(&mut app, SIZE, "a theme change");
    }
}

/// A user message opened past its preview cap, and closed again.
#[test]
fn opening_a_long_message_redraws_it() {
    let mut app = chat(FixtureScenario::ChatPaste);
    assert_memo_agrees_with_a_cold_layout(&mut app, SIZE, "a previewed message");
    for index in 0..app.convo.len() {
        app.update(Msg::ToggleConversationBlockDetails(index));
        assert_memo_agrees_with_a_cold_layout(&mut app, SIZE, "opening a message");
    }
}

/// Items leaving the transcript take their rows with them, and the one that becomes the tail
/// has to lose the gap the item above it used to give it.
#[test]
fn removing_items_removes_their_rows() {
    let mut app = chat(FixtureScenario::ChatTools);
    while !app.convo.is_empty() {
        assert_memo_agrees_with_a_cold_layout(&mut app, SIZE, "a shortened transcript");
        app.convo.pop();
    }
    assert_memo_agrees_with_a_cold_layout(&mut app, SIZE, "an emptied transcript");
}

/// A spinner has to keep spinning. Its card comes out of the same memo as everything else, so
/// a card that is reused between ticks is one frozen mid-animation — which reads as a hang.
#[test]
fn a_running_card_is_never_served_from_the_memo() {
    for scenario in [FixtureScenario::ChatToolRun, FixtureScenario::ChatTools] {
        let mut app = chat(scenario);
        app.status.state = crate::app::AppState::Streaming;
        assert!(
            app.convo.iter().any(|item| matches!(
                item.block(),
                ConvoBlock::Tool(tool)
                    if tool.activity.state == luxe_core::ToolActivityState::Running
            )),
            "{scenario:?} has to have something running for this to mean anything"
        );
        for _ in 0..4 {
            assert_memo_agrees_across(&mut app, SIZE, "an animation tick", |app| {
                app.update(Msg::Tick);
            });
        }
    }
}

/// A card that spins has to be *redrawn* while it spins.
///
/// The layout memo is keyed on the card's revision, so a card whose state does not change is
/// served from cache — and a call the model is still writing revises only when its arguments
/// move. Against a provider that delivers those in one lump at the end, that is never: the
/// spinner froze for the whole time the call took to arrive, which reads worse than no spinner.
#[test]
fn a_card_that_spins_is_redrawn_while_it_spins() {
    use luxe_core::ToolActivityState::{Composing, Queued, Running};
    use ratatui::{backend::TestBackend, Terminal};

    let row_of = |app: &mut App, needle: &str| {
        let mut terminal =
            Terminal::new(TestBackend::new(SIZE.0, SIZE.1)).expect("a test terminal");
        terminal.draw(|frame| render(frame, app)).expect("a frame");
        let buffer = terminal.backend().buffer().clone();
        (0..SIZE.1)
            .map(|y| (0..SIZE.0).map(|x| buffer[(x, y)].symbol()).collect::<String>())
            .find(|row| row.contains(needle))
            .unwrap_or_else(|| panic!("no row carrying {needle:?}"))
    };

    for (state, animates) in [(Running, true), (Composing, true), (Queued, false)] {
        let mut app = chat(FixtureScenario::ChatIdle);
        app.update(Msg::Engine(luxe_core::Event::SessionLoaded {
            session_id: "session-a".into(),
            messages: Vec::new(),
        }));
        app.update(Msg::TerminalResized { width: SIZE.0, height: SIZE.1 });
        app.status.state = crate::app::AppState::Streaming;
        app.status.animating = true;
        let mut activity =
            luxe_core::ToolActivity::running("session-a", "call-a", "bash", "vainfo", 1);
        activity.state = state;
        app.update(Msg::Engine(luxe_core::Event::ToolActivityUpdated(Box::new(activity))));

        let before = row_of(&mut app, "vainfo");
        // Ticks and nothing else: the card itself has not changed, only the clock. A whole
        // frame of them, because the sweep is deliberately slower than the render loop — one
        // tick holds the same glyph by design, and asserting on one would be asserting the
        // spinner's cadence here rather than the memo's freshness.
        for _ in 0..crate::glyphs::spinner_ticks_per_frame() {
            app.update(Msg::Tick);
        }
        let after = row_of(&mut app, "vainfo");
        assert_eq!(
            before != after,
            animates,
            "{state:?} should{} advance across a tick\n  before {before:?}\n  after  {after:?}",
            if animates { "" } else { " not" },
        );
    }
}

/// Scrolling moves the window over the rows; it must not change the rows.
#[test]
fn scrolling_serves_the_same_rows() {
    let mut app = chat(FixtureScenario::ChatWideProse);
    for _ in 0..6 {
        app.scroll_state.up_by(3);
        assert_memo_agrees_with_a_cold_layout(&mut app, SIZE, "scrolling up");
    }
    app.scroll_state.to_bottom();
    assert_memo_agrees_with_a_cold_layout(&mut app, SIZE, "returning to the bottom");
}
