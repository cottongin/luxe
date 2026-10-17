use super::*;

/// The one update an event produced, for a test that expects exactly one.
trait One {
    fn one(self) -> Option<ToolActivity>;
}

impl One for Vec<ToolActivity> {
    fn one(self) -> Option<ToolActivity> {
        assert!(self.len() <= 1, "expected at most one update, got {}", self.len());
        self.into_iter().next()
    }
}

#[test]
fn projects_monotonic_activity_for_one_session() {
    let mut projector = ToolActivityProjector::default();
    assert!(projector
        .project(&Event::ToolStarted { call_id: "call-1".into(), name: "read".into() })
        .one()
        .is_none());
    projector.project(&Event::SessionLoaded { session_id: "session-a".into(), messages: vec![] });

    let running = projector
        .project(&Event::ToolStarted { call_id: "call-1".into(), name: "read".into() })
        .one()
        .unwrap();
    assert_eq!(running.revision, 0);
    assert_eq!(running.state, ToolActivityState::Running);
    assert_eq!(running.session_id, "session-a");

    let progress = projector
        .project(&Event::ToolProgress { call_id: "call-1".into(), chunk: "working".into() })
        .one()
        .unwrap();
    assert_eq!(progress.revision, 1);
    assert_eq!(progress.output_preview.as_deref(), Some("working"));

    let finished = projector
        .project(&Event::ToolEnded { call_id: "call-1".into(), is_error: false })
        .one()
        .unwrap();
    assert_eq!(finished.revision, 2);
    assert_eq!(finished.state, ToolActivityState::Succeeded);
    assert_eq!(finished.summary.as_deref(), Some("Completed"));
    assert!(finished.elapsed_ms.is_some());
    assert!(projector
        .project(&Event::ToolProgress { call_id: "call-1".into(), chunk: "late".into() })
        .one()
        .is_none());
}

/// A failed card's summary is the reader's only chance to learn what happened without opening
/// the details, so it carries the tool's own first line — shortened for the guards, whose
/// message is an instruction rather than a fault.
#[test]
fn a_failed_call_summarizes_its_own_error() {
    let cases = [
        (
            "Error: read 'src/lib.rs' before editing it (read-before-edit guard). Use the \
                 `read` tool first, then retry.",
            "edited before reading the file",
        ),
        (
            "Error: 'src/lib.rs' exists and has no read on record for this Luxe run \
                 (read-before-write guard), so overwriting it would be blind.",
            "overwrote a file it hadn't read",
        ),
        (
            "Error: 'src/lib.rs' changed on disk since you last read it. Re-read it, then \
                 retry the edit.",
            "file changed since it was read",
        ),
        ("Error: edit[0] 'oldText' not found in file", "edit[0] 'oldText' not found in file"),
    ];

    for (output, expected) in cases {
        let mut projector = ToolActivityProjector::default();
        projector
            .project(&Event::SessionLoaded { session_id: "session-a".into(), messages: vec![] });
        projector.project(&Event::ToolStarted { call_id: "call-1".into(), name: "edit".into() });
        projector.project(&Event::MessageCommitted {
            session_id: "session-a".into(),
            entry_id: luxe_core::session::EntryId("entry-1".into()),
            message: Message {
                role: Role::Tool,
                content: vec![ContentBlock::ToolResult {
                    call_id: "call-1".into(),
                    content: vec![ToolOutput::Text(output.to_string())],
                    is_error: true,
                }],
            },
        });
        let failed = projector
            .project(&Event::ToolEnded { call_id: "call-1".into(), is_error: true })
            .one()
            .expect("a terminal activity");
        assert_eq!(failed.state, ToolActivityState::Failed);
        assert_eq!(failed.summary.as_deref(), Some(expected));
        assert!(failed.output_preview.is_some(), "the full message stays available");
    }
}

/// With no result body to quote, the summary still has to say the call failed.
#[test]
fn a_failed_call_without_output_still_says_it_failed() {
    let mut projector = ToolActivityProjector::default();
    projector.project(&Event::SessionLoaded { session_id: "session-a".into(), messages: vec![] });
    projector.project(&Event::ToolStarted { call_id: "call-1".into(), name: "bash".into() });
    let failed = projector
        .project(&Event::ToolEnded { call_id: "call-1".into(), is_error: true })
        .one()
        .expect("a terminal activity");
    assert_eq!(failed.summary.as_deref(), Some("Failed"));
}

/// Opens a composing frame for `call-1`, as the stream does.
fn composing(subject: Option<&str>, body: Option<&str>) -> Event {
    Event::ToolCallComposing {
        call_id: "call-1".into(),
        name: "write".into(),
        subject: subject.map(str::to_string),
        body: body.map(|text| {
            luxe_core::tool_card_body(
                "write",
                &serde_json::json!({ "path": "notes.txt", "content": text }),
                "/project",
            )
            .expect("a write carries its content")
        }),
    }
}

/// A `write`'s content *is* its argument, so the model spends far longer writing the call than
/// the tool spends running it. The card opens for that whole stretch, picks up the path as soon
/// as it can be read out of the fragment, fills in as the file is written, and is adopted —
/// not duplicated — by the dispatch.
#[test]
fn a_card_opens_and_fills_in_while_the_model_writes_the_call() {
    let mut projector = ToolActivityProjector::default();
    projector.project(&Event::SessionLoaded { session_id: "session-a".into(), messages: vec![] });

    let opened = projector
        .project(&composing(None, None))
        .one()
        .expect("the card exists before the call does");
    assert_eq!(
        (opened.revision, opened.state),
        (0, ToolActivityState::Composing),
        "the model is writing it, which is work the card has to show as work",
    );
    assert_eq!(opened.label, "", "a header does not say the tool's name back to itself");
    assert!(opened.card_body.is_none(), "and nothing of the file has arrived");

    let named = projector
        .project(&composing(Some("notes.txt"), None))
        .one()
        .expect("the path arriving is worth a frame");
    assert_eq!((named.revision, named.label.as_str()), (1, "notes.txt"));
    assert!(
        projector.project(&composing(Some("notes.txt"), None)).one().is_none(),
        "and saying it twice is not",
    );

    // The file filling in, which is the whole reason a card exists this early: the count the
    // reader watches climb is `total_lines`.
    let counts: Vec<usize> = ["one\n", "one\ntwo\n", "one\ntwo\nthree\n"]
        .iter()
        .filter_map(|content| projector.project(&composing(Some("notes.txt"), Some(content))).one())
        .filter_map(|activity| activity.card_body.map(|body| body.total_lines))
        .collect();
    assert_eq!(counts, [1, 2, 3]);
    assert!(
        projector.project(&composing(Some("notes.txt"), Some("one\ntwo\nthree\n"))).one().is_none(),
        "a payload that has not grown is not a frame",
    );

    projector.project(&Event::MessageCommitted {
        session_id: "session-a".into(),
        entry_id: luxe_core::session::EntryId("entry-1".into()),
        message: Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolCall {
                id: "call-1".into(),
                name: "write".into(),
                arguments: serde_json::json!({"path": "notes.txt", "content": "one\ntwo\nthree\n"}),
            }],
        },
    });
    let started = projector
        .project(&Event::ToolStarted { call_id: "call-1".into(), name: "write".into() })
        .one()
        .expect("dispatch revises the card it already has");
    // Composing it, naming it, three frames of the file, settling into the queue when the
    // message landed, and dispatch: one row revised six times, never a second card.
    assert_eq!(started.revision, 6, "one row, revised — not a second card");
    assert_eq!(started.state, ToolActivityState::Running, "dispatch is what starts it");
    assert_eq!(
        started.card_body.map(|body| body.total_lines),
        Some(3),
        "the committed payload agrees with the streamed one, so nothing on screen jumps",
    );
    assert!(
        started.started_at_ms >= named.started_at_ms,
        "the clock restarts at dispatch: composing time is the model's, not the tool's",
    );

    let ended = projector
        .project(&Event::ToolEnded { call_id: "call-1".into(), is_error: false })
        .one()
        .expect("and it still settles");
    assert_eq!((ended.revision, ended.state), (7, ToolActivityState::Succeeded));
}

/// A call that runs while the model writes the next one settles on its own `ToolCompleted`,
/// takes its full label once the message commits, and is not touched again by `ToolEnded`.
#[test]
fn a_call_that_runs_before_its_message_commits_settles_on_its_own() {
    let mut projector = ToolActivityProjector::default();
    projector.project(&Event::SessionLoaded { session_id: "session-a".into(), messages: vec![] });
    projector.project(&Event::ToolCallComposing {
        call_id: "call-1".into(),
        name: "bash".into(),
        subject: Some("cargo te".into()),
        body: None,
    });
    let running = projector
        .project(&Event::ToolStarted { call_id: "call-1".into(), name: "bash".into() })
        .one()
        .expect("it starts before anything commits");
    assert_eq!(running.state, ToolActivityState::Running);

    let done = projector
        .project(&Event::ToolCompleted {
            call_id: "call-1".into(),
            is_error: false,
            output: "4 passed".into(),
        })
        .one()
        .expect("its own completion settles it");
    assert_eq!(done.state, ToolActivityState::Succeeded);
    assert_eq!(done.output_preview.as_deref(), Some("4 passed"));

    let committed = projector.project(&Event::MessageCommitted {
        session_id: "session-a".into(),
        entry_id: luxe_core::session::EntryId("entry-1".into()),
        message: Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolCall {
                id: "call-1".into(),
                name: "bash".into(),
                arguments: serde_json::json!({ "command": "cargo test --workspace" }),
            }],
        },
    });
    let enriched = committed.one().expect("the commit fills in the card it already has");
    assert_eq!(enriched.state, ToolActivityState::Succeeded, "and leaves it settled");
    assert_eq!(enriched.label, "cargo test --workspace", "with the whole call's label");
    assert!(
        projector
            .project(&Event::ToolEnded { call_id: "call-1".into(), is_error: false })
            .is_empty(),
        "the batch's commit has nothing left to say about it"
    );
}

/// **Exactly one card is working at a time.** The invariant the whole lifecycle exists to show.
///
/// A model writes a message's calls in order and the engine runs them in order, so at every
/// moment there is one thing happening: the model writing a call, or a tool running one. Both
/// halves of that were reported broken. Drawing every un-started call as running put two
/// spinning `bash` cards side by side, which reads as two shells at once; drawing them all as
/// waiting put a paused mark on a card that was visibly filling in.
#[test]
fn exactly_one_card_is_working_at_a_time() {
    let mut projector = ToolActivityProjector::default();
    projector.project(&Event::SessionLoaded { session_id: "session-a".into(), messages: vec![] });
    let call = |id: &str, command: &str| ContentBlock::ToolCall {
        id: id.into(),
        name: "bash".into(),
        arguments: serde_json::json!({ "command": command }),
    };
    let compose = |id: &str, command: &str| Event::ToolCallComposing {
        call_id: id.into(),
        name: "bash".into(),
        subject: Some(command.into()),
        body: None,
    };
    let commit = Event::MessageCommitted {
        session_id: "session-a".into(),
        entry_id: luxe_core::session::EntryId("entry-1".into()),
        message: Message {
            role: Role::Assistant,
            content: vec![call("call-1", "vainfo"), call("call-2", "vainfo --display drm")],
        },
    };

    // Each step, and what the two cards read as once it has landed.
    let steps: Vec<(&str, Event)> = vec![
        ("the model opens the first call", compose("call-1", "vainfo")),
        ("and moves on to the second", compose("call-2", "vainfo --display drm")),
        ("the message lands", commit),
        (
            "the first is dispatched",
            Event::ToolStarted { call_id: "call-1".into(), name: "bash".into() },
        ),
        ("and finishes", Event::ToolEnded { call_id: "call-1".into(), is_error: false }),
        (
            "only then does the second start",
            Event::ToolStarted { call_id: "call-2".into(), name: "bash".into() },
        ),
    ];

    let mut shape = Vec::new();
    for (note, event) in steps {
        projector.project(&event);
        let mut states: Vec<(String, ToolActivityState)> =
            projector.calls.iter().map(|(id, row)| (id.clone(), row.state)).collect();
        states.sort_by(|left, right| left.0.cmp(&right.0));
        let working = states.iter().filter(|(_, state)| state.is_working()).count();
        assert!(working <= 1, "{note}: {working} cards claim to be working — {states:?}");
        shape.push((note, states.iter().map(|(_, state)| *state).collect::<Vec<_>>()));
    }

    use ToolActivityState::{Composing, Queued, Running, Succeeded};
    assert_eq!(
        shape,
        [
            ("the model opens the first call", vec![Composing]),
            ("and moves on to the second", vec![Queued, Composing]),
            ("the message lands", vec![Queued, Queued]),
            ("the first is dispatched", vec![Running, Queued]),
            ("and finishes", vec![Succeeded, Queued]),
            ("only then does the second start", vec![Succeeded, Running]),
        ],
    );
}

/// A refused or cancelled call is answered without ever being dispatched. Its card is the one
/// the model's writing opened, and it has to settle from there or it spins forever.
#[test]
fn a_composed_call_that_never_runs_still_settles() {
    let mut projector = ToolActivityProjector::default();
    projector.project(&Event::SessionLoaded { session_id: "session-a".into(), messages: vec![] });
    projector.project(&composing(Some("notes.txt"), None));
    let refused = projector
        .project(&Event::ToolEnded { call_id: "call-1".into(), is_error: true })
        .one()
        .expect("the refusal reaches the card");
    assert_eq!(refused.state, ToolActivityState::Failed);

    // Nothing arrives for it afterwards, whatever order the stragglers turn up in.
    assert!(projector.project(&composing(Some("other.txt"), Some("late\n"))).one().is_none());
    assert!(projector
        .project(&Event::ToolStarted { call_id: "call-1".into(), name: "write".into() })
        .one()
        .is_none());
}

#[test]
fn committed_tool_call_enriches_the_next_live_activity() {
    let mut projector = ToolActivityProjector::default();
    projector.project(&Event::SessionLoaded { session_id: "session-a".into(), messages: vec![] });
    projector.project(&Event::MessageCommitted {
        session_id: "session-a".into(),
        entry_id: luxe_core::session::EntryId("entry-1".into()),
        message: Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolCall {
                id: "call-1".into(),
                name: "read".into(),
                arguments: serde_json::json!({"path": "crates/luxe-tui/src/view.rs"}),
            }],
        },
    });

    let Some(running) = projector
        .project(&Event::ToolStarted { call_id: "call-1".into(), name: "read".into() })
        .one()
    else {
        panic!("committed call should enrich the matching lifecycle row");
    };
    assert_eq!(running.label, "crates/luxe-tui/src/view.rs");
    assert_eq!(running.input_preview.as_deref(), Some(r#"{"path":"crates/luxe-tui/src/view.rs"}"#));
}

/// The card's body comes from the call, not the result — so it survives the model's token budget.
///
/// A `write`'s result is capped at a couple of dozen diff rows before it ever reaches the
/// front-end; the arguments carry the whole file, which is what makes an honest line count and a
/// real expand possible.
#[test]
fn a_committed_write_call_carries_the_whole_file_to_the_card() {
    let mut projector = ToolActivityProjector::default();
    projector.project(&Event::SessionLoaded { session_id: "session-a".into(), messages: vec![] });
    let content = (1..=80).map(|n| format!("line {n}")).collect::<Vec<_>>().join("\n");
    projector.project(&Event::MessageCommitted {
        session_id: "session-a".into(),
        entry_id: luxe_core::session::EntryId("entry-1".into()),
        message: Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolCall {
                id: "call-1".into(),
                name: "write".into(),
                arguments: serde_json::json!({"path": "notes.txt", "content": content}),
            }],
        },
    });

    let Some(running) = projector
        .project(&Event::ToolStarted { call_id: "call-1".into(), name: "write".into() })
        .one()
    else {
        panic!("a committed write should enrich its lifecycle row");
    };
    let body = running.card_body.expect("a write carries its content");
    assert_eq!(body.total_lines, 80);
    assert!(body.text.starts_with("line 1\n"));
    assert!(!body.is_truncated(), "eighty lines is well inside the display budget");

    // A tool with nothing worth showing under its header stays a one-line card.
    projector.project(&Event::MessageCommitted {
        session_id: "session-a".into(),
        entry_id: luxe_core::session::EntryId("entry-2".into()),
        message: Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolCall {
                id: "call-2".into(),
                name: "read".into(),
                arguments: serde_json::json!({"path": "notes.txt"}),
            }],
        },
    });
    let read = projector
        .project(&Event::ToolStarted { call_id: "call-2".into(), name: "read".into() })
        .one()
        .expect("a read still projects");
    assert!(read.card_body.is_none());
}

/// A model decides for itself how many calls go in one message, and the reader's problem is the
/// denominator: with only the card in front of them, seven lookups landing in half a second are
/// a wall out of nowhere. The whole run is known here, before any of it has been dispatched.
#[test]
fn a_call_knows_its_place_in_the_run_its_message_asked_for() {
    let mut projector = ToolActivityProjector::default();
    projector.project(&Event::SessionLoaded { session_id: "session-a".into(), messages: vec![] });
    let call = |id: &str, path: &str| ContentBlock::ToolCall {
        id: id.into(),
        name: "read".into(),
        arguments: serde_json::json!({ "path": path }),
    };
    projector.project(&Event::MessageCommitted {
        session_id: "session-a".into(),
        entry_id: luxe_core::session::EntryId("entry-1".into()),
        message: Message {
            role: Role::Assistant,
            content: vec![call("call-1", "a.rs"), call("call-2", "b.rs"), call("call-3", "c.rs")],
        },
    });

    for (call_id, index) in [("call-1", 1), ("call-2", 2), ("call-3", 3)] {
        let started = projector
            .project(&Event::ToolStarted { call_id: call_id.into(), name: "read".into() })
            .one()
            .expect("a committed call projects");
        let batch = started.batch.expect("a call in a run knows its place");
        assert_eq!((batch.index, batch.total), (index, 3));
        // And that these three fold together, which the *first* card has to know or the fold
        // forms late and takes back rows the transcript already spent.
        let run = started.run.expect("three adjacent lookups are a run");
        assert_eq!((run.index, run.total), (index, 3));
    }

    // A message asking for one call has no run to be part of, and `1/1` would report the
    // absence of a queue as though it were one.
    projector.project(&Event::MessageCommitted {
        session_id: "session-a".into(),
        entry_id: luxe_core::session::EntryId("entry-2".into()),
        message: Message { role: Role::Assistant, content: vec![call("call-4", "d.rs")] },
    });
    let alone = projector
        .project(&Event::ToolStarted { call_id: "call-4".into(), name: "read".into() })
        .one()
        .expect("a lone call still projects");
    assert!(alone.batch.is_none());
    assert!(alone.run.is_none(), "and has nothing to fold into");
}

#[test]
fn committed_tool_result_enriches_the_following_terminal_activity() {
    let mut projector = ToolActivityProjector::default();
    projector.project(&Event::SessionLoaded { session_id: "session-a".into(), messages: vec![] });
    projector.project(&Event::ToolStarted { call_id: "call-1".into(), name: "bash".into() });
    projector.project(&Event::MessageCommitted {
        session_id: "session-a".into(),
        entry_id: luxe_core::session::EntryId("entry-2".into()),
        message: Message {
            role: Role::Tool,
            content: vec![ContentBlock::ToolResult {
                call_id: "call-1".into(),
                content: vec![ToolOutput::Text(format!("done sk-{}", "a".repeat(24)))],
                is_error: false,
            }],
        },
    });

    let Some(finished) =
        projector.project(&Event::ToolEnded { call_id: "call-1".into(), is_error: false }).one()
    else {
        panic!("committed result should enrich the matching terminal activity");
    };
    assert_eq!(finished.state, ToolActivityState::Succeeded);
    let Some(output) = finished.output_preview else {
        panic!("terminal activity should include the committed output preview");
    };
    assert!(output.contains("done"));
    assert!(!output.contains("sk-"));
}

#[test]
fn session_switch_isolates_reused_call_ids_and_redacts_previews() {
    let mut projector = ToolActivityProjector::default();
    projector.project(&Event::SessionLoaded { session_id: "session-a".into(), messages: vec![] });
    projector.project(&Event::ToolStarted { call_id: "call-1".into(), name: "bash".into() });
    projector.project(&Event::SessionLoaded { session_id: "session-b".into(), messages: vec![] });

    let running = projector
        .project(&Event::ToolStarted { call_id: "call-1".into(), name: "bash".into() })
        .one()
        .unwrap();
    assert_eq!(running.session_id, "session-b");
    assert_eq!(running.revision, 0);

    let preview = projector
        .project(&Event::ToolProgress {
            call_id: "call-1".into(),
            chunk: format!("sk-{}", "a".repeat(24)),
        })
        .one()
        .unwrap();
    assert!(!preview.output_preview.unwrap().contains("sk-"));
}
