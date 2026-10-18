#![allow(clippy::unwrap_used)]

use super::tests_support::*;
use super::*;
use crate::mouse::{HitRegions, MouseTarget};
use ratatui::{backend::TestBackend, buffer::Buffer, Terminal};
use std::cell::RefCell;

fn theme() -> crate::theme::styles::ThemeStyles {
    let resolved = luxe_core::theme::ThemeRegistry::new().resolve("dark").expect("dark resolves");
    crate::theme::styles::ThemeStyles::from_resolved(&resolved, "dark")
}

/// The wizard drawn as `/login` draws it, with what each cell would be hit as.
fn draw(wizard: &AuthWizardState) -> (Buffer, RefCell<HitRegions>) {
    let hit = RefCell::new(HitRegions::default());
    let mut terminal = Terminal::new(TestBackend::new(90, 32)).unwrap();
    terminal.draw(|frame| wizard.view(frame, frame.area(), &theme(), &hit)).unwrap();
    (terminal.backend().buffer().clone(), hit)
}

/// Where `needle` is drawn, as `(column, row)`.
fn cell_of(buffer: &Buffer, needle: &str) -> (u16, u16) {
    for y in 0..buffer.area.height {
        let row: String = (0..buffer.area.width).map(|x| buffer[(x, y)].symbol()).collect();
        if let Some(byte) = row.find(needle) {
            return (row[..byte].chars().count() as u16, y);
        }
    }
    panic!("{needle} is not drawn");
}

/// A running sign-in asking which GitHub, as Copilot's does.
fn asking_which_github(
) -> (AuthWizardState, tokio::sync::oneshot::Receiver<Result<SecretString, AuthError>>) {
    let mut wizard = AuthWizardState::open(providers(), LoginScope::Any);
    wizard.on_key(&key(KeyCode::Enter));
    let (reply, answer) = tokio::sync::oneshot::channel();
    let choice = |id: &str, label: &str, detail: &str| AuthChoice {
        id: id.into(),
        label: label.into(),
        detail: Some(detail.into()),
    };
    let choices = vec![
        choice("github", "GitHub", "github.com"),
        choice("enterprise", "GitHub Enterprise", "your company's server"),
    ];
    wizard.apply(AuthUiMsg::Prompt { prompt: AuthPrompt::select("Which GitHub?", choices), reply });
    (wizard, answer)
}

/// Each choice of a question the sign-in asks is clickable where it is drawn, its detail line
/// included, as the provider and method lists are. It used to draw without registering anywhere,
/// so a click on "GitHub" did nothing.
#[test]
fn a_question_s_choices_are_clickable_where_they_are_drawn() {
    let (wizard, _answer) = asking_which_github();
    let (buffer, hit) = draw(&wizard);
    let (x, y) = cell_of(&buffer, "GitHub Enterprise");
    assert_eq!(hit.borrow().at(x, y), Some(&MouseTarget::AuthMethodRow(1)));
    let (x, y) = cell_of(&buffer, "your company's server");
    assert_eq!(hit.borrow().at(x, y), Some(&MouseTarget::AuthMethodRow(1)), "the detail line too");
    let (x, y) = cell_of(&buffer, "github.com");
    assert_eq!(hit.borrow().at(x, y), Some(&MouseTarget::AuthMethodRow(0)));
}

/// A click lands as the keyboard would: the row it hit becomes the answer.
#[tokio::test]
async fn a_clicked_choice_is_the_answer() {
    let (mut wizard, answer) = asking_which_github();
    wizard.select_method(1);
    wizard.on_key(&key(KeyCode::Enter));
    let chosen = answer.await.unwrap().unwrap();
    assert_eq!(secrecy::ExposeSecret::expose_secret(&chosen), "enterprise");
}

/// A sign-in waiting on a device code offers it to copy, by click or by `c`, and says so.
#[test]
fn a_waiting_device_code_can_be_copied() {
    let mut wizard = AuthWizardState::open(providers(), LoginScope::Any);
    wizard.on_key(&key(KeyCode::Enter));
    wizard.apply(AuthUiMsg::Event(AuthEvent::DeviceCode {
        user_code: "WXYZ-9876".into(),
        verification_uri: "https://github.com/login/device".into(),
        expires_in_secs: 900,
        interval_secs: 5,
    }));
    let (buffer, hit) = draw(&wizard);
    let (x, y) = cell_of(&buffer, "Copy code WXYZ-9876");
    assert_eq!(hit.borrow().at(x, y), Some(&MouseTarget::AuthCopyCode));
    cell_of(&buffer, "c copy code");
    match wizard.on_key(&key(KeyCode::Char('c'))) {
        WizardOutcome::CopyCode(code) => assert_eq!(code, "WXYZ-9876"),
        other => panic!("expected the code to copy, got {other:?}"),
    }
}

/// While the flow is asking something, `c` is a letter of the answer, not a copy.
#[test]
fn c_types_into_a_question_rather_than_copying() {
    let mut wizard = AuthWizardState::open(providers(), LoginScope::Any);
    wizard.on_key(&key(KeyCode::Enter));
    wizard.apply(AuthUiMsg::Event(AuthEvent::DeviceCode {
        user_code: "WXYZ-9876".into(),
        verification_uri: "https://github.com/login/device".into(),
        expires_in_secs: 900,
        interval_secs: 5,
    }));
    let (reply, _answer) = tokio::sync::oneshot::channel();
    wizard.apply(AuthUiMsg::Prompt { prompt: AuthPrompt::text("Domain:", None), reply });
    assert!(matches!(wizard.on_key(&key(KeyCode::Char('c'))), WizardOutcome::Consumed));
    assert_eq!(wizard.pending_prompt().map(|(_, typed)| typed), Some("c"));
    assert_eq!(wizard.device_code(), None, "no button while a question is up");
}
