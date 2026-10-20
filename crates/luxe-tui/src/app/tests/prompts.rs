//! Prompts — a modal answered on another front-end comes down here too.

use super::notify::{permission_asked, question_asked};
use super::*;

fn closed(id: &str) -> Msg {
    Msg::Engine(Event::PromptClosed { id: id.into() })
}

fn quiet_app() -> App {
    let (tx, _rx) = mpsc::channel(8);
    App::new(tx, KeyMap::defaults(), Config::default())
}

/// A permission prompt answered on another front-end comes down, and only that one.
///
/// The break this catches: ignoring `PromptClosed`, which leaves the terminal holding a prompt a
/// phone already answered; or closing on any id, which takes down the next prompt because of a
/// late close of the last one.
#[tokio::test]
async fn a_permission_answered_elsewhere_comes_down_here() {
    let mut app = quiet_app();
    app.update(permission_asked("bash", "Run: ls"));
    app.update(closed("perm:another"));
    assert!(app.overlays.permission.is_some(), "another prompt's close leaves this one");
    app.update(closed("perm:1"));
    assert!(app.overlays.permission.is_none());
}

/// The same for a question, which has its own modal.
#[tokio::test]
async fn a_question_answered_elsewhere_comes_down_here() {
    let mut app = quiet_app();
    app.update(question_asked("Which database?"));
    let id = app.overlays.question.as_ref().expect("asked").id.as_str().to_string();
    app.update(closed("ask:another"));
    assert!(app.overlays.question.is_some(), "another question's close leaves this one");
    app.update(closed(&id));
    assert!(app.overlays.question.is_none());
}
