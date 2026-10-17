//! The overlay pair matrix — which of two open overlays a key, a paste and a click reach.
//!
//! The key, paste and pointer ladders each list the overlays in their own order, and they do not
//! agree. This pins what each does today for every pair, through the loop's real input path, so
//! grouping the overlays cannot quietly change who answers. `overlay_matrix.txt` is the record:
//! one line per pair, naming what changed for each probe (`-` when nothing did).

use super::keys::apply_terminal_event;
use super::TuiExit;
use crate::app::{App, ViewMode};
use crate::fixture::{seeded_app, FixtureScenario};
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

pub(crate) const OVERLAYS: [&str; 18] = [
    "palette",
    "slash",
    "mention",
    "model",
    "thinking",
    "theme",
    "auth",
    "permission",
    "question",
    "menu",
    "restack",
    "github",
    "card",
    "context",
    "sessions",
    "configuration",
    "image",
    "keys",
];

pub(crate) fn open(app: &mut App, overlay: &str) {
    let menu = || {
        let context = crate::commands::CommandContext::default();
        crate::commands::CommandRegistry::all().arrange(&context, &app.keymap, &[])
    };
    match overlay {
        "palette" => app.overlays.palette = Some(crate::palette::PaletteState::open(menu())),
        "slash" => {
            app.overlays.slash_flyout = Some(crate::slash_flyout::SlashFlyoutState::new(menu()));
        }
        "mention" => {
            let provider = app.file_provider.clone();
            let root = std::path::PathBuf::from("/work");
            let state = crate::mention_flyout::MentionFlyoutState::new(
                provider,
                root,
                &[],
                Vec::new(),
                &[],
            );
            app.overlays.mention = Some(state);
        }
        "model" => {
            app.overlays.model_picker =
                Some(crate::model_picker::ModelPickerState::open(vec![], None))
        }
        "thinking" => {
            let level = app.status.thinking;
            let picker = crate::thinking_picker::ThinkingPickerState::open(level, None);
            app.overlays.thinking_picker = Some(picker);
        }
        "theme" => {
            let themes = vec!["nord".to_string(), "dark".to_string()];
            app.overlays.theme_picker =
                Some(crate::theme_picker::ThemePickerState::open(themes, "nord"));
        }
        "auth" => {
            let scope = luxe_core::auth::LoginScope::Any;
            app.overlays.auth_wizard =
                Some(crate::auth_wizard::AuthWizardState::open(Vec::new(), scope));
        }
        "permission" => {
            let id = luxe_core::permission::PermissionRequestId::new();
            let prompt = crate::permission_prompt::PermissionPromptState::new(
                id,
                "bash".into(),
                "Run: ls".into(),
                Vec::new(),
            );
            app.overlays.permission = Some(prompt);
        }
        "question" => {
            let option = |label: &str| luxe_core::question::QuestionOption {
                label: label.into(),
                description: None,
                recommended: false,
            };
            let id = luxe_core::question::QuestionRequestId::new();
            let options = vec![option("one"), option("two")];
            let prompt = crate::question_prompt::QuestionPromptState::new(
                id,
                "Which?".into(),
                options,
                false,
            );
            app.overlays.question = Some(prompt);
        }
        "menu" => {
            let menu = crate::context_menu::ContextMenu::for_file((10, 10), "src/lib.rs", true);
            app.overlays.context_menu = Some(menu);
        }
        "restack" => app.overlays.restack = Some(crate::restack_panel::RestackPanel::Planning),
        "github" => app.overlays.github = Some(crate::github_panel::GithubPanel::new(&[])),
        "card" => {
            app.overlays.pull_request_card = Some(crate::github_detail::PullRequestCard::other(7))
        }
        "context" => {
            use crate::context_files::{ContextFileRow, ContextFilesState, ContextKind};
            let row = |label: &str| ContextFileRow {
                kind: ContextKind::Instructions,
                label: label.into(),
                scope: "this project".into(),
                path: std::path::PathBuf::from("/work").join(label),
                creates: false,
                reach: None,
                duplicate: false,
            };
            let rows = vec![row("AGENTS.md"), row("CLAUDE.md")];
            app.overlays.context_files = Some(ContextFilesState::from_rows(rows));
        }
        "sessions" => {
            app.overlays.session_picker =
                Some(crate::session_picker::SessionPickerState::new(Vec::new()));
            app.current_view = ViewMode::SessionPicker;
        }
        "configuration" => {
            let state = crate::configuration::ConfigurationState::open(&[], Vec::new());
            app.overlays.configuration = Some(state);
            app.current_view = ViewMode::Configuration;
        }
        "image" => app.overlays.image_viewer = Some(crate::graphics::ImageKey::new("matrix")),
        "keys" => app.overlays.keys_card = true,
        other => panic!("no overlay {other}"),
    }
}

/// Every overlay as its `Debug` shows it, the composer's draft, and the view.
fn snapshot(app: &App) -> Vec<String> {
    let debug = |value: &dyn std::fmt::Debug| format!("{value:?}");
    vec![
        debug(&app.overlays.palette),
        debug(&app.overlays.slash_flyout),
        debug(&app.overlays.mention),
        debug(&app.overlays.model_picker),
        debug(&app.overlays.thinking_picker),
        debug(&app.overlays.theme_picker),
        debug(&app.overlays.auth_wizard),
        debug(&app.overlays.permission),
        debug(&app.overlays.question),
        debug(&app.overlays.context_menu),
        debug(&app.overlays.restack),
        debug(&app.overlays.github),
        debug(&app.overlays.pull_request_card),
        debug(&app.overlays.context_files),
        debug(&app.overlays.session_picker),
        debug(&app.overlays.configuration),
        debug(&app.overlays.image_viewer),
        debug(&app.overlays.keys_card),
        app.editor.current_input().to_string(),
        debug(&app.current_view),
    ]
}

/// The names of what `probe` changed, or `-`.
fn changed(app: &mut App, probe: impl FnOnce(&mut App)) -> String {
    let before = snapshot(app);
    probe(app);
    let after = snapshot(app);
    let names = OVERLAYS.iter().chain(&["composer", "view"]);
    let moved: Vec<&str> = before
        .iter()
        .zip(&after)
        .zip(names)
        .filter(|((was, now), _)| was != now)
        .map(|(_, name)| *name)
        .collect();
    match moved.is_empty() {
        true => "-".to_string(),
        false => moved.join(","),
    }
}

fn with_pair(first: &str, second: &str) -> (App, tokio::sync::mpsc::Receiver<luxe_core::Action>) {
    let mut app = seeded_app("nord", FixtureScenario::ChatIdle).expect("fixture");
    let (tx, rx) = tokio::sync::mpsc::channel(64);
    app.engine_tx = tx;
    app.mouse_enabled = true;
    // The permission prompt ignores keys for a moment after it opens; a clock that never moves
    // keeps every probe inside that moment, so a slow machine records the same table
    app.status.frame_clock = Some(std::time::Instant::now());
    open(&mut app, first);
    open(&mut app, second);
    (app, rx)
}

pub(crate) fn send(app: &mut App, event: Event) {
    let mut exit = TuiExit::Quit;
    apply_terminal_event(app, event, &mut exit);
}

fn key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn click(app: &mut App) {
    use ratatui::{backend::TestBackend, Terminal};
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).expect("a terminal");
    terminal.draw(|frame| crate::view::render(frame, app)).expect("a frame");
    for kind in [MouseEventKind::Down(MouseButton::Left), MouseEventKind::Up(MouseButton::Left)] {
        let mouse = MouseEvent { kind, column: 50, row: 15, modifiers: KeyModifiers::NONE };
        send(app, Event::Mouse(mouse));
    }
}

/// One line per pair: what Esc, Down, a paste and a click in the middle of the screen changed.
fn matrix() -> String {
    let mut lines = Vec::new();
    for (index, first) in OVERLAYS.iter().enumerate() {
        for second in &OVERLAYS[index + 1..] {
            let probe = |probe: &dyn Fn(&mut App)| {
                let (mut app, _rx) = with_pair(first, second);
                changed(&mut app, |app| probe(app))
            };
            let esc = probe(&|app| send(app, key(KeyCode::Esc)));
            let down = probe(&|app| send(app, key(KeyCode::Down)));
            let paste = probe(&|app| send(app, Event::Paste("z".into())));
            let click = probe(&click);
            lines.push(format!(
                "{first}+{second}: esc={esc} down={down} paste={paste} click={click}"
            ));
        }
    }
    lines.join("\n") + "\n"
}

/// Who answers Esc, Down, a paste and a click when two overlays are open, for every pair.
///
/// The break this catches: grouping or reordering the overlays changing which one an input
/// reaches. Set `LUXE_OVERLAY_MATRIX` to a path to write the current table there.
#[tokio::test]
async fn every_pair_of_overlays_answers_as_recorded() {
    let actual = matrix();
    if let Ok(path) = std::env::var("LUXE_OVERLAY_MATRIX") {
        std::fs::write(path, &actual).expect("write the matrix");
    }
    let recorded = include_str!("overlay_matrix.txt");
    for (want, got) in recorded.lines().zip(actual.lines()) {
        assert_eq!(got, want, "an overlay pair answers differently");
    }
    assert_eq!(recorded.lines().count(), actual.lines().count());
}
