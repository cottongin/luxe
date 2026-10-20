use super::*;
use luxe_core::Message;

/// Frames as a session sends them, copied from core's pinned wire shapes.
const PERMISSION: &str = r#"{"kind":"permission_requested","content":{"id":"perm:1","session_id":"session-1","tool":"bash","permission":"bash","summary":"Run: rm -rf build","always_patterns":["rm *"]}}"#;
const QUESTION: &str = r#"{"kind":"question_asked","content":{"id":"ask:1","session_id":"session-1","prompt":"Which database?","options":[{"label":"postgres","description":"Relational, boring, correct","recommended":true}],"multi":false}}"#;
const CLOSED: &str = r#"{"kind":"prompt_closed","content":{"id":"perm:1"}}"#;
const DELTA: &str = r#"{"kind":"token_delta","content":{"block":0,"delta":"hello"}}"#;
const FINISHED: &str = r#"{"kind":"turn_ended","content":{"reason":"end_turn"}}"#;

fn event(line: &str) -> Event {
    serde_json::from_str(line).expect("a protocol frame")
}

fn committed(message: Message) -> Event {
    let frame = serde_json::json!({
        "kind": "message_committed",
        "content": { "session_id": "session-1", "entry_id": "e1", "message": message },
    });
    serde_json::from_value(frame).expect("a committed message")
}

fn reading() -> Reading {
    Reading::new("session-1", "/home/alex/work/luxe-dev")
}

/// A notice for `body` from the reading above.
fn notice(body: &str, urgent: bool) -> Notice {
    let (title, session) = ("luxe-dev".into(), "session-1".into());
    Notice { title, body: body.into(), session, urgent }
}

/// The break this catches: the command, the question's text or its options in the notification,
/// where a lock screen shows them to whoever is looking.
#[test]
fn a_prompt_says_what_waits_and_never_the_detail() {
    let now = Instant::now();
    let asked = reading().read(&event(PERMISSION), now);
    assert_eq!(asked, Some(Heard::Prompt("perm:1".into(), notice("Approve bash", true))));
    let asked = reading().read(&event(QUESTION), now);
    let expected = Heard::Prompt("ask:1".into(), notice("The agent has a question for you", true));
    assert_eq!(asked, Some(expected));
    assert_eq!(reading().read(&event(CLOSED), now), Some(Heard::Closed("perm:1".into())));
}

/// The break this catches: a quick reply at the desk buzzing the phone, or a long turn ending
/// without a word to someone who walked away.
#[test]
fn only_a_long_turns_end_is_news() {
    let start = Instant::now();
    let mut quick = reading();
    quick.read(&committed(Message::user("hi")), start);
    assert_eq!(quick.read(&event(FINISHED), start + LONG_TURN - Duration::from_secs(1)), None);
    let mut long = reading();
    long.read(&committed(Message::user("refactor it")), start);
    let ended = long.read(&event(FINISHED), start + LONG_TURN);
    assert_eq!(ended, Some(Heard::Now(notice("Finished, and waiting for you", false))));
}

/// The break this catches: timing a turn from its first sign each time it shows (every delta),
/// so no turn is ever long; or from a reply the model committed, which isn't a turn starting.
#[test]
fn a_turn_is_timed_from_its_first_sign() {
    let start = Instant::now();
    let mut reading = reading();
    reading.read(&event(DELTA), start);
    reading.read(&event(DELTA), start + LONG_TURN);
    assert!(reading.read(&event(FINISHED), start + LONG_TURN).is_some(), "timed from the first");

    let mut replies = super::tests::reading();
    replies.read(&committed(Message::assistant("done")), start);
    assert_eq!(replies.read(&event(FINISHED), start + LONG_TURN), None, "a reply isn't a start");
}

/// A turn is timed from `turn_started`, ahead of anything it streams.
///
/// The break this catches: ignoring `turn_started`, so a turn that spends its first seconds
/// waiting on the model is timed short, and its end isn't news.
#[test]
fn a_turn_is_timed_from_when_it_started() {
    let start = Instant::now();
    let mut reading = reading();
    reading.read(&event(r#"{"kind":"turn_started"}"#), start);
    reading.read(&event(DELTA), start + Duration::from_secs(20));
    assert!(reading.read(&event(FINISHED), start + LONG_TURN).is_some());
}

/// The break this catches: a turn's start carried past its end, so the next quick turn looks long.
#[test]
fn each_turn_is_timed_on_its_own() {
    let start = Instant::now();
    let mut reading = reading();
    reading.read(&event(DELTA), start);
    reading.read(&event(FINISHED), start + Duration::from_secs(1));
    reading.read(&event(DELTA), start + LONG_TURN);
    assert_eq!(reading.read(&event(FINISHED), start + LONG_TURN + Duration::from_secs(1)), None);
}

/// The break this catches: reading a stop's free-text reason (ruled out), or calling a cancelled
/// or failed turn finished.
#[test]
fn a_turn_that_stopped_says_so() {
    let start = Instant::now();
    let mut reading = reading();
    reading.read(&event(DELTA), start);
    let stopped = Event::TurnEnded { reason: StopReason::Other("cancelled".into()) };
    assert_eq!(
        reading.read(&stopped, start + LONG_TURN),
        Some(Heard::Now(notice("Stopped", false)))
    );
}

/// The break this catches: a notice naming the session the watcher joined after the terminal
/// moved to another one, so opening it lands on a session that isn't running.
#[test]
fn a_notice_names_the_session_now_running() {
    let mut reading = reading();
    let loaded = serde_json::json!({
        "kind": "session_loaded", "content": { "session_id": "session-2", "messages": [] },
    });
    reading.read(&serde_json::from_value(loaded).unwrap(), Instant::now());
    let Some(Heard::Prompt(_, asked)) = reading.read(&event(PERMISSION), Instant::now()) else {
        panic!("a prompt is heard");
    };
    assert_eq!(asked.session, "session-2");
}
