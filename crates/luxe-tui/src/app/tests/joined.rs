//! Joined clients: messages sent here and from a client joined to the same session.

use super::*;
use luxe_core::message::Message;
use luxe_core::session::EntryId;

/// Who's joined, as the shared session reports it, is what the status holds, replaced whole each
/// time, and it adds nothing to the transcript.
///
/// The break this catches: the dock's list never updated, kept growing as phones reconnect, or a
/// join written into the transcript after all.
#[test]
fn whos_joined_is_held_for_the_dock_and_not_written_down() {
    let (mut app, _rx) = app_in_session();
    let before = app.convo.len();
    app.update(Msg::Engine(Event::GuestsChanged(vec!["iPhone".into()])));
    app.update(Msg::Engine(Event::GuestsChanged(vec!["iPad".into()])));
    assert_eq!(app.status.guests, ["iPad"]);
    assert_eq!(app.convo.len(), before);
}

fn app_in_session() -> (App, mpsc::Receiver<Action>) {
    let (tx, rx) = mpsc::channel(8);
    let config = luxe_core::config::Config::default();
    let mut app = App::new(tx, luxe_core::keymap::KeyMap::defaults(), config);
    app.status.model = "gpt-5.6".into();
    app.update(Msg::Engine(Event::SessionLoaded { session_id: "s".into(), messages: Vec::new() }));
    (app, rx)
}

fn send(app: &mut App, text: &str) {
    app.editor.insert_text(text);
    app.submit_prompt();
}

fn committed(app: &mut App, entry: &str, text: &str) {
    let message = Message::user(text);
    let entry_id = EntryId(entry.into());
    app.update(Msg::Engine(Event::MessageCommitted { session_id: "s".into(), entry_id, message }));
}

fn user_messages(app: &App) -> Vec<(&ConversationItemId, &str)> {
    app.convo.iter().filter_map(user_message).collect()
}

fn user_message(item: &ConversationItem) -> Option<(&ConversationItemId, &str)> {
    match item.block() {
        ConvoBlock::User { text, .. } => Some((&item.id, text.as_str())),
        _ => None,
    }
}

/// A message sent here is on screen once, and takes the id of the entry the engine saved.
///
/// The break this catches: landing every committed user message, so each one you type shows
/// twice; or leaving the copy's id provisional, so copy and expand can't find it in the session.
#[test]
fn a_message_sent_here_is_adopted_not_repeated() {
    let (mut app, _engine) = app_in_session();
    send(&mut app, "explain the stall");
    committed(&mut app, "entry-1", "explain the stall");
    let saved = ConversationItemId::Persisted {
        session_id: "s".into(),
        entry_id: "entry-1".into(),
        block_index: 0,
    };
    assert_eq!(user_messages(&app), [(&saved, "explain the stall")]);
}

/// A message a joined client sent is shown, after one sent here has already been adopted.
///
/// The break this catches: adopting whatever user message commits next, so the phone's message
/// replaces the id of the last one typed here and never appears.
#[test]
fn a_joined_clients_message_is_shown() {
    let (mut app, _engine) = app_in_session();
    send(&mut app, "mine");
    committed(&mut app, "entry-1", "mine");
    committed(&mut app, "entry-2", "from the phone");
    let texts: Vec<&str> = user_messages(&app).into_iter().map(|(_, text)| text).collect();
    assert_eq!(texts, ["mine", "from the phone"]);
}

/// A turn a joined client started shows as running from its start, not its first token.
///
/// The break this catches: ignoring `turn_started`, so the bar reads idle (and Enter starts a turn
/// instead of steering) while the phone's turn waits on the model.
#[test]
fn a_turn_another_client_started_shows_as_running() {
    let (mut app, _engine) = app_in_session();
    app.update(Msg::Engine(Event::TurnStarted));
    assert!(app.turn_in_flight());
    assert!(app.status.turn_started.is_some(), "the turn clock runs");
}

/// The break this catches: `turn_started` restarting the clock of a turn sent here, which began
/// counting when Enter was pressed.
#[test]
fn the_start_of_a_turn_sent_here_keeps_its_clock() {
    let (mut app, _engine) = app_in_session();
    send(&mut app, "explain the stall");
    let pressed = app.status.turn_started;
    std::thread::sleep(std::time::Duration::from_millis(2));
    app.update(Msg::Engine(Event::TurnStarted));
    assert_eq!(app.status.turn_started, pressed);
}

/// A prompt a hook refused never commits, so the turn's end lets go of it.
///
/// The break this catches: still waiting on the refused prompt's commit, so the next message a
/// joined client sends is taken for it and never shows.
#[test]
fn a_refused_prompt_does_not_claim_the_next_message() {
    let (mut app, _engine) = app_in_session();
    send(&mut app, "refused");
    let reason = luxe_core::StopReason::EndTurn;
    app.update(Msg::Engine(Event::TurnEnded { reason }));
    committed(&mut app, "entry-2", "from the phone");
    let texts: Vec<&str> = user_messages(&app).into_iter().map(|(_, text)| text).collect();
    assert_eq!(texts, ["refused", "from the phone"]);
}
