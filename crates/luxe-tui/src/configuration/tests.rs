#![allow(clippy::unwrap_used)]

use super::*;
use crossterm::event::KeyModifiers;
use luxe_core::auth::{AuthMethod, CredentialKind, StoredCredentialInfo};

pub(super) fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

pub(super) fn files() -> Vec<ConfigFile> {
    vec![
        ConfigFile {
            name: "config.json",
            detail: "Theme, dock, statusline",
            path: PathBuf::from("/home/dev/.config/luxe/config.json"),
        },
        ConfigFile {
            name: "models.json",
            detail: "Providers and models",
            path: PathBuf::from("/home/dev/.config/luxe/models.json"),
        },
        ConfigFile {
            name: "auth.json",
            detail: "Credentials",
            path: PathBuf::from("/home/dev/.config/luxe/auth.json"),
        },
    ]
}

pub(super) fn provider(id: &str, status: CredentialStatus) -> ProviderOption {
    ProviderOption {
        id: id.to_string(),
        name: id.to_uppercase(),
        methods: vec![AuthMethod::api_key(None)],
        status,
        note: None,
        console_url: None,
        keyless: false,
        needs_endpoint: false,
    }
}

pub(super) fn state() -> ConfigurationState {
    ConfigurationState::open(&[], files())
}

#[test]
fn each_row_opens_the_list_of_providers_it_names() {
    let mut config = state();
    assert_eq!(
        config.on_key(&key(KeyCode::Enter)),
        ConfigOutcome::SignIn(LoginScope::Subscription)
    );

    config.on_key(&key(KeyCode::Down));
    assert_eq!(config.on_key(&key(KeyCode::Enter)), ConfigOutcome::SignIn(LoginScope::ApiKey));

    config.on_key(&key(KeyCode::Down));
    assert_eq!(
        config.on_key(&key(KeyCode::Enter)),
        ConfigOutcome::SignIn(LoginScope::LocalOrCustom)
    );
}

/// The numbers are drawn beside the rows, so pressing one has to run that row.
#[test]
fn a_numbered_row_runs_without_arrowing_to_it() {
    let mut config = state();
    assert_eq!(config.on_key(&key(KeyCode::Char('2'))), ConfigOutcome::SignIn(LoginScope::ApiKey));
    assert_eq!(config.selected_row(), 1, "the cursor follows the number that was pressed");
    // A number past the end of the list does nothing rather than wrapping to something.
    assert_eq!(config.on_key(&key(KeyCode::Char('9'))), ConfigOutcome::Consumed);
}

#[test]
fn the_last_row_opens_the_file_list_and_then_one_file() {
    let mut config = state();
    assert_eq!(config.on_key(&key(KeyCode::Char('5'))), ConfigOutcome::Consumed);
    assert!(config.showing_files());

    config.on_key(&key(KeyCode::Down));
    assert_eq!(
        config.on_key(&key(KeyCode::Enter)),
        ConfigOutcome::EditFile(PathBuf::from("/home/dev/.config/luxe/models.json"))
    );
}

/// One step back at a time: out of the file list to the menu, out of the menu to the caller.
#[test]
fn escape_steps_back_rather_than_out() {
    let mut config = state();
    config.on_key(&key(KeyCode::Char('5')));
    assert_eq!(config.on_key(&key(KeyCode::Esc)), ConfigOutcome::Consumed);
    assert!(!config.showing_files());
    assert_eq!(config.on_key(&key(KeyCode::Esc)), ConfigOutcome::Close);
}

#[test]
fn the_cursor_stops_at_both_ends_of_each_list() {
    let mut config = state();
    for _ in 0..6 {
        config.on_key(&key(KeyCode::Down));
    }
    assert_eq!(config.selected_row(), ConfigChoice::ROWS.len() - 1);
    for _ in 0..6 {
        config.on_key(&key(KeyCode::Up));
    }
    assert_eq!(config.selected_row(), 0);

    // The file list keeps its own cursor, and coming back lands on the row that opened it.
    config.on_key(&key(KeyCode::Char('5')));
    config.on_key(&key(KeyCode::Down));
    assert_eq!(config.selected_row(), 1);
    config.on_key(&key(KeyCode::Esc));
    assert_eq!(config.selected_row(), ConfigChoice::ROWS.len() - 1);
}

/// "Configured" and "signed in" are not the same claim: an environment variable is the user's
/// shell rather than something luxe stored, and the summary has to say which.
#[test]
fn the_summary_names_every_credential_and_where_it_came_from() {
    let providers = vec![
        provider(
            "openai",
            CredentialStatus::Stored(StoredCredentialInfo {
                kind: CredentialKind::ApiKey,
                expires_at_ms: None,
                account: None,
            }),
        ),
        provider("anthropic", CredentialStatus::Env("ANTHROPIC_API_KEY".into())),
        provider("groq", CredentialStatus::Missing),
    ];
    let config = ConfigurationState::open(&providers, files());
    assert!(config.is_configured());
    let summary = config.summary();
    assert!(summary.contains("OPENAI (API key)"), "{summary}");
    assert!(summary.contains("ANTHROPIC (ANTHROPIC_API_KEY)"), "{summary}");
    assert!(!summary.contains("GROQ"), "a provider with no credential is not signed in");

    let empty = ConfigurationState::open(&[provider("groq", CredentialStatus::Missing)], files());
    assert!(!empty.is_configured());
    assert_eq!(empty.summary(), "No provider is configured yet.");
}

/// Below its minimum the surface draws only "Resize to continue", so the menu and the setup flow
/// hold their keys there the way the settings pages do; Esc still leaves.
///
/// The break this catches: only the settings pages knew they were undrawn, so a key on the menu
/// or in setup opened a sign-in or answered a question nobody could see.
#[test]
fn an_undrawn_surface_holds_its_keys_on_every_stage() {
    use ratatui::{backend::TestBackend, Terminal};
    let styles = crate::theme::styles::ThemeStyles::from_resolved(
        &luxe_core::theme::ThemeRegistry::new().resolve("nord").unwrap(),
        "nord",
    );
    let hit = std::cell::RefCell::new(crate::mouse::HitRegions::default());
    let draw = |config: &ConfigurationState, (width, height): (u16, u16)| {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let clock = crate::widgets::BannerClock::default();
        terminal.draw(|f| config.view(f, f.area(), &styles, &hit, clock)).unwrap();
    };

    let mut config = state();
    draw(&config, (30, 8));
    assert_eq!(config.on_key(&key(KeyCode::Down)), ConfigOutcome::Consumed);
    assert_eq!(config.selected_row(), 0, "the cursor did not move under the notice");
    assert_eq!(config.on_key(&key(KeyCode::Enter)), ConfigOutcome::Consumed, "nothing opened");
    assert_eq!(config.on_key(&key(KeyCode::Esc)), ConfigOutcome::Close, "esc still leaves");
    let mut config = state();
    draw(&config, (80, 24));
    config.on_key(&key(KeyCode::Down));
    assert_eq!(config.selected_row(), 1, "drawn, the menu moves");

    let mut config = state();
    let setup = crate::configuration::SetupState::open(
        crate::configuration::SetupMode::FirstRun,
        &luxe_core::config::Config::default(),
        vec!["nord".into(), "gruvbox".into()],
        None,
        "nord".into(),
    );
    config.show_setup(setup);
    let before = format!("{:?}", config.setup());
    draw(&config, (30, 8));
    assert_eq!(config.on_key(&key(KeyCode::Down)), ConfigOutcome::Consumed);
    assert_eq!(config.on_key(&key(KeyCode::Enter)), ConfigOutcome::Consumed);
    assert_eq!(format!("{:?}", config.setup()), before, "setup took nothing undrawn");
}

/// The app draws the resize notice itself, without the surface's own view, so the hold has to
/// come from that path too: through the real render and the real key route, nothing moves or
/// opens on the menu or in setup.
#[test]
fn the_app_holds_the_surfaces_keys_under_its_resize_notice() {
    use crate::fixture::{seeded_app, FixtureScenario};
    use ratatui::{backend::TestBackend, Terminal};
    for scenario in [FixtureScenario::Configuration, FixtureScenario::Setup] {
        let mut app = seeded_app("luxe", scenario).expect("the fixture");
        app.update(crate::app::Msg::TerminalResized { width: 30, height: 8 });
        let mut terminal = Terminal::new(TestBackend::new(30, 8)).unwrap();
        terminal.draw(|frame| crate::view::render(frame, &mut app)).unwrap();
        assert!(app.overlays.configuration.is_some(), "{scenario:?} opens the surface");
        let before = format!("{:?}", app.overlays.configuration);
        for code in [KeyCode::Down, KeyCode::Enter] {
            app.update(crate::app::Msg::Key(key(code)));
        }
        assert_eq!(format!("{:?}", app.overlays.configuration), before, "{scenario:?}");
        assert!(app.overlays.auth_wizard.is_none(), "{scenario:?} opened nothing");
    }
}
