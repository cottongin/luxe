//! Pickers — the model, theme and command pickers, and the login that feeds them.

use super::*;

/// An edited `models.json` — or a models.dev refresh landing mid-session — must show up on the
/// next `/model`, not the next launch.
#[tokio::test]
async fn model_picker_rebuilds_its_catalog_each_time_it_opens() {
    use crate::model_picker::{ModelEntry, ModelMenu};
    use luxe_core::provider::{ModelInfo, ModelRef};
    use std::sync::atomic::{AtomicUsize, Ordering};

    let (tx, _rx) = mpsc::channel(100);
    let mut app =
        App::new(tx, luxe_core::keymap::KeyMap::defaults(), luxe_core::config::Config::default());

    // Each call returns one more model than the last, so a cached catalog is detectable.
    let opens = std::sync::Arc::new(AtomicUsize::new(0));
    let counter = opens.clone();
    app.model_sources =
        Some(crate::model_picker::ModelSources::reading(std::sync::Arc::new(move || {
            let nth = counter.fetch_add(1, Ordering::Relaxed) + 1;
            ModelMenu {
                catalog: (0..nth)
                    .map(|i| ModelEntry {
                        info: ModelInfo {
                            r#ref: ModelRef::new("anthropic", format!("model-{i}")),
                            ..Default::default()
                        },
                        label: format!("model-{i}"),
                        configured: true,
                        unverified: false,
                    })
                    .collect(),
                active: None,
            }
        })));

    app.open_model_picker();
    assert_eq!(app.model_catalog.len(), 1);
    app.overlays.model_picker = None;

    app.open_model_picker();
    assert_eq!(app.model_catalog.len(), 2, "the catalog is re-read, not cached from startup");
    assert_eq!(opens.load(Ordering::Relaxed), 2);
}

/// A local server's list arrives after the picker is already drawn, because asking it takes a
/// round trip and a frame may not wait on a socket. What comes back has to reach the list the
/// reader is looking at, not only the catalog behind it.
#[tokio::test]
async fn a_catalog_that_lands_late_reaches_the_open_picker() {
    use crate::model_picker::ModelEntry;
    use luxe_core::provider::{ModelInfo, ModelRef};

    let (tx, _rx) = mpsc::channel(8);
    let mut app =
        App::new(tx, luxe_core::keymap::KeyMap::defaults(), luxe_core::config::Config::default());
    let entry = |id: &str| ModelEntry {
        info: ModelInfo { r#ref: ModelRef::new("lmstudio", id), ..Default::default() },
        label: id.to_string(),
        configured: true,
        unverified: false,
    };

    app.model_catalog = vec![entry("qwen3.8-27b")];
    app.open_model_picker();
    assert_eq!(app.overlays.model_picker.as_ref().map(|picker| picker.match_count()), Some(1));

    // The server had a model loaded that the recorded list did not know about.
    let served = vec![entry("qwen3.8-27b"), entry("gemma-4-12b")];
    app.update(Msg::ModelCatalogRefreshed(served.clone()));
    assert_eq!(app.model_catalog.len(), 2);
    assert_eq!(
        app.overlays.model_picker.as_ref().map(|picker| picker.match_count()),
        Some(2),
        "the open picker grew with it"
    );

    // The same answer twice changes nothing — the probe runs on every open, and a picker that
    // rebuilt itself each time would drop the reader's place for no reason.
    app.overlays.model_picker.as_mut().expect("open").select(1);
    app.update(Msg::ModelCatalogRefreshed(served));
    assert_eq!(
        app.overlays.model_picker.as_ref().and_then(|picker| picker.selected_entry()),
        Some(&entry("gemma-4-12b"))
    );
}

#[test]
fn command_palette_filters_and_dispatches() {
    let (tx, _rx) = mpsc::channel(8);
    let mut app =
        App::new(tx, luxe_core::keymap::KeyMap::defaults(), luxe_core::config::Config::default());

    app.update(Msg::Key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL)));
    assert!(app.overlays.palette.is_some());
    for character in "quit".chars() {
        app.update(Msg::Key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE)));
    }
    app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));

    assert!(app.should_quit, "accepted palette command should run");
    assert!(app.overlays.palette.is_none());
    assert!(app.editor.current_input().is_empty(), "palette text must not leak to the editor");
}

/// A finished login opens the picker on the provider it signed in to, at its top row, and leaves
/// the session's model alone: which model to run is the reader's call, and luxe has no favorite to
/// land them on.
#[tokio::test]
async fn a_finished_login_opens_the_picker_at_the_top_and_chooses_nothing() {
    use crate::model_picker::{ModelEntry, ModelMenu};
    use luxe_core::auth::{CredentialKind, LoginOutcome};
    use luxe_core::provider::{ModelInfo, ModelRef};

    let entry = |provider: &str, id: &str| ModelEntry {
        info: ModelInfo { r#ref: ModelRef::new(provider, id), ..Default::default() },
        label: id.to_string(),
        configured: true,
        unverified: false,
    };
    let catalog = vec![
        entry("anthropic", "claude-sonnet-5"),
        entry("openai", "gpt-5.4"),
        entry("openai", "gpt-5.6"),
    ];
    let outcome = || LoginOutcome {
        provider_id: "openai".into(),
        provider_name: "OpenAI".into(),
        kind: Some(CredentialKind::ApiKey),
        account: Some("acct-1".into()),
        notes: vec!["2 models available".into()],
        advice: vec!["Enable it once in VS Code.".into()],
    };
    let finished = || Msg::Auth(crate::auth_wizard::AuthUiMsg::Finished(Ok(outcome())));
    let picker = |app: &App| app.overlays.model_picker.as_ref().expect("the picker opens").clone();
    let app_with = |catalog: Vec<ModelEntry>| {
        let (tx, _rx) = mpsc::channel(8);
        let mut app = App::new(
            tx,
            luxe_core::keymap::KeyMap::defaults(),
            luxe_core::config::Config::default(),
        );
        let build =
            std::sync::Arc::new(move || ModelMenu { catalog: catalog.clone(), active: None });
        app.model_sources = Some(crate::model_picker::ModelSources::reading(build));
        // The flow reports through the wizard that started it, which is the door the outcome
        // comes back through.
        app.overlays.auth_wizard = Some(crate::auth_wizard::AuthWizardState::open(
            Vec::new(),
            luxe_core::auth::LoginScope::Any,
        ));
        app
    };

    // A first login: nothing chosen, the cursor on the first row, and a caption saying so
    let mut app = app_with(catalog.clone());
    app.update(finished());
    assert_eq!(app.status.model, "", "the login picks no model for the reader");
    assert_eq!(picker(&app).cursor(), 0, "the picker starts at its top");
    assert!(picker(&app).note().iter().any(|line| line.contains("Choose a model to finish")));
    assert_eq!(picker(&app).match_count(), 2, "the models of the provider just signed in to");
    let top = picker(&app).selected_entry().map(ModelEntry::qualified_id);
    assert_eq!(top.as_deref(), Some("openai/gpt-5.4"));

    // A second provider while one is in use: still the top, and Esc keeps what is running
    let mut app = app_with(catalog.clone());
    app.apply_model_selection(catalog[2].info.clone());
    app.update(finished());
    assert_eq!(app.status.model, "gpt-5.6", "the session stays on what it was on");
    assert_eq!(picker(&app).cursor(), 0, "not on the model in use either");
    assert!(picker(&app).note().iter().any(|line| line.contains("esc to stay on gpt-5.6")));

    // And the whole of what the login said survives in the transcript, where the picker's
    // two-line caption cannot carry it.
    let said: Vec<&str> = app
        .convo
        .iter()
        .filter_map(|item| match item.block() {
            ConvoBlock::System(text) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    let report = said.iter().find(|line| line.contains("Signed in to OpenAI")).expect("reported");
    assert!(report.contains("API key"), "{report}");
    assert!(report.contains("acct-1"), "{report}");
    assert!(report.contains("2 models available"), "{report}");
    assert!(report.contains("Enable it once in VS Code."), "the advice is not dropped: {report}");
}

/// The three ways out of the image viewer, and the keys that must not be one.
///
/// Enter is the point: it submits everywhere else, and a reader who pressed it to dismiss a
/// picture would have sent whatever was in the composer.
#[tokio::test]
async fn the_image_viewer_closes_on_the_keys_that_mean_close_and_no_others() {
    let (tx, _rx) = mpsc::channel(8);
    let mut app =
        App::new(tx, luxe_core::keymap::KeyMap::defaults(), luxe_core::config::Config::default());
    let open = |a: &mut App| a.overlays.image_viewer = Some(crate::graphics::ImageKey::new("img"));

    for (code, modifiers) in [
        (KeyCode::Esc, KeyModifiers::NONE),
        (KeyCode::Char('q'), KeyModifiers::NONE),
        (KeyCode::Char('i'), KeyModifiers::ALT),
    ] {
        open(&mut app);
        app.update(Msg::Key(KeyEvent::new(code, modifiers)));
        assert!(app.overlays.image_viewer.is_none(), "{code:?}+{modifiers:?} puts the image away");
    }

    for (code, modifiers) in [
        (KeyCode::Enter, KeyModifiers::NONE),
        (KeyCode::Char('x'), KeyModifiers::NONE),
        (KeyCode::Char('q'), KeyModifiers::ALT),
        (KeyCode::Char('i'), KeyModifiers::NONE),
        (KeyCode::Esc, KeyModifiers::ALT),
    ] {
        open(&mut app);
        app.update(Msg::Key(KeyEvent::new(code, modifiers)));
        assert!(app.overlays.image_viewer.is_some(), "{code:?}+{modifiers:?} is not a way out");
    }
    app.overlays.image_viewer = None;
}

/// A picker of models nobody can authenticate is worse than no picker: choosing one looks like
/// progress and fails on the first turn. With nothing configured the door leads to setup, which
/// is the step that actually exists.
#[tokio::test]
async fn with_no_provider_the_model_picker_opens_setup_instead() {
    let (tx, _rx) = mpsc::channel(8);
    let mut app =
        App::new(tx, luxe_core::keymap::KeyMap::defaults(), luxe_core::config::Config::default());
    app.auth_service = Some(std::sync::Arc::new(crate::fixture::UnconfiguredAuth));
    // Which is what the catalog looks like on that machine: the models on offer are the ones a
    // configured provider can answer for.
    app.model_catalog = Vec::new();

    app.open_model_picker();
    assert!(app.overlays.model_picker.is_none(), "there is nothing to choose between yet");
    assert_eq!(app.current_view, ViewMode::Configuration);
    assert!(app.overlays.configuration.is_some());

    // And a submission with no model says which step is missing, rather than naming the one the
    // reader cannot take.
    let said: Vec<&str> = app
        .convo
        .iter()
        .filter_map(|item| match item.block() {
            ConvoBlock::System(text) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(said.iter().any(|line| line.contains("No provider is configured")), "{said:?}");
}

/// The picker's whole point is the preview: arrowing onto a theme has to repaint the app, not just
/// move a cursor. This drives the real key path rather than the picker in isolation.
#[tokio::test]
async fn arrowing_through_the_theme_picker_repaints_the_app() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let (tx, _rx) = mpsc::channel(100);
    let config = luxe_core::config::Config {
        theme: Some("luxe".into()),
        ..luxe_core::config::Config::default()
    };
    let mut app = App::new(tx, luxe_core::keymap::KeyMap::defaults(), config);
    let opened_with = app.theme_styles.accent.fg;

    app.open_theme_picker();
    assert!(app.overlays.theme_picker.is_some(), "/theme opens the picker");

    // Walk until the accent actually changes — the list is alphabetical, and a neighbor that
    // happened to share an accent with `luxe` would make this vacuous.
    let mut moved = false;
    for _ in 0..12 {
        app.update(Msg::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)));
        if app.theme_styles.accent.fg != opened_with {
            moved = true;
            break;
        }
    }
    assert!(moved, "moving the cursor never repainted the app");
    assert!(app.overlays.theme_picker.is_some(), "and the picker stays open while previewing");

    // Esc puts back exactly what we arrived with, and records nothing.
    app.update(Msg::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
    assert!(app.overlays.theme_picker.is_none(), "Esc closes it");
    assert_eq!(app.theme_styles.accent.fg, opened_with, "a cancelled preview leaves no trace");
    assert_eq!(app.config.theme.as_deref(), Some("luxe"), "and changes nothing in the config");
}

/// Enter commits: the app keeps the previewed theme and the session remembers it.
#[tokio::test]
async fn keeping_a_theme_holds_it_for_the_session() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let home = tempfile::tempdir().expect("temp dir");
    let _config_home = crate::test_env::TempConfigHome::at(home.path());

    let (tx, _rx) = mpsc::channel(100);
    let config = luxe_core::config::Config {
        theme: Some("luxe".into()),
        ..luxe_core::config::Config::default()
    };
    let mut app = App::new(tx, luxe_core::keymap::KeyMap::defaults(), config);
    app.open_theme_picker();
    app.update(Msg::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)));
    let previewed = app.overlays.theme_picker.as_ref().expect("open").current().to_string();
    app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));

    assert!(app.overlays.theme_picker.is_none(), "Enter closes it");
    assert_eq!(app.config.theme.as_deref(), Some(previewed.as_str()), "the session remembers");
}

/// A theme that did not load, or a configured name that matches nothing, is said in the
/// conversation — the fallback alone looks like a theme that is not very different.
#[test]
fn a_theme_that_fell_back_says_so() {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("broken.json"), "{ not json").expect("write");
    let registry = luxe_core::theme::ThemeRegistry::with_themes_in(dir.path());
    let config = Config { theme: Some("hotpink".into()), ..Config::default() };

    let notices = crate::app::theme_notices(&config, &registry);
    assert_eq!(notices.len(), 2, "{notices:?}");
    assert!(notices[0].contains("broken.json"), "{notices:?}");
    assert!(notices[1].contains("`hotpink`") && notices[1].contains("themes/hotpink.json"));

    let fine = Config { theme: Some("nord".into()), ..Config::default() };
    assert!(crate::app::theme_notices(&fine, &luxe_core::theme::ThemeRegistry::new()).is_empty());
}

/// A re-themed desktop repaints only a session that is following it: a reader who chose a theme
/// keeps it, and one mid-picker keeps the preview they are judging.
#[test]
fn a_desktop_retheme_leaves_a_chosen_theme_alone() {
    let (tx, _rx) = tokio::sync::mpsc::channel::<luxe_core::engine::Action>(64);
    let config = Config { theme: Some("nord".into()), ..Default::default() };
    let mut app = App::new(tx, KeyMap::defaults(), config);
    let marker = ratatui::style::Style::default().bg(ratatui::style::Color::Rgb(1, 2, 3));
    app.theme_styles.background = marker;
    app.update(Msg::DesktopThemeChanged);
    assert_eq!(app.theme_styles.background, marker, "chosen themes stay put");

    app.config.theme = None;
    app.open_theme_picker();
    app.theme_styles.background = marker;
    app.update(Msg::DesktopThemeChanged);
    assert_eq!(app.theme_styles.background, marker, "the picker's preview stays put");

    app.overlays.theme_picker = None;
    app.update(Msg::DesktopThemeChanged);
    assert_ne!(app.theme_styles.background, marker, "a following session repaints");
}

thread_local! {
    static THEME_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The built-ins, counted, so a test can see when the themes are read.
fn counted_themes() -> luxe_core::theme::ThemeRegistry {
    THEME_READS.with(|reads| reads.set(reads.get() + 1));
    luxe_core::theme::ThemeRegistry::new()
}

/// The themes are read again on opening `/theme`, on `/reload` and on every desktop re-theme, and
/// not by a preview or setup.
///
/// The break this catches: a re-theme while the reader had chosen another theme, or had the
/// picker open, kept the desktop theme's old palette, and choosing it later applied stale colors.
#[test]
fn the_themes_are_read_again_only_where_they_may_have_changed() {
    let (tx, _rx) = tokio::sync::mpsc::channel::<luxe_core::engine::Action>(64);
    let config = Config { theme: Some("nord".into()), ..Default::default() };
    let mut app = App::new_fixture(tx, KeyMap::defaults(), config);
    app.themes = crate::app::environment::Themes::read(counted_themes);
    let reads = || THEME_READS.with(std::cell::Cell::get);
    let start = reads();

    app.preview_theme("nord");
    app.open_setup(crate::configuration::SetupMode::Init);
    assert_eq!(reads(), start, "a preview or setup reads nothing");
    app.overlays.configuration = None;

    app.update(Msg::DesktopThemeChanged);
    assert_eq!(reads(), start + 1, "a chosen theme still learns the desktop's new palette");
    app.open_theme_picker();
    assert_eq!(reads(), start + 2, "the picker shows a theme file added since");
    app.update(Msg::DesktopThemeChanged);
    assert_eq!(reads(), start + 3, "an open picker learns the new palette too");

    let repo = tempfile::tempdir().expect("temp dir");
    crate::app::commands::reload_config(&mut app, repo.path());
    assert_eq!(reads(), start + 4, "/reload reads them again");
}
