use super::*;
use serde_json::json;

/// Loading a deferred tool's schema is our own request-budget bookkeeping, not a step in the
/// reader's task — ship every schema and the call does not exist. A failed one still shows,
/// because a call nobody can see cannot explain the stall it caused, and an invisible retry
/// loop is worse than a visible one.
#[test]
fn housekeeping_that_went_fine_stays_out_of_the_transcript() {
    let activity = |name: &str, state| {
        let mut activity = ToolActivity::running("s", "c", name, name, 0);
        activity.state = state;
        activity
    };

    assert!(is_bookkeeping(crate::tool::ACTIVATE_TOOLS_NAME));
    for tool in ["read", "bash", "screenshot", "mcp", "task"] {
        assert!(!is_bookkeeping(tool), "{tool} is work, not housekeeping");
    }

    let activate = crate::tool::ACTIVATE_TOOLS_NAME;
    for quiet in [
        ToolActivityState::Composing,
        ToolActivityState::Queued,
        ToolActivityState::Running,
        ToolActivityState::Succeeded,
    ] {
        assert!(!activity(activate, quiet).shows_in_transcript(), "{quiet:?}");
    }
    for loud in
        [ToolActivityState::Failed, ToolActivityState::Cancelled, ToolActivityState::Interrupted]
    {
        assert!(
            activity(activate, loud).shows_in_transcript(),
            "{loud:?} has to be visible — it is the one that explains a stall"
        );
    }

    // Nothing else is ever hidden, whatever it did.
    for state in [
        ToolActivityState::Composing,
        ToolActivityState::Queued,
        ToolActivityState::Running,
        ToolActivityState::Succeeded,
    ] {
        assert!(activity("screenshot", state).shows_in_transcript());
    }
}

/// A run is known in full from the message that asked for it, which is what lets its first card
/// already be a fold. Short runs are absent, so a front-end never has to count.
#[test]
fn a_message_declares_the_runs_it_asked_for() {
    let call = |id: &str, name: &str| crate::message::ContentBlock::ToolCall {
        id: id.to_string(),
        name: name.to_string(),
        arguments: json!({}),
    };
    let place =
        |runs: &HashMap<String, ToolRun>, id: &str| runs.get(id).map(|run| (run.index, run.total));

    let runs = lookup_runs(&[
        crate::message::ContentBlock::Text("prose never separates two cards".into()),
        call("a", "read"),
        call("b", "grep"),
        call("c", "read"),
    ]);
    assert_eq!(place(&runs, "a"), Some((1, 3)));
    assert_eq!(place(&runs, "b"), Some((2, 3)));
    assert_eq!(place(&runs, "c"), Some((3, 3)));

    // Supervised work breaks a run, and what is left either stands or is too short to fold.
    let runs = lookup_runs(&[
        call("a", "read"),
        call("b", "read"),
        call("edit", "edit"),
        call("c", "read"),
        call("d", "read"),
        call("e", "read"),
    ]);
    assert_eq!(place(&runs, "a"), None, "two lookups are not a wall");
    assert_eq!(place(&runs, "edit"), None, "an edit is never folded away");
    assert_eq!(place(&runs, "c"), Some((1, 3)));
    assert_eq!(place(&runs, "e"), Some((3, 3)));

    // A hidden housekeeping call draws nothing, so it cannot break what it is not in.
    let runs = lookup_runs(&[
        call("a", "read"),
        call("h", crate::tool::ACTIVATE_TOOLS_NAME),
        call("b", "read"),
        call("c", "read"),
    ]);
    assert_eq!(place(&runs, "c"), Some((3, 3)));
    assert_eq!(place(&runs, "h"), None);

    assert!(lookup_runs(&[call("a", "read")]).is_empty());
    assert!(lookup_runs(&[]).is_empty());
}

/// A failed card carries the tool's own sentence, so a reader learns the reason without opening
/// the details. This is shared by the live projector and the resume replay for exactly that
/// reason — a card that reads differently after a restart is a card nobody trusts.
#[test]
fn a_failed_card_says_what_went_wrong_in_the_tools_own_words() {
    assert_eq!(failure_reason("Error: no such file: a.rs"), "no such file: a.rs");
    assert_eq!(failure_reason("\n\nError: bad glob\nmore detail"), "bad glob");
    assert_eq!(failure_reason(""), "Failed", "a silent failure still needs a word");

    // The sequence guards are instructions, not faults, and get the plainer phrasing.
    assert_eq!(
        failure_reason("Error: read-before-edit guard: read a.rs first"),
        "edited before reading the file"
    );
    assert_eq!(
        failure_reason("Error: a.rs changed on disk since it was read"),
        "file changed since it was read"
    );

    // An interrupted call's explanation is the case this has to carry well: it is the whole
    // reason the reader can tell a retry-safe stop from a risky one.
    let interrupted = crate::tool_pairing::Unfinished::NeverStarted.message();
    assert!(failure_reason(interrupted).starts_with("not executed"), "{interrupted}");

    // Long output is cut on a character boundary with an ellipsis, never mid-grapheme panic.
    let long = format!("Error: {}", "é".repeat(200));
    let summary = failure_reason(&long);
    assert!(summary.ends_with('\u{2026}'));
    assert_eq!(summary.chars().count(), FAILURE_SUMMARY_LIMIT + 1);
}

/// The guards and dispatch format their refusals from the phrases `failure_reason` reads, so
/// each phrase, wherever it sits in a message, still gets its plain wording.
#[test]
fn every_shared_failure_phrase_is_recognized() {
    let cases = [
        (
            format!("Error: no read ({})", guards::READ_BEFORE_EDIT),
            "edited before reading the file",
        ),
        (format!("Error: x ({})", guards::READ_BEFORE_WRITE), "overwrote a file it hadn't read"),
        (
            format!("Error: 'a.rs' {} you read it", guards::CHANGED_ON_DISK),
            "file changed since it was read",
        ),
        (format!("{}: git commit", denial::BY_USER), "Rejected"),
        (format!("{}: git commit", denial::BY_POLICY), "Denied by the permission rules"),
    ];
    for (message, plain) in cases {
        assert_eq!(failure_reason(&message), plain, "{message}");
    }
}

#[test]
fn running_activity_roundtrips() {
    let activity = ToolActivity::running("session-1", "call-1", "read", "Read workspace.rs", 42);
    let encoded = serde_json::to_string(&activity).unwrap();
    let decoded: ToolActivity = serde_json::from_str(&encoded).unwrap();

    assert_eq!(decoded, activity);
    assert!(!decoded.is_terminal());
}

/// A session written before card bodies existed still loads.
#[test]
fn an_activity_without_a_card_body_still_decodes() {
    let legacy = json!({
        "session_id": "session-1", "turn_id": null, "call_id": "call-1", "revision": 0,
        "name": "write", "label": "a.rs", "state": "running", "started_at_ms": 0,
        "elapsed_ms": null, "summary": null, "input_preview": null, "output_preview": null
    });
    let decoded: ToolActivity = serde_json::from_value(legacy).expect("legacy activity");
    assert!(decoded.card_body.is_none());
}

#[test]
fn terminal_states_are_final() {
    for open in
        [ToolActivityState::Composing, ToolActivityState::Queued, ToolActivityState::Running]
    {
        assert!(!open.is_terminal());
        let encoded = serde_json::to_string(&open).unwrap();
        assert_eq!(serde_json::from_str::<ToolActivityState>(&encoded).unwrap(), open);
    }
    // The model writing a call and the tool running it are both work; a call that has been
    // written and is waiting its turn is the only one that is genuinely idle.
    assert!(ToolActivityState::Composing.is_working());
    assert!(ToolActivityState::Running.is_working());
    assert!(!ToolActivityState::Queued.is_working());
    for state in [
        ToolActivityState::Succeeded,
        ToolActivityState::Failed,
        ToolActivityState::Cancelled,
        ToolActivityState::Interrupted,
    ] {
        assert!(state.is_terminal());
        let encoded = serde_json::to_string(&state).unwrap();
        assert_eq!(serde_json::from_str::<ToolActivityState>(&encoded).unwrap(), state);
    }
}
