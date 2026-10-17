//! Tool cards — activity rows, folds, interrupts and what a replay rebuilds.

use super::*;

#[tokio::test]
async fn session_load_rebuilds_finished_and_interrupted_tool_items() {
    use luxe_core::message::{ContentBlock, Message, Role, ToolOutput};
    use luxe_core::session::EntryId;

    let (tx, _rx) = mpsc::channel(100);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    let full_output = format!("{} canonical tail", "x".repeat(320));
    app.update(Msg::Engine(Event::SessionLoaded {
        session_id: "session-a".into(),
        messages: vec![
            luxe_core::event::SessionMessage {
                entry_id: EntryId("assistant-entry".into()),
                message: Message {
                    role: Role::Assistant,
                    content: vec![
                        ContentBlock::Text("Checking the files.".into()),
                        ContentBlock::ToolCall {
                            id: "call-read".into(),
                            name: "read".into(),
                            arguments: serde_json::json!({"path": "src/lib.rs"}),
                        },
                        ContentBlock::ToolCall {
                            id: "call-bash".into(),
                            name: "bash".into(),
                            arguments: serde_json::json!({"command": "cargo test"}),
                        },
                    ],
                },
            },
            luxe_core::event::SessionMessage {
                entry_id: EntryId("tool-entry".into()),
                message: Message {
                    role: Role::Tool,
                    content: vec![ContentBlock::ToolResult {
                        call_id: "call-read".into(),
                        content: vec![ToolOutput::Text(full_output.clone())],
                        is_error: false,
                    }],
                },
            },
        ],
    }));

    let read = app.tool_items.get(&("session-a".into(), "call-read".into())).unwrap();
    assert_eq!(read.activity.state, luxe_core::ToolActivityState::Succeeded);
    assert_eq!(read.activity.elapsed_ms, None);
    assert_ne!(read.activity.output_preview.as_deref(), Some(full_output.as_str()));
    app.selected_block = Some(1);
    assert!(app.copy_selected_block());
    assert!(app.outbox.copy.as_deref().is_some_and(|copy| copy.contains(&full_output)));
    let bash = app.tool_items.get(&("session-a".into(), "call-bash".into())).unwrap();
    assert_eq!(bash.activity.state, luxe_core::ToolActivityState::Interrupted);
    assert_eq!(bash.activity.summary.as_deref(), Some("Interrupted after session ended"));
    assert_eq!(bash.activity.elapsed_ms, None);
    assert!(matches!(
        transcript_shape(&app).as_slice(),
        [
            (_, ConvoBlock::Assistant(text)),
            (ConversationItemId::Tool { call_id: read_id, .. }, ConvoBlock::Tool(_)),
            (ConversationItemId::Tool { call_id: bash_id, .. }, ConvoBlock::Tool(_)),
        ] if text == "Checking the files." && read_id == "call-read" && bash_id == "call-bash"
    ));
}

/// A resumed session's cards read exactly like live ones, bodies included.
///
/// The body comes from the call's arguments, and a replayed session still has those — so a `write`
/// from three sessions ago still shows the file it wrote rather than dropping to a bare header.
#[tokio::test]
async fn a_replayed_session_rebuilds_card_bodies_from_the_calls() {
    use luxe_core::message::{ContentBlock, Message, Role};
    use luxe_core::session::EntryId;

    let (tx, _rx) = mpsc::channel(100);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    let content = (1..=40).map(|n| format!("line {n}")).collect::<Vec<_>>().join("\n");
    app.update(Msg::Engine(Event::SessionLoaded {
        session_id: "session-a".into(),
        messages: vec![luxe_core::event::SessionMessage {
            entry_id: EntryId("assistant-entry".into()),
            message: Message {
                role: Role::Assistant,
                content: vec![
                    ContentBlock::ToolCall {
                        id: "call-write".into(),
                        name: "write".into(),
                        arguments: serde_json::json!({"path": "notes.txt", "content": content}),
                    },
                    ContentBlock::ToolCall {
                        id: "call-read".into(),
                        name: "read".into(),
                        arguments: serde_json::json!({"path": "notes.txt"}),
                    },
                ],
            },
        }],
    }));

    let write = app.tool_items.get(&("session-a".into(), "call-write".into())).unwrap();
    let body = write.activity.card_body.as_ref().expect("a replayed write keeps its content");
    assert_eq!(body.total_lines, 40);
    let read = app.tool_items.get(&("session-a".into(), "call-read".into())).unwrap();
    assert!(read.activity.card_body.is_none(), "a read has nothing to show under its header");
}

/// A resumed session folds the runs the live one folded.
///
/// Which calls belong together is a fact about the message, and history has the message — so replay
/// can say it without claiming anything is still happening. Progress is the part replay has nothing
/// to report, which is why `batch` stays empty and the meter with it.
#[tokio::test]
async fn a_replayed_run_folds_the_way_the_live_one_did() {
    use luxe_core::message::{ContentBlock, Message, Role};
    use luxe_core::session::EntryId;

    let call = |id: &str, name: &str, path: &str| ContentBlock::ToolCall {
        id: id.into(),
        name: name.into(),
        arguments: serde_json::json!({ "path": path }),
    };
    let result = |id: &str| ContentBlock::ToolResult {
        call_id: id.into(),
        content: vec![luxe_core::message::ToolOutput::Text("a wall of matches".into())],
        is_error: false,
    };
    let (tx, _rx) = mpsc::channel(100);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    app.update(Msg::Engine(Event::SessionLoaded {
        session_id: "session-a".into(),
        messages: vec![
            luxe_core::event::SessionMessage {
                entry_id: EntryId("assistant-entry".into()),
                message: Message {
                    role: Role::Assistant,
                    content: vec![
                        call("c1", "read", "a.rs"),
                        call("c2", "read", "b.rs"),
                        call("c3", "read", "c.rs"),
                        call("c4", "edit", "a.rs"),
                    ],
                },
            },
            luxe_core::event::SessionMessage {
                entry_id: EntryId("tool-entry".into()),
                message: Message {
                    role: Role::Tool,
                    content: ["c1", "c2", "c3", "c4"].map(result).to_vec(),
                },
            },
        ],
    }));

    assert_eq!(folded(&app), vec![0..3], "the three lookups are one line again");
    let read = app.tool_items.get(&("session-a".into(), "c1".into())).unwrap();
    assert!(read.activity.batch.is_none(), "history has no progress to report");
}

/// The cards one assistant message's tool calls produce, exactly as the runtime builds them:
/// labels, bodies and run projection all from `luxe_core`, so a test card is the reader's card.
///
/// Every card comes back `Running`; a test that wants an outcome sets it, the way the call would.
pub(super) fn projected_message(calls: &[(&str, &str, serde_json::Value)]) -> Vec<ToolItem> {
    let content = calls
        .iter()
        .map(|(id, name, arguments)| luxe_core::message::ContentBlock::ToolCall {
            id: (*id).to_string(),
            name: (*name).to_string(),
            arguments: arguments.clone(),
        })
        .collect::<Vec<_>>();
    let runs = luxe_core::lookup_runs(&content);
    calls
        .iter()
        .map(|(id, name, arguments)| {
            let label = luxe_core::describe_call(name, arguments, "/project");
            let mut activity = ToolActivity::running("session-a", *id, *name, label, 1);
            // Exactly what the live projector and the resume replay attach to every card.
            activity.input_preview = Some(arguments.to_string());
            activity.card_body = luxe_core::tool_card_body(name, arguments, "/project");
            activity.run = runs.get(*id).copied();
            ToolItem { activity, expanded: false, details_toggled: false, waited_on_user_ms: 0 }
        })
        .collect()
}

/// Marks a card the way the call ending does.
pub(super) fn settle(item: &mut ToolItem, state: luxe_core::ToolActivityState, summary: &str) {
    item.activity.state = state;
    item.activity.summary = Some(summary.to_string());
    item.activity.output_preview = Some("a wall of matches".into());
}

pub(super) fn folded(app: &App) -> Vec<std::ops::Range<usize>> {
    app.tool_run_folds().into_iter().map(|fold| fold.range).collect()
}

/// A folded run has to form from the cards the *runtime* actually builds.
///
/// It never did. The rule asked for empty input and output previews, and the live projector fills
/// in the call's arguments while the resume path fills in the result — so every real card failed a
/// test that only a hand-built one could pass, and a run has not folded outside a fixture since the
/// feature landed. What the rule means is "this card draws nothing under its header", which is a
/// question about the card, not about which data reached it.
#[test]
fn a_run_of_real_lookups_folds() {
    let (tx, _rx) = mpsc::channel(100);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    let reads = projected_message(&[
        ("c1", "read", serde_json::json!({ "path": "a.rs" })),
        ("c2", "read", serde_json::json!({ "path": "b.rs" })),
        ("c3", "read", serde_json::json!({ "path": "c.rs" })),
    ]);
    for mut read in reads {
        settle(&mut read, luxe_core::ToolActivityState::Succeeded, "Completed");
        app.push_convo(ConvoBlock::Tool(Box::new(read)));
    }
    assert_eq!(folded(&app), vec![0..3], "three real reads fold into one line");

    // The tools a reader is supervising never fold: which command ran and what an edit changed are
    // the things the transcript exists to show.
    let (tx, _rx) = mpsc::channel(100);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    let supervised = projected_message(&[
        ("c1", "bash", serde_json::json!({ "command": "rm -rf build" })),
        ("c2", "bash", serde_json::json!({ "command": "git push --force" })),
        ("c3", "edit", serde_json::json!({ "path": "a.rs" })),
        ("c4", "todo", serde_json::json!({ "action": "complete", "id": 1 })),
    ]);
    for mut call in supervised {
        settle(&mut call, luxe_core::ToolActivityState::Succeeded, "Completed");
        app.push_convo(ConvoBlock::Tool(Box::new(call)));
    }
    assert!(folded(&app).is_empty(), "supervised work stays on its own card");
}

/// One `mcp` call per message, each to the same server: the agent working through a task list.
pub(super) fn mcp_call(app: &mut App, id: &str, arguments: serde_json::Value) {
    let mut call = projected_message(&[(id, "mcp", arguments)]).remove(0);
    settle(&mut call, luxe_core::ToolActivityState::Succeeded, "Completed");
    app.push_convo(ConvoBlock::Tool(Box::new(call)));
}

/// MCP calls arrive a message at a time, so their run is seen forming rather than promised: the
/// third call in a row to one server folds the run, and the run keeps growing while calls do.
#[test]
fn a_row_of_calls_to_one_mcp_server_folds_as_it_forms() {
    let (tx, _rx) = mpsc::channel(100);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    let call = |tool: &str| serde_json::json!({ "tool": tool, "args": "{}" });
    mcp_call(&mut app, "m1", serde_json::json!({ "search": "asana" }));
    mcp_call(&mut app, "m2", call("asana/get_tasks"));
    assert!(folded(&app).is_empty(), "two calls are not a wall");
    // A message that said nothing between two calls draws nothing, so it keeps them adjacent
    app.push_convo(ConvoBlock::Assistant("\n\n".into()));
    mcp_call(&mut app, "m3", call("asana/get_task"));
    assert_eq!(folded(&app), vec![0..4], "the search joins: it found the tool");
    mcp_call(&mut app, "m4", call("asana/update_task"));
    assert_eq!(folded(&app), vec![0..5]);
    assert_eq!(
        app.tool_run_folds(),
        vec![ToolRunFold { range: 0..5, arriving: false }],
        "nothing is promised about what comes next"
    );

    // Opening it is how a reader reviews each call, and it stays open as the run grows
    app.update(Msg::ToggleConversationBlockDetails(0));
    mcp_call(&mut app, "m5", call("asana/get_task"));
    assert!(app.tool_group_is_expanded(&(0..6)), "keyed by the call it starts at");
}

/// The server is the part of an `mcp` call's label before its slash, when both halves are a plain
/// name; a search names none.
#[test]
fn an_mcp_label_names_its_server() {
    use super::conversation::mcp_server;
    assert_eq!(mcp_server("asana/get_tasks"), Some("asana"));
    for label in ["search tasks", "search a/b", "a/b c", "/get_tasks", "asana/", ""] {
        assert_eq!(mcp_server(label), None, "{label:?}");
    }
}

/// Anything a reader would see between two calls ends the run: words, another server, a failure.
#[test]
fn whatever_a_reader_sees_between_mcp_calls_breaks_the_run() {
    let call = |tool: &str| serde_json::json!({ "tool": tool, "args": "{}" });
    let (tx, _rx) = mpsc::channel(100);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    mcp_call(&mut app, "a1", call("asana/get_tasks"));
    mcp_call(&mut app, "a2", call("asana/get_task"));
    app.push_convo(ConvoBlock::Assistant("Found it. Updating the due date now.".into()));
    mcp_call(&mut app, "a3", call("asana/update_task"));
    assert!(folded(&app).is_empty(), "prose between them is part of the conversation");

    let (tx, _rx) = mpsc::channel(100);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    mcp_call(&mut app, "b1", call("asana/get_tasks"));
    mcp_call(&mut app, "b2", call("github/list_issues"));
    mcp_call(&mut app, "b3", call("asana/get_task"));
    assert!(folded(&app).is_empty(), "two servers are two stories");

    let (tx, _rx) = mpsc::channel(100);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    mcp_call(&mut app, "c1", call("asana/get_tasks"));
    mcp_call(&mut app, "c2", call("asana/get_task"));
    let mut failed = projected_message(&[("c3", "mcp", call("asana/update_task"))]).remove(0);
    settle(&mut failed, luxe_core::ToolActivityState::Failed, "403 forbidden");
    app.push_convo(ConvoBlock::Tool(Box::new(failed)));
    mcp_call(&mut app, "c4", call("asana/get_task"));
    assert!(folded(&app).is_empty(), "a failure is the one card with something to say");

    // And the one switch every run answers to
    let config = Config {
        conversation: luxe_core::config::ConversationConfig {
            tool_runs: Some(luxe_core::config::ToolRuns::Cards),
            ..Default::default()
        },
        ..Config::default()
    };
    let (tx, _rx) = mpsc::channel(100);
    let mut app = App::new(tx, KeyMap::defaults(), config);
    for id in ["d1", "d2", "d3"] {
        mcp_call(&mut app, id, call("asana/get_task"));
    }
    assert!(folded(&app).is_empty(), "`tool_runs: cards` keeps every card");
}

/// The run is one line from its *first* card, not from its third.
///
/// Folding once three cards had landed meant taking back the rows the first two already had, and
/// the transcript dropped four lines under whatever the reader was in the middle of. The message
/// says how long its run will be before any of it is dispatched, so there is nothing to take back.
#[test]
fn a_run_is_one_line_from_its_first_card() {
    let (tx, _rx) = mpsc::channel(100);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    let calls = (1..=4)
        .map(|n| (format!("c{n}"), "read", serde_json::json!({ "path": format!("{n}.rs") })))
        .collect::<Vec<_>>();
    let borrowed =
        calls.iter().map(|(id, name, args)| (id.as_str(), *name, args.clone())).collect::<Vec<_>>();

    for (landed, mut card) in projected_message(&borrowed).into_iter().enumerate() {
        let arriving = landed + 1 < 4;
        app.push_convo(ConvoBlock::Tool(Box::new(card.clone())));
        assert_eq!(
            app.tool_run_folds(),
            vec![ToolRunFold { range: 0..landed + 1, arriving }],
            "the call in flight is the whole run's line"
        );

        settle(&mut card, luxe_core::ToolActivityState::Succeeded, "Completed");
        let last = app.convo.len() - 1;
        app.convo[last].set_block(ConvoBlock::Tool(Box::new(card)));
        assert_eq!(
            app.tool_run_folds(),
            vec![ToolRunFold { range: 0..landed + 1, arriving }],
            "a run between two of its calls has not settled"
        );
    }
}

/// A run assembled out of separate messages is not a run.
///
/// Three lookups one message asked for arrive in one breath; three that took a round-trip each
/// arrived at conversational speed, and folding them away would be reclaiming rows from a reader
/// who was keeping up fine — seconds later, which is the jarring part.
#[test]
fn lookups_from_separate_messages_keep_their_cards() {
    let (tx, _rx) = mpsc::channel(100);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    for call in ["c1", "c2", "c3"] {
        let mut card =
            projected_message(&[(call, "read", serde_json::json!({ "path": "a.rs" }))]).remove(0);
        settle(&mut card, luxe_core::ToolActivityState::Succeeded, "Completed");
        app.push_convo(ConvoBlock::Tool(Box::new(card)));
    }
    assert!(folded(&app).is_empty(), "one call per message is one card per message");

    // And two messages that each asked for a run stay two runs, rather than merging into a count
    // that describes neither.
    let (tx, _rx) = mpsc::channel(100);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    for message in 0..2 {
        let calls = (0..3)
            .map(|n| (format!("m{message}c{n}"), "read", serde_json::json!({ "path": "a.rs" })))
            .collect::<Vec<_>>();
        let borrowed = calls
            .iter()
            .map(|(id, name, args)| (id.as_str(), *name, args.clone()))
            .collect::<Vec<_>>();
        for mut card in projected_message(&borrowed) {
            settle(&mut card, luxe_core::ToolActivityState::Succeeded, "Completed");
            app.push_convo(ConvoBlock::Tool(Box::new(card)));
        }
    }
    assert_eq!(folded(&app), vec![0..3, 3..6]);
}

/// Turning folding off leaves every call on the card it would have had.
#[test]
fn tool_runs_cards_never_folds() {
    let config = Config {
        conversation: luxe_core::config::ConversationConfig {
            tool_runs: Some(luxe_core::config::ToolRuns::Cards),
            ..Default::default()
        },
        ..Default::default()
    };
    let (tx, _rx) = mpsc::channel(100);
    let mut app = App::new(tx, KeyMap::defaults(), config);
    let calls = [
        ("c1", "read", serde_json::json!({ "path": "a.rs" })),
        ("c2", "read", serde_json::json!({ "path": "b.rs" })),
        ("c3", "read", serde_json::json!({ "path": "c.rs" })),
    ];
    for mut card in projected_message(&calls) {
        settle(&mut card, luxe_core::ToolActivityState::Succeeded, "Completed");
        app.push_convo(ConvoBlock::Tool(Box::new(card)));
    }
    assert!(folded(&app).is_empty(), "the run was projected; the reader asked for cards");
}

/// A run folds while it is *running*, or it could only ever form once the reader no longer needed
/// it — and a failure always breaks out, because it is the one card in a run with something to say.
#[test]
fn a_run_folds_live_and_a_failure_leaves_it() {
    let grep =
        |n: usize| (format!("c{n}"), "grep", serde_json::json!({ "pattern": "needle", "n": n }));
    let calls = (1..=6).map(grep).collect::<Vec<_>>();
    let borrowed =
        calls.iter().map(|(id, name, args)| (id.as_str(), *name, args.clone())).collect::<Vec<_>>();
    let mut cards = projected_message(&borrowed);
    for card in cards.iter_mut().take(2) {
        settle(card, luxe_core::ToolActivityState::Succeeded, "Completed");
    }

    let (tx, _rx) = mpsc::channel(100);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    for card in cards.iter().take(3) {
        app.push_convo(ConvoBlock::Tool(Box::new(card.clone())));
    }
    assert_eq!(folded(&app), vec![0..3], "the call in flight is part of the run");

    // A run that is still working grows, so what an opened run is remembered by cannot be its
    // membership: the reader clicks it open and the next call to join snaps it shut again.
    app.selected_block = Some(1);
    assert!(app.toggle_selected_tool_details());
    assert!(app.tool_group_is_expanded(&(0..3)));
    app.push_convo(ConvoBlock::Tool(Box::new(cards[3].clone())));
    assert!(app.tool_group_is_expanded(&(0..4)), "a joining call must not close what was opened");
    app.selected_block = Some(1);
    assert!(app.toggle_selected_tool_details());
    assert!(!app.tool_group_is_expanded(&(0..4)), "and it still closes on a second click");

    settle(&mut cards[4], luxe_core::ToolActivityState::Failed, "no such file");
    app.push_convo(ConvoBlock::Tool(Box::new(cards[4].clone())));
    app.push_convo(ConvoBlock::Tool(Box::new(cards[5].clone())));
    assert_eq!(folded(&app), vec![0..4], "a failure is not folded away");
}

/// A failure in the middle of a run leaves two runs and a card, each still what it says it is.
#[test]
fn a_failure_stands_between_the_runs_it_broke() {
    let calls = (1..=7)
        .map(|n| (format!("c{n}"), "read", serde_json::json!({ "path": format!("{n}.rs") })))
        .collect::<Vec<_>>();
    let borrowed =
        calls.iter().map(|(id, name, args)| (id.as_str(), *name, args.clone())).collect::<Vec<_>>();

    let (tx, _rx) = mpsc::channel(100);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    for (index, mut card) in projected_message(&borrowed).into_iter().enumerate() {
        match index {
            3 => settle(&mut card, luxe_core::ToolActivityState::Failed, "no such file"),
            _ => settle(&mut card, luxe_core::ToolActivityState::Succeeded, "Completed"),
        }
        app.push_convo(ConvoBlock::Tool(Box::new(card)));
    }

    assert_eq!(folded(&app), vec![0..3, 4..7]);
    app.selected_block = Some(1);
    assert!(app.toggle_selected_tool_details());
    assert!(app.tool_group_is_expanded(&(0..3)));
    assert!(app.toggle_selected_tool_details());
    assert!(!app.tool_group_is_expanded(&(0..3)));
}

#[tokio::test]
async fn selected_tool_enter_and_space_toggle_details_without_editing() {
    let (tx, _rx) = mpsc::channel(100);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    app.session_id = Some("session-a".into());
    app.update(Msg::Engine(Event::ToolActivityUpdated(Box::new(ToolActivity::running(
        "session-a",
        "call-a",
        "read",
        "Cargo.toml",
        1,
    )))));
    app.selected_block = Some(0);

    app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
    assert!(app.tool_items.get(&("session-a".into(), "call-a".into())).unwrap().expanded);
    assert!(matches!(&app.convo[0].block(), ConvoBlock::Tool(item) if item.expanded));
    assert!(app.editor.current_input().is_empty());

    app.update(Msg::Key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE)));
    assert!(!app.tool_items.get(&("session-a".into(), "call-a".into())).unwrap().expanded);
    assert!(matches!(&app.convo[0].block(), ConvoBlock::Tool(item) if !item.expanded));
    assert!(app.editor.current_input().is_empty());
}

#[tokio::test]
async fn tool_activity_rejects_stale_and_post_terminal_updates() {
    let (tx, _rx) = mpsc::channel(100);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    app.session_id = Some("session-a".into());
    let running = ToolActivity::running("session-a", "call-a", "write", "Write file", 1);
    let mut succeeded = running.clone();
    succeeded.revision = 1;
    succeeded.state = luxe_core::ToolActivityState::Succeeded;
    let mut invalid_progress = succeeded.clone();
    invalid_progress.revision = 2;
    invalid_progress.state = luxe_core::ToolActivityState::Running;

    app.update(Msg::Engine(Event::ToolActivityUpdated(Box::new(running))));
    app.update(Msg::Engine(Event::ToolActivityUpdated(Box::new(succeeded))));
    app.update(Msg::Engine(Event::ToolActivityUpdated(Box::new(invalid_progress))));

    let activity = app.tool_activity.get(&("session-a".into(), "call-a".into())).unwrap();
    assert_eq!(activity.revision, 1);
    assert!(activity.is_terminal());
    let item = app.tool_items.get(&("session-a".into(), "call-a".into())).unwrap();
    assert_eq!(item.activity.revision, 1);
    assert!(!item.expanded);
    assert!(matches!(
        transcript_shape(&app).as_slice(),
        [(
            ConversationItemId::Tool { session_id, call_id },
            ConvoBlock::Tool(item),
        )] if session_id == "session-a"
            && call_id == "call-a"
            && item.activity.revision == 1
            && item.activity.is_terminal()
    ));
}

#[tokio::test]
async fn configured_tool_detail_defaults_apply_when_activity_first_creates_a_row() {
    for (preference, state, expected_expanded) in [
        (
            luxe_core::config::ToolDetailsPreference::All,
            luxe_core::ToolActivityState::Running,
            true,
        ),
        (
            luxe_core::config::ToolDetailsPreference::Errors,
            luxe_core::ToolActivityState::Failed,
            true,
        ),
        (
            luxe_core::config::ToolDetailsPreference::Collapsed,
            luxe_core::ToolActivityState::Failed,
            false,
        ),
    ] {
        let (tx, _rx) = mpsc::channel(100);
        let mut app = App::new(
            tx,
            KeyMap::defaults(),
            Config {
                conversation: luxe_core::config::ConversationConfig {
                    tool_details: Some(preference),
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        app.session_id = Some("session-a".into());
        let mut activity = ToolActivity::running("session-a", "call-a", "read", "Read file", 1);
        activity.state = state;

        app.update(Msg::Engine(Event::ToolActivityUpdated(Box::new(activity))));

        assert_eq!(
            app.tool_items.get(&("session-a".into(), "call-a".into())).unwrap().expanded,
            expected_expanded
        );
    }
}

#[tokio::test]
async fn interleaved_tool_activity_updates_keep_each_call_in_place() {
    let (tx, _rx) = mpsc::channel(100);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    app.session_id = Some("session-a".into());
    let first = ToolActivity::running("session-a", "call-a", "read", "Read Cargo.toml", 1);
    let second = ToolActivity::running("session-a", "call-b", "bash", "cargo test", 2);
    let mut second_done = second.clone();
    second_done.revision = 1;
    second_done.state = luxe_core::ToolActivityState::Succeeded;
    let mut first_done = first.clone();
    first_done.revision = 1;
    first_done.state = luxe_core::ToolActivityState::Failed;

    app.update(Msg::Engine(Event::ToolActivityUpdated(Box::new(first))));
    app.update(Msg::Engine(Event::ToolActivityUpdated(Box::new(second))));
    app.update(Msg::Engine(Event::ToolActivityUpdated(Box::new(second_done))));
    app.update(Msg::Engine(Event::ToolActivityUpdated(Box::new(first_done))));

    assert!(matches!(
        transcript_shape(&app).as_slice(),
        [
            (ConversationItemId::Tool { call_id: first_id, .. }, ConvoBlock::Tool(first_item)),
            (ConversationItemId::Tool { call_id: second_id, .. }, ConvoBlock::Tool(second_item)),
        ] if first_id == "call-a"
            && first_item.activity.state == luxe_core::ToolActivityState::Failed
            && second_id == "call-b"
            && second_item.activity.state == luxe_core::ToolActivityState::Succeeded
    ));
}

#[tokio::test]
async fn tool_events_update_active_status() {
    let (tx, _rx) = mpsc::channel(8);
    let mut app =
        App::new(tx, luxe_core::keymap::KeyMap::defaults(), luxe_core::config::Config::default());
    app.update(Msg::Engine(Event::ToolStarted {
        call_id: "call-1".into(),
        name: "web_search".into(),
    }));
    assert_eq!(app.status.active_tool.as_deref(), Some("web_search"));
    assert!(app.status.turn_started.is_some());

    app.update(Msg::Engine(Event::ToolEnded { call_id: "call-1".into(), is_error: false }));
    assert!(app.status.active_tool.is_none());
}

/// A card opened while the model was writing a call is the one thing a turn can leave spinning:
/// every *dispatched* call is answered, but a stream that fails — or a cancel that lands before
/// dispatch — ends the turn with that call never reaching the engine. A spinner nothing will come
/// back for reads as a hang.
#[tokio::test]
async fn a_turn_that_ends_settles_the_cards_it_left_spinning() {
    let ended = Event::TurnEnded { reason: luxe_core::StopReason::Other("cancelled".into()) };
    let failed = Event::Error("the provider hung up".into());
    for (ending, why) in [(ended, "The turn ended"), (failed, "The turn failed")] {
        let (tx, _rx) = mpsc::channel(8);
        let mut app = App::new(
            tx,
            luxe_core::keymap::KeyMap::defaults(),
            luxe_core::config::Config::default(),
        );
        app.session_id = Some("session-a".into());
        app.update(Msg::Engine(Event::ToolActivityUpdated(Box::new(ToolActivity::running(
            "session-a",
            "call-a",
            "write",
            "notes.txt",
            1,
        )))));
        assert!(
            matches!(app.convo[0].block(), ConvoBlock::Tool(item) if !item.activity.is_terminal())
        );

        app.update(Msg::Engine(ending.clone()));
        let ConvoBlock::Tool(item) = app.convo[0].block() else {
            panic!("the card is still there")
        };
        assert_eq!(item.activity.state, luxe_core::ToolActivityState::Cancelled);
        assert!(item.activity.summary.as_ref().is_some_and(|said| said.starts_with(why)));

        // A settled card is history; a second ending must not revise it again.
        let revision = item.activity.revision;
        app.update(Msg::Engine(ending));
        let ConvoBlock::Tool(item) = app.convo[0].block() else {
            panic!("the card is still there")
        };
        assert_eq!(item.activity.revision, revision);
    }
}

/// An app with one running card, the way a turn looks while its call runs.
pub(super) fn app_running_a_call(tx: mpsc::Sender<luxe_core::engine::Action>) -> App {
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    app.update(Msg::Engine(Event::SessionLoaded {
        session_id: "session-a".into(),
        messages: Vec::new(),
    }));
    app.update(Msg::Engine(Event::ToolStarted { call_id: "call-1".into(), name: "git".into() }));
    app.update(Msg::Engine(Event::ToolActivityUpdated(Box::new(ToolActivity::running(
        "session-a",
        "call-1",
        "git",
        "commit",
        1,
    )))));
    app
}

pub(super) fn card_state(app: &App) -> luxe_core::ToolActivityState {
    app.tool_activity
        .values()
        .find(|activity| activity.call_id == "call-1")
        .expect("the card")
        .state
}

/// An interrupt the engine never confirms stops the transcript waiting for it: the card settles as
/// interrupted rather than sitting on a spinner frame with its clock running for the rest of the
/// session, and the transcript says what happened.
#[tokio::test]
async fn an_unconfirmed_interrupt_settles_the_cards_it_left_running() {
    let (tx, _rx) = mpsc::channel(8);
    let mut app = app_running_a_call(tx);
    let base = std::time::Instant::now();
    app.status.frame_clock = Some(base);
    app.dispatch_action(UiAction::Cancel);
    assert!(app.interrupt_sent_at.is_some(), "a turn was under way, so the stop is awaited");
    assert!(app.animating(), "frames keep coming while the stop is awaited");

    app.status.frame_clock = Some(base + std::time::Duration::from_secs(1));
    app.update(Msg::Tick);
    assert_eq!(card_state(&app), luxe_core::ToolActivityState::Running, "still within reason");

    let items = app.convo.len();
    app.status.frame_clock = Some(base + super::events::INTERRUPT_CONFIRM);
    app.update(Msg::Tick);
    assert_eq!(card_state(&app), luxe_core::ToolActivityState::Cancelled);
    assert!(app.interrupt_sent_at.is_none());
    assert_eq!(app.convo.len(), items + 1, "and the transcript says so");
    assert!(matches!(
        app.convo.last().map(ConversationItem::block),
        Some(ConvoBlock::System(text)) if text.contains("did not confirm")
    ));
}

/// The engine ending the turn is the confirmation, and then nothing is said about it.
#[tokio::test]
async fn a_confirmed_interrupt_is_not_reported() {
    let (tx, _rx) = mpsc::channel(8);
    let mut app = app_running_a_call(tx);
    let base = std::time::Instant::now();
    app.status.frame_clock = Some(base);
    app.dispatch_action(UiAction::Cancel);
    app.update(Msg::Engine(Event::TurnEnded {
        reason: luxe_core::event::StopReason::Other("cancelled".into()),
    }));
    let items = app.convo.len();
    app.status.frame_clock = Some(base + super::events::INTERRUPT_CONFIRM * 2);
    app.update(Msg::Tick);
    assert!(app.interrupt_sent_at.is_none());
    assert_eq!(app.convo.len(), items, "nothing to report");
}

/// A settled card takes a newer revision that keeps it settled — a call that ran before its
/// message committed learns its full label afterwards — and never goes back to running.
#[tokio::test]
async fn a_settled_card_takes_its_label_but_never_runs_again() {
    let (tx, _rx) = mpsc::channel(100);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    app.session_id = Some("session-a".into());
    let mut done = ToolActivity::running("session-a", "call-a", "test", "test", 1);
    done.revision = 2;
    done.state = luxe_core::ToolActivityState::Succeeded;
    app.update(Msg::Engine(Event::ToolActivityUpdated(Box::new(done.clone()))));

    let mut labeled = done.clone();
    labeled.revision = 3;
    labeled.label = "project tests".into();
    app.update(Msg::Engine(Event::ToolActivityUpdated(Box::new(labeled))));
    let key = ("session-a".to_string(), "call-a".to_string());
    assert_eq!(app.tool_activity[&key].label, "project tests");

    let mut revived = done;
    revived.revision = 4;
    revived.state = luxe_core::ToolActivityState::Running;
    app.update(Msg::Engine(Event::ToolActivityUpdated(Box::new(revived))));
    assert_eq!(
        app.tool_activity[&key].state,
        luxe_core::ToolActivityState::Succeeded,
        "settled stays settled"
    );
}
