use super::*;

/// Copied from a real gpt-6.1-sol reply that went on like this for 128k tokens.
#[test]
fn a_blank_run_with_line_breaks_runs_away_and_indentation_does_not() {
    let stuck = format!("{{\"subjects\":[\"a\"] \t\n \r\n{}", " \n".repeat(140));
    assert!(runs_away(&stuck));
    assert!(!runs_away(&stuck[..stuck.len() - 200]), "not yet that long");
    let pretty = format!("{{\n{}\"n\": 1\n}}", " ".repeat(40));
    assert!(!runs_away(&pretty));
    // Spaces alone can sit inside a string, where they may be the content
    let padded = format!("{{\"text\":\"{}", " ".repeat(1000));
    assert!(!runs_away(&padded));
}

fn call(index: usize, name: &str, arguments: &str) -> ToolCallInfo {
    ToolCallInfo {
        index,
        id: format!("call-{index}"),
        name: name.into(),
        arguments: arguments.into(),
    }
}

fn reply(calls: Vec<ToolCallInfo>) -> StreamedReply {
    StreamedReply {
        text: String::new(),
        thinking: HashMap::new(),
        order: Vec::new(),
        tool_calls: calls,
        stop_reason: None,
    }
}

/// Reasoning that is only an opaque token still commits, because it is replayed: Copilot's
/// Gemini sends its state with no readable text at all.
#[test]
fn opaque_reasoning_commits_without_text() {
    let mut reply = reply(Vec::new());
    reply.note_thinking_block(0).opaque = Some("CpcBAb4+9vs".into());
    let blocks = reply.into_blocks();
    assert!(matches!(
        blocks.as_slice(),
        [ContentBlock::Thinking { text, opaque: Some(opaque), .. }]
            if text.is_empty() && opaque == "CpcBAb4+9vs"
    ));
}

/// A call is handed over once it is closed, in order; the first call that has to wait makes
/// every later one wait, so nothing runs ahead of a call written before it.
#[test]
fn calls_are_handed_over_in_order_once_closed() {
    let (to_run, mut handed) = mpsc::unbounded_channel();
    let mut handover = Handover::new(Some(to_run));
    let mut reply = reply(vec![call(0, "edit", r#"{"path":"a"}"#)]);
    assert!(!handover.advance(&reply), "still being written");

    reply.tool_calls.push(call(1, "bash", r#"{"command":"#));
    handover.close_all(&reply);
    reply.tool_calls[1].arguments.push_str(r#""ls"}"#);
    assert!(handover.advance(&reply), "a later block closed both");
    assert_eq!(handed.try_recv().map(|call| call.index), Ok(0));
    assert_eq!(handed.try_recv().map(|call| call.index), Ok(1));

    reply.tool_calls.push(call(2, "read", r#"{"path":"b"}"#));
    reply.tool_calls.push(call(3, "test", ""));
    handover.close_all(&reply);
    assert!(!handover.advance(&reply), "a lookup waits for the message");
    assert!(handed.try_recv().is_err(), "and so does the call after it");
}

/// Only a call that is whole, named, and not a lookup runs early.
#[test]
fn what_may_run_early() {
    assert!(runs_early(&call(0, "edit", r#"{"path":"a"}"#)));
    assert!(runs_early(&call(0, "test", "")), "closed with no arguments takes none");
    assert!(!runs_early(&call(0, "edit", r#"{"path":"#)), "cut off mid-argument");
    assert!(!runs_early(&call(0, "edit", "[1]")), "arguments are an object");
    assert!(!runs_early(&call(0, "grep", r#"{"pattern":"x"}"#)), "a lookup folds");
    let mut nameless = call(0, "edit", "{}");
    nameless.id.clear();
    assert!(!runs_early(&nameless), "half an identity");
}

/// A wrap-up turn hands nothing over, however finished its calls are.
#[test]
fn nothing_is_handed_over_without_somewhere_to_run_it() {
    let mut handover = Handover::new(None);
    let reply = reply(vec![call(0, "edit", "{}")]);
    handover.close_all(&reply);
    assert!(!handover.advance(&reply));
}

/// A lookup never composes: three adjacent ones fold into a single line, declared by the
/// committed message, so a card opened early is a row the fold then has to take back.
#[test]
fn only_a_call_worth_watching_arrive_opens_a_card_early() {
    for tool in ["write", "edit", "bash", "task", "screenshot"] {
        assert!(composes("call-1", tool), "{tool} should open its card early");
    }
    for tool in ["read", "grep", "glob", "symbol_search", crate::tool::ACTIVATE_TOOLS_NAME] {
        assert!(!composes("call-1", tool), "{tool} must wait for dispatch");
    }
    // Half an identity cannot be correlated with the call that follows it, so it keeps the old
    // behavior and gets its card at `ToolStarted`.
    assert!(!composes("", "write"));
    assert!(!composes("call-1", ""));
}

/// The first subject is worth interrupting the cadence for: one small string, and it is what
/// turns a bare spinner into a card. The payload is resent whole each time, so it waits its turn
/// rather than spending the channel on frames nobody draws.
#[test]
fn the_payload_is_rate_limited_and_the_name_is_not() {
    let start = std::time::Instant::now();
    let mut state = Composed::default();
    let frame = |state: &mut Composed, after_ms: u64, arguments: &str| {
        state.next_frame(start + std::time::Duration::from_millis(after_ms), "write", arguments)
    };

    assert!(frame(&mut state, 0, "{\"path\":\"a.rs").is_none(), "nothing has closed yet");

    // The path closes: named at once, carrying whatever of the file has landed — there is no
    // cadence to respect until something has actually gone out.
    let named = frame(&mut state, 1, "{\"path\":\"a.rs\",\"content\":\"one\\n").expect("a frame");
    assert_eq!(named.subject.as_deref(), Some("a.rs"));
    assert_eq!(named.body.map(|body| body.total_lines), Some(1));

    // Inside the window the file keeps arriving and the channel stays quiet.
    assert!(frame(&mut state, 50, "{\"path\":\"a.rs\",\"content\":\"one\\ntwo\\n").is_none());

    let caught_up = frame(&mut state, 200, "{\"path\":\"a.rs\",\"content\":\"one\\ntwo\\n")
        .expect("the window has passed");
    assert!(caught_up.subject.is_none(), "the name has not changed, so it is not resent");
    assert_eq!(caught_up.body.map(|body| body.total_lines), Some(2), "and it caught up");

    // A payload that has not grown is not a frame, however long it has been.
    assert!(frame(&mut state, 900, "{\"path\":\"a.rs\",\"content\":\"one\\ntwo\\n").is_none());
    let grown = frame(&mut state, 1_000, "{\"path\":\"a.rs\",\"content\":\"one\\ntwo\\nthree\\n")
        .expect("a third line");
    assert_eq!(grown.body.map(|body| body.total_lines), Some(3));
}

const BASH_COMMAND: &str = r#"{"command":"sleep 5","#;
const BASH_WHOLE: &str = r#"{"command":"sleep 5","description":"Wait for the build"}"#;

/// A `bash` is named by its command until the `description` written after it arrives, and then
/// by that, so the card running it says what it is for.
///
/// The break this catches: a subject sent once and never again, which leaves the command in the
/// title and again in the body until the call is over.
#[test]
fn a_later_member_renames_the_card() {
    let start = std::time::Instant::now();
    let mut state = Composed::default();
    let at = |ms| start + std::time::Duration::from_millis(ms);
    let named = state.next_frame(at(0), "bash", BASH_COMMAND).expect("the command closed");
    assert_eq!(named.subject.as_deref(), Some("sleep 5"));
    let renamed = state.next_frame(at(200), "bash", BASH_WHOLE).expect("the description closed");
    assert_eq!(renamed.subject.as_deref(), Some("Wait for the build"));
    assert!(state.next_frame(at(400), "bash", BASH_WHOLE).is_none(), "said once");
}

/// A call closed inside the cadence's window still tells its card the whole call before it is
/// handed over to run, since dispatch brings no arguments with it.
///
/// The break this catches: handing the call over with the frame still held back, so the card
/// runs under its command until the message commits after the call is over.
#[tokio::test]
async fn a_closed_call_tells_its_card_everything_before_it_runs() {
    let start = std::time::Instant::now();
    let mut composed = HashMap::from([(0, Composed::default())]);
    let state = composed.get_mut(&0).expect("just inserted");
    state.next_frame(start, "bash", BASH_COMMAND).expect("named by the command");
    let late = start + std::time::Duration::from_millis(10);
    assert!(state.next_frame(late, "bash", BASH_WHOLE).is_none(), "held back by the cadence");

    let (event_tx, mut events) = mpsc::channel(8);
    let (to_run, mut handed) = mpsc::unbounded_channel();
    let mut handover = Handover::new(Some(to_run));
    let reply = reply(vec![call(0, "bash", BASH_WHOLE)]);
    let end = ProviderEvent::ToolCallEnd { index: 0 };
    close_written(&end, &mut handover, &reply, &mut composed, &event_tx).await;

    let Ok(Event::ToolCallComposing { subject, .. }) = events.try_recv() else {
        panic!("the card heard nothing before the call was handed over");
    };
    assert_eq!(subject.as_deref(), Some("Wait for the build"));
    assert_eq!(handed.try_recv().map(|call| call.index), Ok(0));
    assert!(composed.is_empty(), "a written call is no longer watched");
}
