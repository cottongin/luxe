#![allow(clippy::unwrap_used)]

use super::tests_support::*;
use super::*;

#[test]
fn a_single_method_provider_starts_immediately() {
    let mut wizard = AuthWizardState::open(providers(), LoginScope::Any);
    assert!(matches!(wizard.stage(), Stage::Provider { .. }));
    let outcome = wizard.on_key(&key(KeyCode::Enter));
    match outcome {
        WizardOutcome::Start { provider, method } => {
            assert_eq!((provider.as_str(), method.as_str()), ("openai", "api-key"));
        }
        other => panic!("expected a start, got {other:?}"),
    }
    assert!(matches!(wizard.stage(), Stage::Running { .. }), "must move to the running stage");
}

#[test]
fn a_provider_with_two_methods_asks_which_one() {
    let mut wizard = AuthWizardState::open(providers(), LoginScope::Any);
    wizard.on_key(&key(KeyCode::Down));
    assert!(matches!(wizard.on_key(&key(KeyCode::Enter)), WizardOutcome::Consumed));
    assert_eq!(wizard.method_provider().map(|o| o.id.as_str()), Some("openrouter"));

    // The subscription option leads, so Enter takes it.
    match wizard.on_key(&key(KeyCode::Enter)) {
        WizardOutcome::Start { provider, method } => {
            assert_eq!((provider.as_str(), method.as_str()), ("openrouter", "oauth"));
        }
        other => panic!("expected a start, got {other:?}"),
    }
}

#[test]
fn escape_from_the_method_list_goes_back_rather_than_out() {
    let mut wizard = AuthWizardState::open(providers(), LoginScope::Any);
    wizard.on_key(&key(KeyCode::Down));
    wizard.on_key(&key(KeyCode::Enter));
    assert!(matches!(wizard.on_key(&key(KeyCode::Esc)), WizardOutcome::Consumed));
    assert!(matches!(wizard.stage(), Stage::Provider { .. }));
    // A second Esc leaves.
    assert!(matches!(wizard.on_key(&key(KeyCode::Esc)), WizardOutcome::Close));
}

#[test]
fn typing_filters_the_provider_list() {
    let mut wizard = AuthWizardState::open(providers(), LoginScope::Any);
    for ch in "anth".chars() {
        wizard.on_key(&key(KeyCode::Char(ch)));
    }
    let visible: Vec<&str> =
        wizard.visible_providers().iter().map(|(_, option)| option.id.as_str()).collect();
    assert_eq!(visible, vec!["anthropic"]);
    assert_eq!(wizard.query(), "anth");

    // Selecting the filtered row starts *that* provider, not the one at the same index.
    match wizard.on_key(&key(KeyCode::Enter)) {
        WizardOutcome::Start { provider, .. } => assert_eq!(provider, "anthropic"),
        other => panic!("expected anthropic, got {other:?}"),
    }
}

#[test]
fn a_filter_that_matches_nothing_cannot_start_a_login() {
    let mut wizard = AuthWizardState::open(providers(), LoginScope::Any);
    for ch in "zzzz".chars() {
        wizard.on_key(&key(KeyCode::Char(ch)));
    }
    assert!(wizard.visible_providers().is_empty());
    assert!(matches!(wizard.on_key(&key(KeyCode::Enter)), WizardOutcome::Consumed));
}

#[test]
fn opening_for_a_named_provider_skips_the_list() {
    let wizard = AuthWizardState::open_for(providers(), "openrouter", LoginScope::Any);
    assert_eq!(wizard.method_provider().map(|o| o.id.as_str()), Some("openrouter"));
    // An unknown id falls back to the list rather than erroring.
    let fallback = AuthWizardState::open_for(providers(), "nope", LoginScope::Any);
    assert!(matches!(fallback.stage(), Stage::Provider { .. }));
}

#[tokio::test]
async fn a_secret_prompt_collects_an_answer_and_replies_to_the_flow() {
    let mut wizard = AuthWizardState::open(providers(), LoginScope::Any);
    wizard.on_key(&key(KeyCode::Enter));

    let (reply, answer) = tokio::sync::oneshot::channel();
    wizard.apply(AuthUiMsg::Prompt { prompt: AuthPrompt::secret("Key:", None), reply });
    assert!(wizard.is_awaiting_answer());

    for ch in "sk-1".chars() {
        wizard.on_key(&key(KeyCode::Char(ch)));
    }
    wizard.on_key(&key(KeyCode::Backspace));
    assert!(matches!(wizard.on_key(&key(KeyCode::Enter)), WizardOutcome::Answer));
    let received = answer.await.expect("channel open").expect("an answer");
    assert_eq!(secrecy::ExposeSecret::expose_secret(&received), "sk-");
    assert!(!wizard.is_awaiting_answer(), "the prompt is consumed once answered");
}

#[tokio::test]
async fn an_empty_answer_is_refused_unless_the_prompt_allows_it() {
    let mut wizard = AuthWizardState::open(providers(), LoginScope::Any);
    wizard.on_key(&key(KeyCode::Enter));

    let (reply, mut answer) = tokio::sync::oneshot::channel();
    wizard.apply(AuthUiMsg::Prompt { prompt: AuthPrompt::secret("Key:", None), reply });
    assert!(matches!(wizard.on_key(&key(KeyCode::Enter)), WizardOutcome::Consumed));
    assert!(answer.try_recv().is_err(), "nothing may be sent");

    // A prompt that documents an empty answer accepts one (Copilot's enterprise domain).
    let (reply, answer) = tokio::sync::oneshot::channel();
    wizard.apply(AuthUiMsg::Prompt { prompt: AuthPrompt::optional_text("Domain:", None), reply });
    assert!(matches!(wizard.on_key(&key(KeyCode::Enter)), WizardOutcome::Answer));
    assert_eq!(
        secrecy::ExposeSecret::expose_secret(&answer.await.unwrap().unwrap()),
        "",
        "an allowed empty answer is delivered as empty"
    );
}

#[tokio::test]
async fn a_select_prompt_answers_with_the_choice_id() {
    let mut wizard = AuthWizardState::open(providers(), LoginScope::Any);
    wizard.on_key(&key(KeyCode::Enter));
    let (reply, answer) = tokio::sync::oneshot::channel();
    wizard.apply(AuthUiMsg::Prompt {
        prompt: AuthPrompt::select(
            "How?",
            vec![
                AuthChoice { id: "browser".into(), label: "Browser".into(), detail: None },
                AuthChoice { id: "device".into(), label: "Device".into(), detail: None },
            ],
        ),
        reply,
    });
    wizard.on_key(&key(KeyCode::Down));
    wizard.on_key(&key(KeyCode::Enter));
    assert_eq!(secrecy::ExposeSecret::expose_secret(&answer.await.unwrap().unwrap()), "device");
}

/// The race that makes SSH logins work: the loopback callback wins, the flow emits progress, and
/// the paste prompt has to disappear on its own.
#[tokio::test]
async fn a_progress_event_supersedes_a_pending_prompt() {
    let mut wizard = AuthWizardState::open(providers(), LoginScope::Any);
    wizard.on_key(&key(KeyCode::Enter));
    let (reply, _answer) = tokio::sync::oneshot::channel();
    wizard.apply(AuthUiMsg::Prompt { prompt: AuthPrompt::text("Paste the URL:", None), reply });
    assert!(wizard.is_awaiting_answer());

    wizard.apply(AuthUiMsg::Event(AuthEvent::Progress { message: "Exchanging…".into() }));
    assert!(!wizard.is_awaiting_answer(), "the paste prompt must clear itself");
    assert_eq!(wizard.lines().last(), Some(&WizardLine::Progress("Exchanging…".into())));
}

/// An informational event must *not* clear a prompt: a flow that says something while waiting
/// would otherwise eat the user's half-typed answer.
#[tokio::test]
async fn an_info_event_leaves_a_pending_prompt_alone() {
    let mut wizard = AuthWizardState::open(providers(), LoginScope::Any);
    wizard.on_key(&key(KeyCode::Enter));
    let (reply, _answer) = tokio::sync::oneshot::channel();
    wizard.apply(AuthUiMsg::Prompt { prompt: AuthPrompt::secret("Key:", None), reply });
    wizard.on_key(&key(KeyCode::Char('x')));
    wizard
        .apply(AuthUiMsg::Event(AuthEvent::Info { message: "Heads up".into(), links: Vec::new() }));
    assert!(wizard.is_awaiting_answer());
    assert_eq!(wizard.pending_prompt().map(|(_, input)| input), Some("x"));
}

#[tokio::test]
async fn closing_the_wizard_declines_the_flows_question() {
    let mut wizard = AuthWizardState::open(providers(), LoginScope::Any);
    wizard.on_key(&key(KeyCode::Enter));
    let (reply, answer) = tokio::sync::oneshot::channel();
    wizard.apply(AuthUiMsg::Prompt { prompt: AuthPrompt::secret("Key:", None), reply });
    wizard.cancel_pending();
    assert!(answer.await.unwrap().is_err(), "the flow must learn it was cancelled");
}

/// A credential that landed ends the wizard rather than parking on a screen about itself, and
/// the outcome has to reach the app intact — it is the only copy, and the picker's opening
/// line is written from it.
#[test]
fn a_finished_login_hands_the_outcome_back_and_ends_the_wizard() {
    let mut wizard = AuthWizardState::open(providers(), LoginScope::Any);
    wizard.on_key(&key(KeyCode::Enter));
    let outcome = wizard.apply(AuthUiMsg::Finished(Ok(LoginOutcome {
        provider_id: "openai".into(),
        provider_name: "OpenAI".into(),
        kind: Some(luxe_core::auth::CredentialKind::ApiKey),
        account: None,
        suggested_model: None,
        notes: vec!["12 models available".into()],
        advice: Vec::new(),
    })));
    match outcome {
        WizardOutcome::Finished(outcome) => {
            assert_eq!(outcome.provider_id, "openai");
            assert_eq!(outcome.notes.len(), 1);
        }
        other => panic!("expected a finished login, got {other:?}"),
    }
    assert!(
        matches!(wizard.stage(), Stage::Running { .. }),
        "the wizard does not invent a screen for its own success"
    );
}

#[test]
fn a_failed_login_can_be_retried_without_reopening() {
    let mut wizard = AuthWizardState::open(providers(), LoginScope::Any);
    wizard.on_key(&key(KeyCode::Enter));
    wizard.apply(AuthUiMsg::Finished(Err(AuthError::Rejected("401".into()))));
    assert!(matches!(wizard.stage(), Stage::Failed(message) if message.contains("401")));
    assert!(matches!(wizard.on_key(&key(KeyCode::Char('r'))), WizardOutcome::Consumed));
    assert!(matches!(wizard.stage(), Stage::Provider { .. }));
}

#[test]
fn a_cancelled_flow_says_nothing_was_saved() {
    let mut wizard = AuthWizardState::open(providers(), LoginScope::Any);
    wizard.on_key(&key(KeyCode::Enter));
    wizard.apply(AuthUiMsg::Finished(Err(AuthError::Cancelled)));
    assert!(
        matches!(wizard.stage(), Stage::Failed(message) if message.contains("Nothing was saved"))
    );
}

/// Paste is how most people enter a key, so it goes where typing would — and nowhere else.
#[tokio::test]
async fn a_paste_reaches_whatever_the_wizard_is_collecting() {
    // The provider list is a search box.
    let mut wizard = AuthWizardState::open(providers(), LoginScope::Any);
    assert!(wizard.paste("anthropic\n"));
    assert_eq!(wizard.query(), "anthropic");
    assert_eq!(wizard.visible_providers().len(), 1);
    assert!(!wizard.paste("   "), "a paste of nothing types nothing");
    assert_eq!(wizard.query(), "anthropic");

    // A key prompt takes it, without the newline the console copy brought along.
    let mut wizard = AuthWizardState::open(providers(), LoginScope::Any);
    wizard.on_key(&key(KeyCode::Enter));
    let (reply, _answer) = tokio::sync::oneshot::channel();
    wizard.apply(AuthUiMsg::Prompt { prompt: AuthPrompt::secret("Key:", None), reply });
    assert!(wizard.paste("sk-1\n"));
    assert_eq!(wizard.pending_prompt().map(|(_, input)| input), Some("sk-1"));

    // A question with a fixed set of answers has no field: pasting into it types nothing,
    // and the answer stays whatever the cursor is on.
    let mut wizard = AuthWizardState::open(providers(), LoginScope::Any);
    wizard.on_key(&key(KeyCode::Enter));
    let (reply, _answer) = tokio::sync::oneshot::channel();
    wizard.apply(AuthUiMsg::Prompt {
        prompt: AuthPrompt::select(
            "How?",
            vec![AuthChoice { id: "browser".into(), label: "Browser".into(), detail: None }],
        ),
        reply,
    });
    assert!(!wizard.paste("sk-1"));
    assert_eq!(wizard.pending_prompt().map(|(_, input)| input), Some(""));
}

/// A key typed or pasted into a prompt never shows up in the wizard's `Debug`, while the provider
/// and stage stay there to diagnose with.
#[tokio::test]
async fn wizard_debug_never_contains_typed_or_pasted_secret() {
    for paste in [false, true] {
        let mut wizard = AuthWizardState::open(providers(), LoginScope::Any);
        wizard.on_key(&key(KeyCode::Enter));
        let (reply, _answer) = tokio::sync::oneshot::channel();
        wizard.apply(AuthUiMsg::Prompt { prompt: AuthPrompt::secret("Key:", None), reply });
        let secret = "zq-typed-secret-7781";
        if paste {
            assert!(wizard.paste(secret));
        } else {
            for ch in secret.chars() {
                wizard.on_key(&key(KeyCode::Char(ch)));
            }
        }
        let debug = format!("{wizard:?}");
        assert!(!debug.contains(secret) && !debug.contains("7781"), "{debug}");
        assert!(debug.contains("Running") && debug.contains("Key:"), "{debug}");
    }
}

/// A prompt arriving after the stage moved on must be declined, not queued forever.
#[tokio::test]
async fn a_prompt_arriving_after_the_flow_ended_is_declined() {
    let mut wizard = AuthWizardState::open(providers(), LoginScope::Any);
    let (reply, answer) = tokio::sync::oneshot::channel();
    wizard.apply(AuthUiMsg::Prompt { prompt: AuthPrompt::secret("Key:", None), reply });
    assert!(answer.await.unwrap().is_err());
}

#[test]
fn device_codes_and_urls_become_their_own_transcript_lines() {
    let mut wizard = AuthWizardState::open(providers(), LoginScope::Any);
    wizard.on_key(&key(KeyCode::Enter));
    wizard.apply(AuthUiMsg::Event(AuthEvent::DeviceCode {
        user_code: "ABCD-1234".into(),
        verification_uri: "https://github.com/login/device".into(),
        interval_secs: 5,
        expires_in_secs: 900,
    }));
    wizard.apply(AuthUiMsg::Event(AuthEvent::OpenUrl {
        url: "https://auth.openai.com/x".into(),
        instructions: Some("Finish in the browser".into()),
    }));
    match &wizard.lines()[0] {
        WizardLine::DeviceCode { code, expires_in_minutes, .. } => {
            assert_eq!(code, "ABCD-1234");
            assert_eq!(*expires_in_minutes, 15);
        }
        other => panic!("expected a device code, got {other:?}"),
    }
    assert!(matches!(&wizard.lines()[1], WizardLine::Url { .. }));
}
