//! Status — the status model, usage, the clock and the prompts that stop it.

use super::*;

/// A full turn (stream → commit → turn end) leaves the status line Idle, not Streaming.
#[tokio::test]
async fn test_turn_end_returns_to_idle() {
    let (tx, _rx) = mpsc::channel(8);
    let keymap = luxe_core::keymap::KeyMap::defaults();
    let config = luxe_core::config::Config::default();
    let mut app = App::new(tx, keymap, config);
    app.update(Msg::Engine(Event::TokenDelta { block: 0, delta: "hi".into() }));
    assert!(matches!(app.status.state, AppState::Streaming));
    let committed = luxe_core::message::Message::assistant("hi");
    app.update(Msg::Engine(Event::MessageCommitted {
        session_id: "test-session".into(),
        entry_id: luxe_core::session::EntryId("entry-1".into()),
        message: committed,
    }));
    app.update(Msg::Engine(Event::TurnEnded { reason: luxe_core::event::StopReason::EndTurn }));
    assert!(
        matches!(app.status.state, AppState::Idle),
        "status must return to Idle once the turn ends"
    );
}

/// A `ContextUsage` event fills in the status-line context percentage.
#[tokio::test]
async fn test_context_usage_sets_percent() {
    let (tx, _rx) = mpsc::channel(8);
    let keymap = luxe_core::keymap::KeyMap::defaults();
    let config = luxe_core::config::Config::default();
    let mut app = App::new(tx, keymap, config);
    app.update(Msg::Engine(Event::ContextUsage { used_tokens: 8_240, context_window: 128_000 }));
    assert!(
        (app.status.context_pct - 6.4375).abs() < 0.01,
        "context_pct should be ~6.44%, got {}",
        app.status.context_pct
    );
    assert_eq!(app.status.context_window, 128_000);
}

#[tokio::test]
async fn state_change_updates_model_and_thinking() {
    let (tx, _rx) = mpsc::channel(8);
    let mut app =
        App::new(tx, luxe_core::keymap::KeyMap::defaults(), luxe_core::config::Config::default());
    app.update(Msg::Engine(Event::StateChanged(luxe_core::event::EngineState {
        model: luxe_core::event::ModelRef::new("anthropic", "claude-sonnet"),
        streaming: false,
        status: "mode: plan".into(),
        mode: luxe_core::mode::PLAN.into(),
        thinking: luxe_core::provider::ThinkingLevel::High,
    })));

    assert_eq!(app.status.model, "claude-sonnet");
    assert_eq!(app.status.thinking, luxe_core::provider::ThinkingLevel::High);
}

#[test]
fn test_status_model_spinner_advances() {
    let mut status = StatusModel { state: AppState::Streaming, ..Default::default() };
    assert_eq!(status.spinner_frame, 0);
    status.tick(false);
    assert_eq!(status.spinner_frame, 1);
    status.tick(false);
    assert_eq!(status.spinner_frame, 2);
    // Idle, it holds still unless a panel is waiting on something
    let mut idle = StatusModel::default();
    idle.tick(false);
    assert_eq!(idle.spinner_frame, 0);
    idle.tick(true);
    assert_eq!(idle.spinner_frame, 1);
}

/// The timer measures against the instant it is handed, not the wall.
///
/// It used to sleep 10 ms and assert the wall had moved, which passes whichever clock the timer
/// reads — so it sat green through every recorded demo printing `0.0s`. Handing it an instant the
/// test chose is the version that can fail.
#[test]
fn test_status_model_elapsed_starts_stops() {
    let mut status = StatusModel::default();
    let now = std::time::Instant::now();
    assert!(status.elapsed(now).is_none(), "no elapsed when idle");

    status.on_turn_start();
    let started = status.turn_started.expect("a turn is under way");
    let four_seconds_on = started + std::time::Duration::from_secs(4);
    assert_eq!(
        status.elapsed(four_seconds_on),
        Some(std::time::Duration::from_secs(4)),
        "the timer reported its own clock rather than the one it was given"
    );

    // A pinned clock can sit behind the start it was pinned against; that is 0, not a panic.
    assert_eq!(
        status.elapsed(started - std::time::Duration::from_secs(1)),
        Some(std::time::Duration::ZERO)
    );

    status.turn_started = None;
    assert!(status.elapsed(now).is_none(), "elapsed must be None after turn ends");
}

/// An answer the engine could not be handed keeps its prompt open, because a prompt that closed
/// over a lost answer left the turn blocked behind nothing anyone could press.
#[tokio::test]
async fn an_answer_that_could_not_be_delivered_keeps_its_prompt() {
    let (tx, mut rx) = mpsc::channel(1);
    let mut app = app_running_a_call(tx.clone());
    let base = std::time::Instant::now();
    app.status.frame_clock = Some(base);
    app.update(Msg::Engine(Event::PermissionRequested {
        id: serde_json::from_value(serde_json::json!("perm:1")).expect("an id"),
        session_id: "session-a".into(),
        tool: "git".into(),
        permission: "git".into(),
        summary: "git commit — more".into(),
        always_patterns: Vec::new(),
    }));
    // The channel is full, the way a stalled engine leaves it
    tx.try_send(luxe_core::engine::Action::RefreshSessions).expect("room for one");
    app.update(Msg::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
    assert!(app.overlays.permission.is_some(), "the answer did not go, so the prompt stays");

    // Room again: the same key answers it
    let _ = rx.try_recv();
    app.update(Msg::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
    assert!(app.overlays.permission.is_none());
    assert!(matches!(
        rx.try_recv(),
        Ok(luxe_core::engine::Action::RespondPermission {
            reply: luxe_core::permission::PermissionReply::Reject,
            ..
        })
    ));
}

/// The wait is reconciled from what is on screen, so every way a prompt closes stops the clock.
///
/// A prompt opens on an event and closes on a keystroke, on an interrupt, or with the turn — four
/// paths, and a reducer that banked the time on only some of them would leave the clock stopped
/// for the rest of the session.
#[tokio::test]
async fn a_prompt_on_screen_stops_the_clock_and_the_card_discounts_it() {
    let (tx, _rx) = mpsc::channel(8);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    app.update(Msg::Engine(Event::SessionLoaded {
        session_id: "session-a".into(),
        messages: Vec::new(),
    }));
    app.update(Msg::Engine(Event::ToolStarted { call_id: "call-1".into(), name: "bash".into() }));
    app.update(Msg::Engine(Event::ToolActivityUpdated(Box::new(ToolActivity::running(
        "session-a",
        "call-1",
        "bash",
        "rm -rf build",
        1,
    )))));
    // Anchored on the turn's own start, which `on_turn_start` reads off the wall clock.
    let base = app.status.turn_started.expect("a turn is under way");
    let at = |secs: u64| base + std::time::Duration::from_secs(secs);
    app.status.frame_clock = Some(base);

    app.status.frame_clock = Some(at(1));
    app.update(Msg::Engine(Event::PermissionRequested {
        id: serde_json::from_value(serde_json::json!("perm:1")).expect("an id"),
        session_id: "session-a".into(),
        tool: "bash".into(),
        permission: "bash".into(),
        summary: "Run: rm -rf build".into(),
        always_patterns: Vec::new(),
    }));
    assert!(app.overlays.permission.is_some());

    // Nothing accrues while the reader is looking at it.
    app.status.frame_clock = Some(at(21));
    app.update(Msg::Tick);
    assert_eq!(app.status.elapsed(at(21)).expect("a turn").as_secs(), 1);

    // Answered at twenty-one seconds, the way the modal actually closes.
    app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
    assert!(app.overlays.permission.is_none(), "the prompt is answered and gone");
    assert_eq!(app.status.user_wait(at(21)).as_secs(), 20);
    // And the agent's own time picks up from the second it had before the prompt.
    app.status.frame_clock = Some(at(23));
    app.update(Msg::Tick);
    assert_eq!(app.status.elapsed(at(23)).expect("a turn").as_secs(), 3);

    // And the card that was blocked discounts it rather than reporting it as the tool's own time.
    let blocked = app
        .tool_items
        .values()
        .find(|item| item.activity.call_id == "call-1")
        .expect("the card the prompt was gating");
    assert_eq!(blocked.waited_on_user_ms / 1000, 20);
    assert!(
        matches!(
            app.convo.first().map(ConversationItem::block),
            Some(ConvoBlock::Tool(item)) if item.waited_on_user_ms / 1000 == 20
        ),
        "the transcript's own copy of the card carries it too"
    );
}

/// The timer reports how long the *agent* has been working, so it stops while a person decides.
///
/// A turn blocked on a permission prompt or the `question` tool is not a turn working, and the
/// figure beside `Ctrl+Esc to interrupt` is read as how long the agent has been at it. Left
/// running it reports the reader's own deliberation back to them.
#[test]
fn the_turn_timer_stops_while_a_person_is_deciding() {
    let mut status = StatusModel::default();
    status.on_turn_start();
    let started = status.turn_started.expect("a turn is under way");
    let at = |secs| started + std::time::Duration::from_secs(secs);
    let elapsed = |status: &StatusModel, secs| {
        status.elapsed(at(secs)).expect("a turn is under way").as_secs()
    };

    assert_eq!(elapsed(&status, 2), 2);

    // A prompt opens at two seconds and the clock stops where it was, however long it stays up.
    status.waiting_on_user(true, at(2));
    assert_eq!(elapsed(&status, 5), 2, "a wait still open does not accrue");
    assert_eq!(elapsed(&status, 30), 2);

    // Answered at thirty-two; the agent's time picks up from the two it had.
    assert_eq!(
        status.waiting_on_user(false, at(32)).map(|waited| waited.as_secs()),
        Some(30),
        "the wait it banked is what the card it blocked has to discount",
    );
    assert_eq!(elapsed(&status, 33), 3);
    assert_eq!(status.user_wait(at(33)).as_secs(), 30);

    // A second wait banks on top of the first rather than replacing it.
    status.waiting_on_user(true, at(33));
    status.waiting_on_user(false, at(43));
    assert_eq!(elapsed(&status, 44), 4);
    assert_eq!(status.user_wait(at(44)).as_secs(), 40);

    // Redundant reconciliations are no-ops — it is called after every message, not per prompt.
    assert!(status.waiting_on_user(false, at(45)).is_none());
    status.waiting_on_user(true, at(45));
    assert!(status.waiting_on_user(true, at(46)).is_none());

    // And a fresh turn owes nothing to the last one's waits.
    status.on_turn_start();
    let restarted = status.turn_started.expect("a turn is under way");
    assert_eq!(
        status.elapsed(restarted + std::time::Duration::from_secs(1)),
        Some(std::time::Duration::from_secs(1))
    );
}

/// A turn's usage events are **per provider call**, so the per-turn figures sum them.
///
/// They used to overwrite, on the theory that each event restated a running total. It does not: a
/// turn that runs twenty tools makes twenty independent calls, and treating the last one as the
/// whole turn is how a session's spend came to be understated several-fold.
#[test]
fn per_turn_figures_sum_every_call_in_the_turn() {
    let mut status = StatusModel::default();
    assert_eq!(status.prompt_tokens, 0);
    assert_eq!(status.completion_tokens, 0);
    assert!(status.cost_usd.is_none());

    status.on_call_usage(100, 50, 800, Some(0.001));
    assert_eq!(status.prompt_tokens, 100);
    assert_eq!(status.completion_tokens, 50);
    assert_eq!(status.cache_read_tokens, 800);
    assert_eq!(status.cost_usd, Some(0.001));

    status.on_call_usage(120, 60, 200, Some(0.002));
    assert_eq!(status.prompt_tokens, 220, "a second call adds to the turn");
    assert_eq!(status.completion_tokens, 110);
    assert_eq!(status.cache_read_tokens, 1_000);
    assert_eq!(status.cost_usd, Some(0.003));

    status.on_turn_start();
    assert_eq!(status.prompt_tokens, 0, "a new turn starts its own per-turn figures");
    assert_eq!(status.cost_usd, None);
}

/// The session figures are the engine's, rendered verbatim — the UI performs no accounting.
#[test]
fn session_totals_are_adopted_from_the_engine_not_derived() {
    use luxe_core::usage::{SessionUsage, SourceUsage, UsageSource, UsageTotals};

    let mut status = StatusModel::default();
    assert_eq!(status.session_cost_label(), None, "nothing spent yet, nothing claimed");

    let agent = UsageTotals {
        calls: 47,
        prompt_tokens: 1_270_000,
        completion_tokens: 11_900,
        cost_usd: 0.4381,
        ..Default::default()
    };
    status.on_session_usage(SessionUsage {
        total: agent,
        by_source: vec![SourceUsage { source: UsageSource::Agent, totals: agent }],
    });
    assert_eq!(status.session.total.calls, 47);
    assert_eq!(status.session_cost_label().as_deref(), Some("$0.44"), "rounded to the cent");

    // A later snapshot replaces the previous one rather than adding to it.
    let grown = UsageTotals { cost_usd: 0.61, ..agent };
    status.on_session_usage(SessionUsage {
        total: grown,
        by_source: vec![SourceUsage { source: UsageSource::Agent, totals: grown }],
    });
    assert_eq!(status.session_cost_label().as_deref(), Some("$0.61"));

    status.reset_session_cost();
    assert_eq!(status.session_cost_label(), None, "loading another session starts a fresh tally");
}

#[test]
fn test_status_model_cost_none_when_price_unknown() {
    let mut status = StatusModel::default();
    // Default model has cost.input = 0.0, so cost should be None.
    status.on_call_usage(100, 50, 0, None);
    assert_eq!(status.cost_usd, None);
}

/// The turn timer reads at every scale a turn reaches, the way a tool card does.
///
/// The break this catches: minutes as a clock (`1:23`) with no hours, so a two-hour turn read
/// `125:03` above the composer while the tool card beside it said `2h 05m`.
#[test]
fn the_turn_timer_reads_at_every_scale() {
    use crate::statusline::format_elapsed_compact as timer;
    use std::time::Duration;
    assert_eq!(timer(Duration::ZERO), "0s");
    // Whole seconds: a tenths digit moves ten times a second beside a bar already moving
    assert_eq!(timer(Duration::from_millis(3_140)), "3s");
    assert_eq!(timer(Duration::from_secs(59)), "59s");
    assert_eq!(timer(Duration::from_secs(60)), "1m 00s");
    assert_eq!(timer(Duration::from_secs(83)), "1m 23s");
    assert_eq!(timer(Duration::from_secs(3_599)), "59m 59s");
    assert_eq!(timer(Duration::from_secs(3_600)), "1h 00m");
    assert_eq!(timer(Duration::from_secs(7_503)), "2h 05m");
}

#[test]
fn test_status_model_format_cost() {
    assert_eq!(StatusModel::format_cost(None), String::new(), "None cost formats to empty");
    assert_eq!(StatusModel::format_cost(Some(0.42)), "$0.42");
    // Under half a cent there is nothing to round to, and "$0.00" would report real work as free.
    assert_eq!(StatusModel::format_cost(Some(0.004)), "<$0.01");
}

fn idle_app() -> App {
    let (tx, _rx) = mpsc::channel(8);
    App::new(tx, luxe_core::keymap::KeyMap::defaults(), luxe_core::config::Config::default())
}

fn engine_reports_streaming(app: &mut App, streaming: bool) {
    app.update(Msg::Engine(Event::StateChanged(luxe_core::event::EngineState {
        streaming,
        ..Default::default()
    })));
}

/// `/compact` shows as under way, and its end leaves the dock idle reading what the summary left.
///
/// The break this catches: nothing set the compacting state, so `/compact` showed no sign of life
/// for the minute a summary takes, and the context figure stayed full until the next turn.
#[tokio::test]
async fn compact_on_request_shows_while_it_runs_and_refreshes_the_context_reading() {
    let mut app = idle_app();
    app.update(Msg::Engine(Event::ContextUsage { used_tokens: 190_000, context_window: 200_000 }));
    app.update(Msg::Engine(Event::CompactionStarted { tokens_before: 190_000 }));
    assert!(matches!(app.status.state, AppState::Compacting), "{:?}", app.status.state);

    app.update(Msg::Engine(Event::CompactionDone { tokens_before: 190_000, tokens_after: 20_000 }));
    assert!(matches!(app.status.state, AppState::Idle), "{:?}", app.status.state);
    assert!((app.status.context_pct - 10.0).abs() < 0.01, "got {}", app.status.context_pct);
}

/// A compaction inside a turn hands the dock back to the turn, whether or not it succeeded.
#[tokio::test]
async fn a_stopped_compaction_mid_turn_returns_to_the_turn() {
    let mut app = idle_app();
    engine_reports_streaming(&mut app, true);
    app.update(Msg::Engine(Event::CompactionStarted { tokens_before: 190_000 }));
    app.update(Msg::Engine(Event::CompactionStopped));
    assert!(matches!(app.status.state, AppState::Streaming), "{:?}", app.status.state);
}

/// The engine's turn reports leave a compaction on show while the turn runs, and end it with the turn.
#[tokio::test]
async fn turn_reports_keep_a_compaction_on_show_until_the_turn_ends() {
    let mut app = idle_app();
    engine_reports_streaming(&mut app, true);
    app.update(Msg::Engine(Event::CompactionStarted { tokens_before: 190_000 }));
    engine_reports_streaming(&mut app, true);
    assert!(matches!(app.status.state, AppState::Compacting), "{:?}", app.status.state);
    engine_reports_streaming(&mut app, false);
    assert!(matches!(app.status.state, AppState::Idle), "{:?}", app.status.state);
}

/// The transcript's notices, in order.
fn system_lines(app: &App) -> Vec<String> {
    let notices = app.convo.iter().filter_map(|item| match item.block() {
        ConvoBlock::System(text) => Some(text.clone()),
        _ => None,
    });
    notices.collect()
}

/// A compaction is in the transcript from the moment it starts, and its line becomes the result.
///
/// The break this catches: a minute of summary with nothing in the conversation to say so (the
/// row above the composer has no label), or a second line added beside the first when it's done.
#[tokio::test]
async fn a_compaction_is_written_down_as_it_starts_and_settled_when_done() {
    let mut app = idle_app();
    app.update(Msg::Engine(Event::CompactionStarted { tokens_before: 579_400 }));
    assert_eq!(system_lines(&app), ["Compacting the conversation — 579.4k of context…"]);
    app.update(Msg::Engine(Event::CompactionDone { tokens_before: 579_400, tokens_after: 28_700 }));
    let done = "Compacted the conversation — 579.4k → 28.7k of context.";
    assert_eq!(system_lines(&app), [done]);
}

/// A compaction that ends without a summary says so on the line that announced it.
///
/// The break this catches: "Compacting the conversation…" left standing after a cancel or a
/// failure, reading as a summary still under way.
#[tokio::test]
async fn a_stopped_compaction_settles_its_line() {
    let mut app = idle_app();
    app.update(Msg::Engine(Event::CompactionStarted { tokens_before: 190_000 }));
    app.update(Msg::Engine(Event::Notice("between".into())));
    app.update(Msg::Engine(Event::CompactionStopped));
    assert_eq!(system_lines(&app), ["Compaction stopped.", "between"]);
}

pub(super) fn type_into(app: &mut App, text: &str) {
    for character in text.chars() {
        app.update(Msg::Key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE)));
    }
}
