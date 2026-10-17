#![allow(clippy::unwrap_used)]

use super::*;
use crate::theme::styles::ThemeStyles;
use luxe_core::auth::{AuthMethod, AuthPrompt, ProviderOption};
use ratatui::{backend::TestBackend, Terminal};

fn providers() -> Vec<ProviderOption> {
    vec![
        ProviderOption {
            id: "openai".into(),
            name: "OpenAI".into(),
            methods: vec![AuthMethod::api_key(Some("sk-… — or set OPENAI_API_KEY".into()))],
            status: CredentialStatus::Missing,
            note: None,
            console_url: None,
            keyless: false,
            needs_endpoint: false,
        },
        ProviderOption {
            id: "openrouter".into(),
            name: "OpenRouter".into(),
            methods: vec![
                AuthMethod::subscription("Sign in with OpenRouter", Some("Mints a key".into())),
                AuthMethod::api_key(None),
            ],
            status: CredentialStatus::Env("OPENROUTER_API_KEY".into()),
            note: Some("A note worth reading".into()),
            console_url: None,
            keyless: false,
            needs_endpoint: false,
        },
    ]
}

fn theme() -> ThemeStyles {
    let registry = luxe_core::theme::ThemeRegistry::new();
    let resolved = registry.resolve("nord").expect("nord resolves");
    ThemeStyles::from_resolved(&resolved, "nord")
}

fn render(state: &AuthWizardState) -> String {
    let mut terminal = Terminal::new(TestBackend::new(90, 26)).unwrap();
    let theme = theme();
    let hit = std::cell::RefCell::new(HitRegions::default());
    terminal
        .draw(|frame| {
            let area = frame.area();
            state.view(frame, area, &theme, &hit);
        })
        .unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .chunks(90)
        .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_provider_list_shows_names_status_and_the_subscription_hint() {
    let state = AuthWizardState::open(providers(), LoginScope::Any);
    let screen = render(&state);
    assert!(screen.contains("Sign in to a provider"), "{screen}");
    assert!(screen.contains("OpenAI"));
    assert!(screen.contains("OpenRouter"));
    assert!(screen.contains("env"), "an ambient key is worth surfacing: {screen}");
    assert!(screen.contains("subscription"));
    assert!(screen.contains("enter open"));
}

/// A scoped list is one method's worth of the catalog, and the panel has to say which — the
/// reader chose the method on the surface before this one and never sees it again otherwise.
#[test]
fn a_scope_names_itself_and_says_what_it_is_offering() {
    let subscription = AuthWizardState::open(providers(), LoginScope::Subscription);
    let screen = render(&subscription);
    assert!(screen.contains("Sign in to a subscription"), "{screen}");
    assert!(screen.contains("already pay for"), "{screen}");

    let keys = AuthWizardState::open(providers(), LoginScope::ApiKey);
    assert!(render(&keys).contains("Add an API key"));

    let local = AuthWizardState::open(providers(), LoginScope::LocalOrCustom);
    let screen = render(&local);
    assert!(screen.contains("Local or custom model"), "{screen}");
    assert!(screen.contains("needs no key"), "{screen}");
}

#[test]
fn the_method_list_shows_the_note_and_both_options() {
    let mut state = AuthWizardState::open(providers(), LoginScope::Any);
    state.on_key(&crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Down,
        crossterm::event::KeyModifiers::NONE,
    ));
    state.on_key(&crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Enter,
        crossterm::event::KeyModifiers::NONE,
    ));
    let screen = render(&state);
    assert!(screen.contains("A note worth reading"), "{screen}");
    assert!(screen.contains("Sign in with OpenRouter"));
    assert!(screen.contains("Use an API key"));
    assert!(screen.contains("esc back"));
}

/// The cell each of a row's two chamfers sits on, for whichever row carries `needle`.
fn edges(screen: &str, needle: &str) -> (usize, usize) {
    let (open, close) = (crate::widgets::Cap::Open.glyph(), crate::widgets::Cap::Close.glyph());
    let row = screen.lines().find(|row| row.contains(needle)).expect("the row is on screen");
    let at = |cut: Option<usize>| UnicodeWidthStr::width(&row[..cut.expect("a cut")]);
    (at(row.find(open)), at(row.rfind(close)))
}

/// The cell `needle` starts on, wherever it is.
fn column(screen: &str, needle: &str) -> usize {
    let row = screen.lines().find(|row| row.contains(needle)).expect("the row is on screen");
    UnicodeWidthStr::width(&row[..row.find(needle).expect("found above")])
}

/// The wizard is where the Configuration surface sends you, so a chosen row has to mean the
/// same thing in both: the slab, not a tint that stops where the words do.
#[test]
fn a_chosen_provider_is_cut_into_a_slab() {
    crate::glyphs::set_tier(crate::glyphs::GlyphTier::Nerd);
    let state = AuthWizardState::open(providers(), LoginScope::Any);
    let screen = render(&state);

    let (open, close) = edges(&screen, "OpenAI");
    assert!(open + 2 < close, "the chosen row is cut at both ends: {screen}");
    assert_eq!(
        column(&screen, "OpenAI"),
        column(&screen, "OpenRouter"),
        "and the row below it keeps the same column"
    );
}

/// A method and the line explaining it are one choice, so they are cut as one slab — the same
/// shape the Configuration surface uses for the entries that open this wizard.
#[test]
fn a_chosen_method_is_cut_as_one_slab() {
    crate::glyphs::set_tier(crate::glyphs::GlyphTier::Nerd);
    let mut state = AuthWizardState::open(providers(), LoginScope::Any);
    for code in [crossterm::event::KeyCode::Down, crossterm::event::KeyCode::Enter] {
        state.on_key(&crossterm::event::KeyEvent::new(code, crossterm::event::KeyModifiers::NONE));
    }
    let screen = render(&state);

    let (label_open, label_close) = edges(&screen, "Sign in with OpenRouter");
    let (detail_open, detail_close) = edges(&screen, "Mints a key");
    assert_eq!(label_open, detail_open, "one shape, so one leading column");
    assert_eq!(label_close, detail_close, "and one trailing column");
    // The explanation sits under the name it explains, not under the number.
    assert_eq!(column(&screen, "Mints a key"), column(&screen, "Sign in with OpenRouter"));
}

/// A question the flow asks mid-login is a list of choices like any other, and the row the
/// cursor is on is cut the same way.
#[test]
fn a_chosen_prompt_answer_is_cut_into_a_slab() {
    crate::glyphs::set_tier(crate::glyphs::GlyphTier::Nerd);
    let mut state = AuthWizardState::open(providers(), LoginScope::Any);
    state.on_key(&crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Enter,
        crossterm::event::KeyModifiers::NONE,
    ));
    let (reply, _rx) = tokio::sync::oneshot::channel();
    state.apply(super::super::AuthUiMsg::Prompt {
        prompt: AuthPrompt::select(
            "Which API does it speak?",
            vec![
                luxe_core::auth::AuthChoice {
                    id: "completions".into(),
                    label: "OpenAI /chat/completions".into(),
                    detail: Some("vLLM, llama.cpp, Ollama, most proxies".into()),
                },
                luxe_core::auth::AuthChoice {
                    id: "responses".into(),
                    label: "OpenAI /responses".into(),
                    detail: Some("Azure OpenAI, newer gateways".into()),
                },
            ],
        ),
        reply,
    });
    let screen = render(&state);

    // The detail is what the flow wrote to be read at the moment of choosing. The headless
    // prompt has always printed it; the wizard used to drop it on the floor.
    assert!(screen.contains("vLLM, llama.cpp"), "the choice explains itself: {screen}");
    let (label_open, label_close) = edges(&screen, "OpenAI /chat/completions");
    let (detail_open, detail_close) = edges(&screen, "vLLM, llama.cpp");
    assert_eq!(label_open, detail_open, "one slab, not two rows");
    assert_eq!(label_close, detail_close);
    assert_eq!(
        column(&screen, "OpenAI /chat/completions"),
        column(&screen, "OpenAI /responses"),
        "and the answer below it keeps the same column"
    );
}

#[test]
fn a_secret_prompt_is_echoed_as_dots_and_never_as_the_key() {
    let mut state = AuthWizardState::open(providers(), LoginScope::Any);
    state.on_key(&crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Enter,
        crossterm::event::KeyModifiers::NONE,
    ));
    let (reply, _rx) = tokio::sync::oneshot::channel();
    state.apply(super::super::AuthUiMsg::Prompt {
        prompt: AuthPrompt::secret("OpenAI API key:", Some("sk-…".into())),
        reply,
    });
    for ch in "sk-secret".chars() {
        state.on_key(&crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char(ch),
            crossterm::event::KeyModifiers::NONE,
        ));
    }
    let screen = render(&state);
    assert!(screen.contains("OpenAI API key:"), "{screen}");
    assert!(!screen.contains("sk-secret"), "the key must never be drawn: {screen}");
    assert!(screen.contains("•••"), "{screen}");
}

#[test]
fn a_device_code_is_rendered_as_numbered_instructions() {
    let mut state = AuthWizardState::open(providers(), LoginScope::Any);
    state.on_key(&crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Enter,
        crossterm::event::KeyModifiers::NONE,
    ));
    state.apply(super::super::AuthUiMsg::Event(luxe_core::auth::AuthEvent::DeviceCode {
        user_code: "WXYZ-9876".into(),
        verification_uri: "https://github.com/login/device".into(),
        interval_secs: 5,
        expires_in_secs: 900,
    }));
    let screen = render(&state);
    assert!(screen.contains("WXYZ-9876"), "{screen}");
    assert!(screen.contains("github.com/login/device"), "{screen}");
    assert!(screen.contains("15 min"), "{screen}");
}

#[test]
fn a_failure_shows_the_reason_and_how_to_retry() {
    let mut state = AuthWizardState::open(providers(), LoginScope::Any);
    state.on_key(&crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Enter,
        crossterm::event::KeyModifiers::NONE,
    ));
    state.apply(super::super::AuthUiMsg::Finished(Err(luxe_core::auth::AuthError::Rejected(
        "OpenAI rejected that credential (401)".into(),
    ))));
    let screen = render(&state);
    assert!(screen.contains("401"), "{screen}");
    assert!(screen.contains("try again"), "{screen}");
}

/// A terminal too small for a dialog must draw nothing rather than a broken box.
#[test]
fn a_tiny_terminal_draws_nothing() {
    let state = AuthWizardState::open(providers(), LoginScope::Any);
    let mut terminal = Terminal::new(TestBackend::new(20, 6)).unwrap();
    let theme = theme();
    let hit = std::cell::RefCell::new(HitRegions::default());
    terminal
        .draw(|frame| {
            let area = frame.area();
            state.view(frame, area, &theme, &hit);
        })
        .unwrap();
    let blank =
        terminal.backend().buffer().content().iter().all(|cell| cell.symbol().trim().is_empty());
    assert!(blank, "nothing should be drawn at 20x6");
}
