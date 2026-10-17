//! Steering — a message sent mid-turn, recalled, queued or never delivered.

use super::*;

/// A mid-turn submission steers by default: it's pinned above the composer and handed to the
/// running turn, without resetting turn state or entering the transcript before it lands.
#[test]
fn a_message_sent_mid_turn_steers_instead_of_starting_a_turn() {
    let (width, height) = (80, 24);
    let backend = test_backend(width, height);
    let (tx, mut rx) = tokio::sync::mpsc::channel::<luxe_core::engine::Action>(64);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    // A live session, so the committed steer below is accepted the way the runtime sends it.
    app.update(Msg::Engine(luxe_core::event::Event::SessionLoaded {
        session_id: "session-a".into(),
        messages: vec![],
    }));

    // A turn is running: the status clock is ticking.
    app.update(Msg::Engine(luxe_core::event::Event::TokenDelta {
        block: 0,
        delta: "working".into(),
    }));
    let started = app.status.turn_started.expect("the turn clock is running");

    app.editor.insert_text("actually, check the tests too");
    app.update(Msg::Key(key(KeyCode::Enter)));

    match rx.try_recv().expect("a steer action reaches the engine") {
        luxe_core::engine::Action::SteerTurn { content } => {
            assert_eq!(content, "actually, check the tests too");
        }
        other => panic!("expected SteerTurn, got {other:?}"),
    }
    assert_eq!(app.status.turn_started, Some(started), "steering must not restart the turn clock");

    // Held above the composer, not dropped into the transcript where the turn's own output would
    // scroll it away.
    assert_eq!(app.pending_submissions.len(), 1);
    assert!(
        !app.convo.iter().any(|item| matches!(item.block(), ConvoBlock::User { .. })),
        "a steer isn't a transcript message until it lands"
    );
    let buffer = render_app(&mut app, &backend);
    let content = buffer_to_string(&buffer);
    assert!(content.contains("steering"), "the tray says what's pending: {content}");
    assert!(content.contains("check the tests too"), "and shows the message: {content}");

    // Delivery is announced twice by design: the engine commits the steer as a real user entry
    // (`MessageCommitted`, which carries the persisted id) and then reports the delivery. Exactly
    // one transcript row may come out of that pair — landing both is what showed every steer twice.
    app.update(Msg::Engine(luxe_core::event::Event::MessageCommitted {
        session_id: "session-a".into(),
        entry_id: luxe_core::session::EntryId("entry-steer".into()),
        message: luxe_core::message::Message::user("actually, check the tests too"),
    }));
    app.update(Msg::Engine(luxe_core::event::Event::SteeringDelivered { count: 1 }));

    assert!(app.pending_submissions.is_empty(), "the tray row retires on delivery");
    let steers = app
        .convo
        .iter()
        .filter(|item| {
            matches!(item.block(), ConvoBlock::User { text, .. } if text == "actually, check the tests too")
        })
        .count();
    assert_eq!(steers, 1, "the delivered steer appears exactly once in the transcript");

    let buffer = render_app(&mut app, &backend);
    let content = buffer_to_string(&buffer);
    assert!(!content.contains("steering"), "the tray row retires on delivery: {content}");
    assert!(content.contains("check the tests too"), "the message stays, in the transcript");
}

/// Cancelling hands a steer the engine never took in back to the composer.
///
/// A message that never ran belongs somewhere it can be fixed and re-sent, not in the transcript as
/// a read-only row — interrupting is usually the first half of "let me put that differently". The
/// engine has to be told as well: a steer is admitted the moment it is submitted, so a UI that only
/// cleared its own tray would leave the recalled text queued for the next turn's first checkpoint.
#[test]
fn a_cancelled_steer_comes_back_to_the_composer() {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<luxe_core::engine::Action>(64);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    app.update(Msg::Engine(luxe_core::event::Event::TokenDelta {
        block: 0,
        delta: "working".into(),
    }));
    app.editor.insert_text("never mind, stop");
    app.update(Msg::Key(key(KeyCode::Enter)));
    assert!(matches!(rx.try_recv(), Ok(luxe_core::engine::Action::SteerTurn { .. })));
    assert_eq!(app.editor.current_input(), "", "submitting cleared the draft");

    app.update(Msg::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::CONTROL)));

    assert!(app.pending_submissions.is_empty(), "nothing is left waiting");
    assert_eq!(app.editor.current_input(), "never mind, stop", "and it is editable again");
    assert!(
        !app.convo.iter().any(
            |item| matches!(item.block(), ConvoBlock::User { text, .. } if text == "never mind, stop")
        ),
        "a message that never ran is not transcript history"
    );
    let sent: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
    assert!(
        sent.iter().any(|action| matches!(action, luxe_core::engine::Action::Cancel)),
        "the turn is still cancelled: {sent:?}"
    );
    assert!(
        sent.iter().any(|action| matches!(action, luxe_core::engine::Action::WithdrawSteering)),
        "and the engine drops the steer it had buffered: {sent:?}"
    );
}

/// Alt+Up takes back everything waiting, in the order it was typed, ahead of the current draft.
#[test]
fn the_recall_key_returns_every_waiting_message_to_the_composer() {
    let (width, height) = (80, 24);
    let backend = test_backend(width, height);
    let (tx, mut rx) = tokio::sync::mpsc::channel::<luxe_core::engine::Action>(64);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    app.update(Msg::Engine(luxe_core::event::Event::TokenDelta {
        block: 0,
        delta: "working".into(),
    }));

    app.editor.insert_text("also check the reconnect backoff");
    app.update(Msg::Key(key(KeyCode::Enter)));
    app.editor.insert_text("then update the architecture doc");
    app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT)));
    assert_eq!(app.pending_submissions.len(), 2, "one steer, one follow-up");

    // The tray says which key brings them back, or the feature is invisible.
    let content = buffer_to_string(&render_app(&mut app, &backend));
    assert!(content.contains("alt+up to edit them here"), "{content}");

    app.editor.insert_text("and a half-typed third");
    app.update(Msg::Key(KeyEvent::new(KeyCode::Up, KeyModifiers::ALT)));

    assert!(app.pending_submissions.is_empty(), "the tray empties");
    assert_eq!(
        app.editor.current_input(),
        "also check the reconnect backoff\n\nthen update the architecture doc\n\nand a half-typed third",
        "restored in submission order, before what was being typed"
    );
    let sent: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
    assert!(
        sent.iter().any(|action| matches!(action, luxe_core::engine::Action::WithdrawSteering)),
        "the buffered steer is withdrawn: {sent:?}"
    );

    // Pressing it again has nothing to take back, and says so rather than reading as a dead key.
    app.update(Msg::Key(KeyEvent::new(KeyCode::Up, KeyModifiers::ALT)));
    let content = buffer_to_string(&render_app(&mut app, &backend));
    assert!(content.contains("Nothing waiting to restore"), "{content}");
}

/// A steer that could not be sent at all still shows up, marked as never sent.
///
/// The one remaining path to that marker: with the engine gone there is nothing to recall from and
/// nothing that will ever deliver it, so the transcript is where it has to be said.
#[test]
fn a_steer_the_engine_never_received_is_marked_never_sent() {
    let (tx, rx) = tokio::sync::mpsc::channel::<luxe_core::engine::Action>(64);
    drop(rx);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    app.update(Msg::Engine(luxe_core::event::Event::TokenDelta {
        block: 0,
        delta: "working".into(),
    }));

    app.editor.insert_text("one more thing");
    app.update(Msg::Key(key(KeyCode::Enter)));

    assert!(app.pending_submissions.is_empty(), "nothing is waiting on a delivery that can't come");
    let landed = app
        .convo
        .iter()
        .find(
            |item| matches!(item.block(), ConvoBlock::User { text, .. } if text == "one more thing"),
        )
        .expect("the undeliverable steer is in the transcript");
    assert!(app.was_never_sent(&landed.id), "and is marked as never sent");
}

/// The tray holds its ground while the turn keeps streaming: a pending message must not drift up
/// with the transcript or scroll out of view before it has run.
#[test]
fn a_pending_message_stays_pinned_above_the_composer_while_output_streams() {
    let (width, height) = (80, 24);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.update(Msg::Engine(luxe_core::event::Event::TokenDelta {
        block: 0,
        delta: "working".into(),
    }));
    app.editor.insert_text("one more thing");
    app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT)));

    let row_of_tray = |app: &mut App| {
        let buffer = render_app(app, &backend);
        (0..height).find(|&y| row_text(&buffer, y, width).contains("one more thing"))
    };
    let before = row_of_tray(&mut app).expect("the tray row renders");

    for index in 0..40 {
        app.update(Msg::Engine(luxe_core::event::Event::TokenDelta {
            block: 0,
            delta: format!("streamed line {index}\n"),
        }));
    }
    let after = row_of_tray(&mut app).expect("the tray row is still on screen");
    assert_eq!(before, after, "a pending message holds its row while the transcript scrolls");
}

/// Alt+Enter queues a follow-up turn instead: held (visibly, with its position) until the current
/// turn ends, then promoted one at a time.
#[test]
fn alt_enter_queues_a_follow_up_turn_promoted_when_the_turn_ends() {
    let (width, height) = (80, 24);
    let backend = test_backend(width, height);
    let (tx, mut rx) = tokio::sync::mpsc::channel::<luxe_core::engine::Action>(64);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    app.update(Msg::Engine(luxe_core::event::Event::TokenDelta {
        block: 0,
        delta: "working".into(),
    }));

    app.editor.insert_text("then update the docs");
    app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT)));
    assert!(rx.try_recv().is_err(), "a queued message must not reach the engine yet");
    assert_eq!(app.queued_count(), 1);

    let buffer = render_app(&mut app, &backend);
    let content = buffer_to_string(&buffer);
    assert!(content.contains("queued"), "the queued message says so: {content}");
    assert!(content.contains("#1"), "and carries its position: {content}");

    // The turn ends: exactly one queued message is promoted into a real turn.
    app.update(Msg::Engine(luxe_core::event::Event::TurnEnded {
        reason: luxe_core::event::StopReason::EndTurn,
    }));
    match rx.try_recv().expect("the queued message is promoted") {
        luxe_core::engine::Action::SubmitUserMessage { content, .. } => {
            assert_eq!(content, "then update the docs");
        }
        other => panic!("expected SubmitUserMessage, got {other:?}"),
    }
    assert_eq!(app.queued_count(), 0);
    assert!(app.status.turn_started.is_some(), "promotion starts the turn clock");
    let buffer = render_app(&mut app, &backend);
    let content = buffer_to_string(&buffer);
    assert!(!content.contains("queued"), "the tray row retires on promotion: {content}");
    assert!(content.contains("then update the docs"), "and the message joins the transcript");
}
