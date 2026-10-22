//! Modes — build and plan, the plan hand-off, and finding on screen.

use super::*;

/// An edit replaces the message it edits and the answer to it, in the transcript as in the
/// session the engine branches, rather than leaving both versions on screen.
#[tokio::test]
async fn an_edit_replaces_the_message_on_screen() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut app = one_exchange(tx);
    app.dispatch_action(UiAction::EditResend);
    assert_eq!(app.editor.current_input(), "teh question");
    app.editor.cancel();
    app.editor.insert_text("the question");
    app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
    assert_eq!(transcript(&app), ["you: the question"]);
    assert!(matches!(
        rx.try_recv(),
        Ok(luxe_core::engine::Action::EditResend { content }) if content == "the question"
    ));
}

pub(super) fn mode_app() -> (App, mpsc::Receiver<Action>) {
    let (tx, rx) = mpsc::channel(16);
    let mut app =
        App::new(tx, luxe_core::keymap::KeyMap::defaults(), luxe_core::config::Config::default());
    app.status.model = "test-model".into();
    app.dashboard = None;
    app.current_view = ViewMode::Chat;
    (app, rx)
}

pub(super) fn in_mode(app: &mut App, mode: &str) {
    app.update(Msg::Engine(Event::StateChanged(luxe_core::event::EngineState {
        model: luxe_core::event::ModelRef::new("openai", "test-model"),
        streaming: false,
        status: format!("mode: {mode}"),
        mode: mode.into(),
        thinking: luxe_core::provider::ThinkingLevel::Off,
    })));
}

pub(super) fn screen(app: &mut App, width: u16, height: u16) -> (String, ratatui::buffer::Buffer) {
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| crate::view::render(frame, app)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    let text = (0..height)
        .map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");
    (text, buffer)
}

/// A switch retitles the composer and leaves the transcript alone, however often it happens.
///
/// The break this catches: a line per switch, which turns a few accidental shift+tabs into a
/// scrollback of mode changes.
#[test]
fn a_mode_switch_leaves_the_transcript_alone() {
    let (mut app, _rx) = mode_app();
    let before = app.convo.len();
    for mode in ["plan", "build", "plan", "build", "plan"] {
        in_mode(&mut app, mode);
    }
    assert_eq!(app.convo.len(), before, "a switch is not a transcript event");
    assert_eq!(app.status.mode, "plan");
}

/// Shift+Tab from the composer moves to the next mode; the `/` menu keeps it for itself.
#[test]
fn shift_tab_switches_mode_from_the_composer_only() {
    let (mut app, mut rx) = mode_app();
    in_mode(&mut app, "build");
    let back_tab = || Msg::Key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));

    app.update(back_tab());
    assert!(matches!(rx.try_recv(), Ok(Action::SetMode(mode)) if mode == "plan"));

    in_mode(&mut app, "plan");
    app.update(back_tab());
    assert!(matches!(rx.try_recv(), Ok(Action::SetMode(mode)) if mode == "build"));

    app.update(Msg::Key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE)));
    assert!(app.overlays.slash_flyout.is_some());
    app.update(back_tab());
    assert!(rx.try_recv().is_err(), "shift+tab in the `/` menu switched the mode");
}

/// Where the mode button is, as `(column, row)` of its name: the row right above the composer's
/// top edge, at its left end.
pub(super) fn mode_badge_at(text: &str, mode: &str) -> Option<(u16, u16)> {
    let lines: Vec<&str> = text.lines().collect();
    let top = lines.iter().rposition(|line| line.contains('╭'))?;
    let corner = lines[top].chars().position(|c| c == '╭')?;
    let above = lines.get(top.checked_sub(1)?)?;
    let at = above.find(mode)?;
    let column = above[..at].chars().count();
    // The icon and a pad come first, and nothing else may: the button leads the row
    (column > corner && column <= corner + 4).then_some((column as u16, top as u16 - 1))
}

/// The mode is named on the row above the composer, at its left end, in its own color, and
/// clicking it switches. The frame no longer carries it.
#[test]
fn the_mode_button_leads_the_row_above_the_composer() {
    let (mut app, mut rx) = mode_app();
    in_mode(&mut app, "build");
    let (text, _) = screen(&mut app, 80, 20);
    assert!(mode_badge_at(&text, "build").is_some(), "the default mode is named too:\n{text}");

    in_mode(&mut app, "plan");
    let (text, buffer) = screen(&mut app, 80, 20);
    assert!(!text.contains("plan ─"), "the frame still carries the mode:\n{text}");
    let (column, row) = mode_badge_at(&text, "plan").expect("the mode button");
    assert_eq!(
        buffer[(column, row)].fg,
        app.theme_styles.mode_plan.fg.unwrap(),
        "the button wears the plan color"
    );

    click_at(&mut app, column, row);
    assert!(matches!(rx.try_recv(), Ok(Action::SetMode(mode)) if mode == "build"));
}

/// A running turn's activity bar and time sit to the right of the mode button, which stays put.
#[test]
fn the_activity_row_keeps_the_mode_button_at_its_left() {
    let (mut app, _rx) = mode_app();
    in_mode(&mut app, "plan");
    app.status.state = crate::app::AppState::Streaming;
    app.status.on_turn_start();
    let (text, _) = screen(&mut app, 80, 20);
    let (column, row) = mode_badge_at(&text, "plan").expect("the button during a turn");
    let line = text.lines().nth(usize::from(row)).unwrap();
    let hint = line.find("to interrupt").expect("the activity is on the same row");
    assert!(line[..hint].chars().count() > usize::from(column), "{line}");
}

/// Plan mode's color reaches the question the agent asks, and nothing further.
#[test]
fn plan_mode_tints_the_agent_s_question() {
    let (mut app, _rx) = mode_app();
    in_mode(&mut app, "plan");
    app.overlays.question = Some(crate::question_prompt::QuestionPromptState::new(
        luxe_core::question::QuestionRequestId::new(),
        "Which?".into(),
        vec![luxe_core::question::QuestionOption::new("this")],
        false,
    ));
    let (text, buffer) = screen(&mut app, 80, 24);
    let row = text.lines().position(|line| line.contains("Question")).expect("the question");
    let corner = text.lines().nth(row).unwrap().chars().position(|c| c == '╭').unwrap();
    let plan = app.theme_styles.mode_plan.fg.unwrap();
    assert_eq!(buffer[(corner as u16, row as u16)].fg, plan, "the border");
    let glyph = (corner as u16 + 1..80)
        .find(|&x| !buffer[(x, row as u16)].symbol().trim().is_empty())
        .expect("the title's glyph");
    assert_eq!(buffer[(glyph, row as u16)].fg, plan, "and the title's glyph with it");
}

/// A plan written in plan mode is offered its next step when the turn ends, and the picked step is
/// carried out by the TUI rather than asked of the agent.
#[test]
fn a_written_plan_is_handed_off_by_the_reader() {
    let (mut app, mut rx) = mode_app();
    in_mode(&mut app, "plan");
    app.update(Msg::Engine(Event::PlanWritten { path: "PLAN.md".into() }));
    app.update(Msg::Engine(Event::TurnEnded { reason: luxe_core::StopReason::EndTurn }));
    let prompt = app.overlays.question.as_ref().expect("the handoff is offered");
    assert!(prompt.prompt.contains("PLAN.md is ready"), "{}", prompt.prompt);

    // Enter takes the recommended option: build it here
    app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
    assert!(app.overlays.question.is_none());
    assert!(matches!(rx.try_recv(), Ok(Action::SetMode(mode)) if mode == "build"));
    match rx.try_recv() {
        Ok(Action::SubmitUserMessage { content, .. }) => {
            assert!(content.contains("`PLAN.md`") && content.contains("phase 0"), "{content}")
        }
        other => panic!("the build was not sent: {other:?}"),
    }
    assert!(rx.try_recv().is_err(), "the handoff answered the engine as if it had asked");
}

/// Building from a fresh session waits for that session before it says anything.
#[test]
fn a_fresh_build_is_sent_to_the_new_session() {
    let (mut app, mut rx) = mode_app();
    in_mode(&mut app, "plan");
    app.update(Msg::Engine(Event::PlanWritten { path: "PLAN.md".into() }));
    app.update(Msg::Engine(Event::TurnEnded { reason: luxe_core::StopReason::EndTurn }));
    app.update(Msg::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)));
    app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));

    assert!(matches!(rx.try_recv(), Ok(Action::SetMode(mode)) if mode == "build"));
    assert!(matches!(rx.try_recv(), Ok(Action::NewSession)));
    assert!(rx.try_recv().is_err(), "nothing is sent into the session being left");

    app.update(Msg::Engine(Event::SessionLoaded {
        session_id: "fresh".into(),
        messages: Vec::new(),
    }));
    assert!(
        matches!(rx.try_recv(), Ok(Action::SubmitUserMessage { content, .. }) if content.contains("PLAN.md"))
    );
}

/// Nothing is offered when the turn that wrote the plan has already been left for build.
#[test]
fn no_handoff_once_the_session_is_building() {
    let (mut app, _rx) = mode_app();
    in_mode(&mut app, "plan");
    app.update(Msg::Engine(Event::PlanWritten { path: "PLAN.md".into() }));
    in_mode(&mut app, "build");
    app.update(Msg::Engine(Event::TurnEnded { reason: luxe_core::StopReason::EndTurn }));
    assert!(app.overlays.question.is_none());
}

/// Where `needle` sits on screen, as `(column, row)`, searching from the bottom.
pub(super) fn find_on_screen(text: &str, needle: &str) -> Option<(u16, u16)> {
    let lines: Vec<&str> = text.lines().collect();
    lines.iter().enumerate().rev().find_map(|(row, line)| {
        let at = line.find(needle)?;
        Some((line[..at].chars().count() as u16, row as u16))
    })
}

/// A wide terminal shows the dock and hides the bar, so the dock says the mode; a click on it
/// switches, as the composer's title does.
#[test]
fn the_dock_names_the_mode_and_switches_it() {
    let (mut app, mut rx) =
        crate::fixture::seeded_session("luxe", crate::fixture::FixtureScenario::ChatPlan).unwrap();
    app.overlays.question = None;
    app.update(Msg::TerminalResized { width: 160, height: 40 });
    let (text, buffer) = screen(&mut app, 160, 40);
    let (column, row) = find_on_screen(&text, "mode ").expect("a mode row in the dock");
    let line = text.lines().nth(usize::from(row)).unwrap();
    assert!(
        line[line.find("mode ").unwrap()..]
            .trim_start_matches("mode")
            .trim_start()
            .contains("plan"),
        "{line}"
    );
    let (value, _) = find_on_screen(line, "plan").unwrap();
    assert_eq!(
        buffer[(value, row)].fg,
        app.theme_styles.mode_plan.fg.unwrap(),
        "in the plan color"
    );

    while rx.try_recv().is_ok() {}
    click_at(&mut app, column, row);
    assert!(matches!(rx.try_recv(), Ok(Action::SetMode(mode)) if mode == "build"));

    // In build the row is still there, in body text, so it is a way in as well as a reading
    app.status.mode = "build".into();
    let (text, buffer) = screen(&mut app, 160, 40);
    let (_, row) = find_on_screen(&text, "mode ").expect("a mode row in build too");
    let line = text.lines().nth(usize::from(row)).unwrap();
    let (value, _) = find_on_screen(line, "build").expect("it says build");
    assert_eq!(buffer[(value, row)].fg, app.theme_styles.text.fg.unwrap());
}

/// A narrower terminal shows the bar instead, which leaves the mode to the composer: its button and
/// color say it beside the bar already, and a third copy is noise.
#[test]
fn the_bar_leaves_the_mode_to_the_composer() {
    let (mut app, _rx) =
        crate::fixture::seeded_session("luxe", crate::fixture::FixtureScenario::ChatPlan).unwrap();
    app.overlays.question = None;
    app.update(Msg::TerminalResized { width: 110, height: 30 });
    let (text, buffer) = screen(&mut app, 110, 30);
    let bar = text.lines().last().unwrap();
    assert!(!bar.contains("plan"), "the bar names the mode: {bar}");
    let (column, row) = mode_badge_at(&text, "plan").expect("the composer's mode button");
    assert_eq!(buffer[(column, row)].fg, app.theme_styles.mode_plan.fg.unwrap());
}

/// The model the engine reports is the one the pickers mark, including one the reader never
/// picked: a mode with a model of its own, or an engine that took over from another.
#[test]
fn the_active_model_follows_what_the_engine_reports() {
    use luxe_core::provider::{ModelInfo, ModelRef};
    let (mut app, _rx) = mode_app();
    let entry = |id: &str| crate::model_picker::ModelEntry {
        info: ModelInfo { r#ref: ModelRef::new("openai", id), ..Default::default() },
        label: id.into(),
        configured: true,
        unverified: false,
    };
    let launched = entry("test-model");
    app.set_model_catalog(vec![launched.clone(), entry("planner")], Some(launched.info));
    app.update(Msg::Engine(Event::StateChanged(luxe_core::event::EngineState {
        model: luxe_core::event::ModelRef::new("openai", "planner"),
        streaming: false,
        status: "mode: plan".into(),
        mode: "plan".into(),
        thinking: luxe_core::provider::ThinkingLevel::Off,
    })));
    let active = app.active_model_info.as_ref().map(|info| info.r#ref.id.as_str());
    assert_eq!(active, Some("planner"));
}
