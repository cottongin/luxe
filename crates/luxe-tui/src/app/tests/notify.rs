//! Notifications — when a turn, a prompt or a question reaches outside the terminal.

use super::*;

/// A finished or failed turn notifies under the project's name, but not while the reader is
/// looking at the terminal; one that has never reported focus counts as in the background.
#[tokio::test]
async fn a_turn_end_notifies_only_while_the_terminal_is_in_the_background() {
    let mut app = notifying_app(luxe_core::config::Notifications::All(true));
    let notifier = crate::notify::MockNotifier::new();
    let calls = notifier.calls.clone();
    app.notifier = Box::new(notifier);
    app.project_root = Some(std::path::PathBuf::from("/home/dev/widget"));
    let end = || Msg::Engine(Event::TurnEnded { reason: luxe_core::event::StopReason::EndTurn });

    app.update(end());
    app.terminal_focused = Some(true);
    app.update(end());
    app.terminal_focused = Some(false);
    app.update(Msg::Engine(Event::Error("rate limited".into())));

    assert_eq!(
        *calls.lock().unwrap(),
        vec![
            ("Luxe · widget".to_string(), "Turn finished".to_string()),
            ("Luxe · widget".to_string(), "Turn failed: rate limited".to_string()),
        ],
        "the focused turn end sent nothing"
    );
}

/// An app whose notifications follow `setting`, with a recording notifier under a project name.
pub(super) fn notifying_app(setting: luxe_core::config::Notifications) -> App {
    let (tx, _rx) = mpsc::channel(8);
    let config = luxe_core::config::Config {
        notifications: Some(setting),
        ..luxe_core::config::Config::default()
    };
    let mut app = App::new(tx, luxe_core::keymap::KeyMap::defaults(), config);
    app.project_root = Some(std::path::PathBuf::from("/home/dev/widget"));
    app
}

pub(super) fn permission_asked(tool: &str, summary: &str) -> Msg {
    Msg::Engine(Event::PermissionRequested {
        id: serde_json::from_value(serde_json::json!("perm:1")).expect("an id"),
        session_id: "session-a".into(),
        tool: tool.into(),
        permission: tool.into(),
        summary: summary.into(),
        always_patterns: Vec::new(),
    })
}

pub(super) fn question_asked(prompt: &str) -> Msg {
    Msg::Engine(Event::QuestionAsked {
        id: luxe_core::question::QuestionRequestId::default(),
        session_id: "session-a".into(),
        prompt: prompt.into(),
        options: Vec::new(),
        multi: false,
    })
}

/// A turn that stops to wait on the reader is the moment most worth hearing about: a permission
/// prompt and a question notify the way a finished turn does, saying what is waiting.
#[tokio::test]
async fn a_prompt_waiting_on_the_reader_notifies() {
    let mut app = notifying_app(luxe_core::config::Notifications::All(true));
    let notifier = crate::notify::MockNotifier::new();
    let calls = notifier.calls.clone();
    app.notifier = Box::new(notifier);

    app.update(permission_asked("bash", "Run: rm -rf build"));
    app.overlays.permission = None;
    app.update(question_asked("Which database should the tests use?"));
    app.overlays.question = None;
    app.terminal_focused = Some(true);
    app.update(question_asked("Asked while you were looking"));

    let bodies: Vec<String> = calls.lock().unwrap().iter().map(|(_, body)| body.clone()).collect();
    assert_eq!(
        bodies,
        ["Approve bash: Run: rm -rf build", "Question: Which database should the tests use?"],
        "and nothing while the terminal has focus"
    );
}

/// A list names the moments worth a notification; the rest stay quiet.
#[tokio::test]
async fn a_notification_list_sends_only_the_moments_it_names() {
    use luxe_core::config::{NotificationEvent, Notifications};
    let mut app = notifying_app(Notifications::Only(vec![
        NotificationEvent::Permission,
        NotificationEvent::Question,
    ]));
    let notifier = crate::notify::MockNotifier::new();
    let calls = notifier.calls.clone();
    app.notifier = Box::new(notifier);

    app.update(Msg::Engine(Event::TurnEnded { reason: luxe_core::event::StopReason::EndTurn }));
    app.update(permission_asked("edit", ""));

    let bodies: Vec<String> = calls.lock().unwrap().iter().map(|(_, body)| body.clone()).collect();
    assert_eq!(bodies, ["Approve edit?"], "the finished turn was not on the list");
}

/// Headless has nobody to tell, whatever the config says.
#[tokio::test]
async fn headless_never_notifies() {
    let mut app = notifying_app(luxe_core::config::Notifications::All(true));
    let notifier = crate::notify::MockNotifier::new();
    let calls = notifier.calls.clone();
    app.notifier = Box::new(notifier);
    app.headless = true;
    app.update(permission_asked("bash", "ls"));
    app.update(Msg::Engine(Event::TurnEnded { reason: luxe_core::event::StopReason::EndTurn }));
    assert!(calls.lock().unwrap().is_empty());
}

/// What herdr is told: a waiting prompt outranks the turn it interrupted, and the turn ending is
/// what makes the agent idle again.
#[tokio::test]
async fn the_agent_state_follows_turns_and_prompts() {
    use crate::herdr::AgentState;
    let (tx, _rx) = mpsc::channel(8);
    let mut app =
        App::new(tx, luxe_core::keymap::KeyMap::defaults(), luxe_core::config::Config::default());
    assert_eq!(app.agent_state(), AgentState::Idle);
    app.update(Msg::Engine(Event::TokenDelta { delta: "Looking".into(), block: 0 }));
    assert_eq!(app.agent_state(), AgentState::Working);
    app.update(permission_asked("bash", "Run: cargo test"));
    assert_eq!(app.agent_state(), AgentState::Blocked("Approve bash".into()));
    app.overlays.permission = None;
    app.update(question_asked("Which one?"));
    assert_eq!(app.agent_state(), AgentState::Blocked("Question: Which one?".into()));
    app.update(Msg::Engine(Event::TurnEnded { reason: luxe_core::event::StopReason::EndTurn }));
    assert_eq!(app.agent_state(), AgentState::Idle, "a prompt cannot outlive its turn");
}
