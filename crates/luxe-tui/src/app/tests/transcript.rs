//! Transcript — what the conversation holds, how items are identified, streamed and replayed.

use super::*;

/// The transcript as `(identity, content)` pairs, which is the shape a `matches!` can destructure
/// now that an item's block is only reachable through an accessor.
pub(super) fn transcript_shape(app: &App) -> Vec<(&ConversationItemId, &ConvoBlock)> {
    app.convo.iter().map(|item| (&item.id, item.block())).collect()
}

#[test]
fn conversation_item_identity_scopes_persisted_and_live_content() {
    let persisted = ConversationItemId::Persisted {
        session_id: "session-a".into(),
        entry_id: "entry-1".into(),
        block_index: 0,
    };
    let same_entry_next_block = ConversationItemId::Persisted {
        session_id: "session-a".into(),
        entry_id: "entry-1".into(),
        block_index: 1,
    };
    let matching_live_block = ConversationItemId::Provisional {
        session_id: "session-a".into(),
        turn_id: 1,
        block_index: 0,
    };
    let other_session_live_block = ConversationItemId::Provisional {
        session_id: "session-b".into(),
        turn_id: 1,
        block_index: 0,
    };
    let tool =
        ConversationItemId::Tool { session_id: "session-a".into(), call_id: "call-1".into() };
    let reused_call_in_other_session =
        ConversationItemId::Tool { session_id: "session-b".into(), call_id: "call-1".into() };

    assert_ne!(persisted, same_entry_next_block);
    assert_ne!(persisted, matching_live_block);
    assert_ne!(matching_live_block, other_session_live_block);
    assert_ne!(tool, reused_call_in_other_session);
}

#[test]
fn conversation_anchor_is_stable_across_flattened_line_offsets() {
    let anchor = ConversationAnchor {
        item_id: ConversationItemId::Persisted {
            session_id: "session-a".into(),
            entry_id: "entry-1".into(),
            block_index: 2,
        },
        visual_line_within_item: 3,
        item_index: 2,
        screen_row: 5,
        scroll_offset: 9,
    };
    let same_semantic_position = anchor.clone();

    assert_eq!(anchor, same_semantic_position);
    assert_eq!(anchor.visual_line_within_item, 3);
    assert_eq!(anchor.screen_row, 5);
}

#[tokio::test]
async fn test_app_new() {
    let (tx, _rx) = mpsc::channel(100);
    let keymap = luxe_core::keymap::KeyMap::defaults();
    let config = luxe_core::config::Config::default();
    let app = App::new(tx, keymap, config);
    assert!(app.convo.is_empty());
    assert!(!app.should_quit);
    assert_eq!(app.dock, crate::workspace_dock::DockState::default());
    assert_eq!(app.workspace_layout, WorkspaceLayout::Standard);
}

#[tokio::test]
async fn errors_default_opens_untouched_failures_but_preserves_manual_collapse() {
    let (tx, _rx) = mpsc::channel(100);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    app.session_id = Some("session-a".into());
    let running = ToolActivity::running("session-a", "call-a", "edit", "Edit file", 1);
    let mut failed = running.clone();
    failed.revision = 1;
    failed.state = luxe_core::ToolActivityState::Failed;

    app.update(Msg::Engine(Event::ToolActivityUpdated(Box::new(running.clone()))));
    app.update(Msg::Engine(Event::ToolActivityUpdated(Box::new(failed.clone()))));
    assert!(app.tool_items.get(&("session-a".into(), "call-a".into())).unwrap().expanded);

    let (tx, _rx) = mpsc::channel(100);
    let mut manually_collapsed = App::new(tx, KeyMap::defaults(), Config::default());
    manually_collapsed.session_id = Some("session-a".into());
    let mut second_running = running;
    second_running.call_id = "call-b".into();
    let mut second_failed = failed;
    second_failed.call_id = "call-b".into();
    manually_collapsed.update(Msg::Engine(Event::ToolActivityUpdated(Box::new(second_running))));
    manually_collapsed.selected_block = Some(0);
    assert!(manually_collapsed.toggle_selected_tool_details());
    assert!(manually_collapsed.toggle_selected_tool_details());
    manually_collapsed.update(Msg::Engine(Event::ToolActivityUpdated(Box::new(second_failed))));
    let item = manually_collapsed.tool_items.get(&("session-a".into(), "call-b".into())).unwrap();
    assert!(!item.expanded);
    assert!(item.details_toggled);
}

#[test]
fn conversation_selection_wraps_and_copies_selected_text() {
    let (tx, _rx) = mpsc::channel(8);
    let mut app =
        App::new(tx, luxe_core::keymap::KeyMap::defaults(), luxe_core::config::Config::default());
    app.push_convo(ConvoBlock::User {
        text: "first".into(),
        attached: Vec::new(),
        chips: Vec::new(),
    });
    app.push_convo(ConvoBlock::Assistant("second".into()));

    app.select_block_up();
    assert_eq!(app.selected_block, Some(1));
    app.select_block_down();
    assert_eq!(app.selected_block, Some(0));
    assert!(app.copy_selected_block());
    assert_eq!(app.outbox.copy.as_deref(), Some("> first\n"));
}

#[test]
fn committed_user_and_system_items_use_persisted_identity() {
    let (tx, _rx) = mpsc::channel(8);
    let mut app =
        App::new(tx, luxe_core::keymap::KeyMap::defaults(), luxe_core::config::Config::default());
    app.update(Msg::Engine(Event::SessionLoaded {
        session_id: "session-a".into(),
        messages: Vec::new(),
    }));
    for (entry_id, role, text) in [
        ("user-entry", luxe_core::message::Role::User, "prompt"),
        ("system-entry", luxe_core::message::Role::System, "notice"),
    ] {
        app.update(Msg::Engine(Event::MessageCommitted {
            session_id: "session-a".into(),
            entry_id: luxe_core::session::EntryId(entry_id.into()),
            message: luxe_core::message::Message {
                role,
                content: vec![luxe_core::message::ContentBlock::Text(text.into())],
            },
        }));
    }

    assert_eq!(
        app.convo.iter().map(|item| item.id.clone()).collect::<Vec<_>>(),
        vec![
            ConversationItemId::Persisted {
                session_id: "session-a".into(),
                entry_id: "user-entry".into(),
                block_index: 0,
            },
            ConversationItemId::Persisted {
                session_id: "session-a".into(),
                entry_id: "system-entry".into(),
                block_index: 0,
            },
        ]
    );
}

#[test]
fn persisted_text_blocks_keep_content_block_identity_live_and_on_replay() {
    let message = luxe_core::message::Message {
        role: luxe_core::message::Role::Assistant,
        content: vec![
            luxe_core::message::ContentBlock::Text("first block".into()),
            luxe_core::message::ContentBlock::Text("second block".into()),
        ],
    };
    let expected_ids = vec![
        ConversationItemId::Persisted {
            session_id: "session-a".into(),
            entry_id: "entry-1".into(),
            block_index: 0,
        },
        ConversationItemId::Persisted {
            session_id: "session-a".into(),
            entry_id: "entry-1".into(),
            block_index: 1,
        },
    ];

    let (tx, _rx) = mpsc::channel(8);
    let mut live = App::new(tx, KeyMap::defaults(), Config::default());
    live.update(Msg::Engine(Event::SessionLoaded {
        session_id: "session-a".into(),
        messages: Vec::new(),
    }));
    live.update(Msg::Engine(Event::MessageCommitted {
        session_id: "session-a".into(),
        entry_id: luxe_core::session::EntryId("entry-1".into()),
        message: message.clone(),
    }));
    assert_eq!(live.convo.iter().map(|item| item.id.clone()).collect::<Vec<_>>(), expected_ids);

    let (tx, _rx) = mpsc::channel(8);
    let mut replay = App::new(tx, KeyMap::defaults(), Config::default());
    replay.update(Msg::Engine(Event::SessionLoaded {
        session_id: "session-a".into(),
        messages: vec![luxe_core::event::SessionMessage {
            entry_id: luxe_core::session::EntryId("entry-1".into()),
            message,
        }],
    }));
    assert_eq!(replay.convo.iter().map(|item| item.id.clone()).collect::<Vec<_>>(), expected_ids);
}

#[test]
fn late_commit_from_replaced_session_is_ignored() {
    let (tx, _rx) = mpsc::channel(8);
    let mut app =
        App::new(tx, luxe_core::keymap::KeyMap::defaults(), luxe_core::config::Config::default());
    app.update(Msg::Engine(Event::SessionLoaded {
        session_id: "session-a".into(),
        messages: Vec::new(),
    }));
    app.update(Msg::Engine(Event::TokenDelta { block: 0, delta: "stale".into() }));
    app.update(Msg::Engine(Event::SessionLoaded {
        session_id: "session-b".into(),
        messages: Vec::new(),
    }));
    app.update(Msg::Engine(Event::MessageCommitted {
        session_id: "session-a".into(),
        entry_id: luxe_core::session::EntryId("entry-a".into()),
        message: luxe_core::message::Message::assistant("stale"),
    }));

    assert!(app.convo.is_empty());
    assert!(!app.streaming);
}

/// A streamed reply renders live and is finalized once committed (no duplicate block).
#[tokio::test]
async fn test_streaming_then_commit_single_block() {
    let (tx, _rx) = mpsc::channel(8);
    let keymap = luxe_core::keymap::KeyMap::defaults();
    let config = luxe_core::config::Config::default();
    let mut app = App::new(tx, keymap, config);
    app.update(Msg::Engine(Event::SessionLoaded {
        session_id: "session-a".into(),
        messages: Vec::new(),
    }));
    app.update(Msg::Engine(Event::TokenDelta { block: 0, delta: "Hel".into() }));
    app.update(Msg::Engine(Event::TokenDelta { block: 0, delta: "lo".into() }));
    let provisional_id = app.convo.last().unwrap().id.clone();
    let committed = luxe_core::message::Message {
        role: luxe_core::message::Role::Assistant,
        content: vec![luxe_core::message::ContentBlock::Text("Hello".into())],
    };
    app.update(Msg::Engine(Event::MessageCommitted {
        session_id: "session-a".into(),
        entry_id: luxe_core::session::EntryId("entry-1".into()),
        message: committed,
    }));

    let assistant_blocks =
        app.convo.iter().filter(|item| matches!(item.block(), ConvoBlock::Assistant(_))).count();
    assert_eq!(assistant_blocks, 1, "streaming + commit must not duplicate the reply");
    assert!(
        matches!(app.convo.last().map(ConversationItem::block), Some(ConvoBlock::Assistant(t)) if t == "Hello")
    );
    assert_ne!(app.convo.last().unwrap().id, provisional_id);
    assert!(matches!(
        app.convo.last().unwrap().id,
        ConversationItemId::Persisted {
            ref session_id,
            ref entry_id,
            block_index: 0,
        } if session_id == "session-a" && entry_id == "entry-1"
    ));
}

/// The card for a call the model is still writing sits at the tail of the transcript when the
/// message commits, and the commit has to reconcile *around* it.
///
/// It did not. Lining the streamed blocks up against the last `n` items put the card inside the
/// window, nothing matched, and the fallback that drops provisional blocks stopped at the first
/// item that was not one — which was the card. So every reply that reasoned, spoke and then called
/// a tool appended its reasoning and its answer a second time, under the card. Measured against one
/// real session: 204 of 375 assistant messages.
#[tokio::test]
async fn a_composing_card_at_the_tail_does_not_duplicate_the_reply() {
    use luxe_core::message::ContentBlock;

    // Every shape a real session produced, with how many of them it produced.
    for content in [
        vec![
            ContentBlock::Thinking {
                text: "weighing it up".into(),
                signature: None,
                id: None,
                encrypted: None,
                opaque: None,
            },
            ContentBlock::Text("Writing the harness.".into()),
            ContentBlock::ToolCall {
                id: "call-1".into(),
                name: "write".into(),
                arguments: serde_json::json!({"path": "a.py", "content": "x\n"}),
            },
        ],
        vec![
            ContentBlock::Thinking {
                text: "weighing it up".into(),
                signature: None,
                id: None,
                encrypted: None,
                opaque: None,
            },
            ContentBlock::ToolCall {
                id: "call-1".into(),
                name: "write".into(),
                arguments: serde_json::json!({"path": "a.py", "content": "x\n"}),
            },
        ],
        vec![
            ContentBlock::Text("Writing the harness.".into()),
            ContentBlock::ToolCall {
                id: "call-1".into(),
                name: "write".into(),
                arguments: serde_json::json!({"path": "a.py", "content": "x\n"}),
            },
        ],
        vec![
            ContentBlock::Thinking {
                text: "weighing it up".into(),
                signature: None,
                id: None,
                encrypted: None,
                opaque: None,
            },
            ContentBlock::Text("Writing both.".into()),
            ContentBlock::ToolCall {
                id: "call-1".into(),
                name: "write".into(),
                arguments: serde_json::json!({"path": "a.py", "content": "x\n"}),
            },
            ContentBlock::ToolCall {
                id: "call-2".into(),
                name: "write".into(),
                arguments: serde_json::json!({"path": "b.py", "content": "y\n"}),
            },
        ],
    ] {
        let (tx, _rx) = mpsc::channel(8);
        let mut app = App::new(tx, KeyMap::defaults(), Config::default());
        app.update(Msg::Engine(Event::SessionLoaded {
            session_id: "session-a".into(),
            messages: Vec::new(),
        }));

        // The stream, in the order a provider really produces it.
        for (index, block) in content.iter().enumerate() {
            match block {
                ContentBlock::Thinking { text, .. } => {
                    app.update(Msg::Engine(Event::ThinkingDelta {
                        block: index,
                        delta: text.clone(),
                    }))
                }
                ContentBlock::Text(text) => {
                    app.update(Msg::Engine(Event::TokenDelta { block: index, delta: text.clone() }))
                }
                ContentBlock::ToolCall { id, name, .. } => {
                    app.update(Msg::Engine(Event::ToolActivityUpdated(Box::new(
                        ToolActivity::running("session-a", id.clone(), name.clone(), "", 1),
                    ))));
                }
                _ => {}
            }
        }

        let shape = |app: &App| {
            app.convo
                .iter()
                .map(|item| match item.block() {
                    ConvoBlock::Thinking { .. } => "thinking",
                    ConvoBlock::Assistant(_) => "text",
                    ConvoBlock::Tool(_) => "card",
                    _ => "other",
                })
                .collect::<Vec<_>>()
        };
        let streamed = shape(&app);

        app.update(Msg::Engine(Event::MessageCommitted {
            session_id: "session-a".into(),
            entry_id: luxe_core::session::EntryId("entry-1".into()),
            message: luxe_core::message::Message {
                role: luxe_core::message::Role::Assistant,
                content: content.clone(),
            },
        }));

        assert_eq!(shape(&app), streamed, "the commit added or moved something: {content:#?}");
        // And it really did finalize them, rather than leaving the stream's own provisional rows.
        assert!(
            app.convo.iter().all(|item| match item.block() {
                ConvoBlock::Thinking { .. } | ConvoBlock::Assistant(_) =>
                    matches!(item.id, ConversationItemId::Persisted { .. }),
                _ => true,
            }),
            "a provisional block survived the commit: {:?}",
            shape(&app),
        );
    }
}

/// Reasoning streams into its own block ahead of the answer, and a commit reconciles both
/// (no duplication) with the thinking block promoted to a persisted id.
#[tokio::test]
async fn test_thinking_streams_then_commits_before_answer() {
    use luxe_core::message::ContentBlock;
    let (tx, _rx) = mpsc::channel(8);
    let keymap = luxe_core::keymap::KeyMap::defaults();
    let config = luxe_core::config::Config::default();
    let mut app = App::new(tx, keymap, config);
    app.update(Msg::Engine(Event::SessionLoaded {
        session_id: "session-a".into(),
        messages: Vec::new(),
    }));
    app.update(Msg::Engine(Event::ThinkingDelta { block: 0, delta: "Let me ".into() }));
    app.update(Msg::Engine(Event::ThinkingDelta { block: 0, delta: "reason.".into() }));
    app.update(Msg::Engine(Event::TokenDelta { block: 0, delta: "Answer".into() }));

    // Live: a thinking block precedes the streaming answer.
    assert!(matches!(
        app.convo.first().map(|item| item.block()),
        Some(ConvoBlock::Thinking { text, expanded: true }) if text == "Let me reason."
    ));
    assert!(
        matches!(app.convo.last().map(|item| item.block()), Some(ConvoBlock::Assistant(t)) if t == "Answer")
    );

    let committed = luxe_core::message::Message {
        role: luxe_core::message::Role::Assistant,
        content: vec![
            ContentBlock::Thinking {
                text: "Let me reason.".into(),
                signature: None,
                id: None,
                encrypted: None,
                opaque: None,
            },
            ContentBlock::Text("Answer".into()),
        ],
    };
    app.update(Msg::Engine(Event::MessageCommitted {
        session_id: "session-a".into(),
        entry_id: luxe_core::session::EntryId("entry-1".into()),
        message: committed,
    }));

    let thinking =
        app.convo.iter().filter(|i| matches!(i.block(), ConvoBlock::Thinking { .. })).count();
    let assistant =
        app.convo.iter().filter(|i| matches!(i.block(), ConvoBlock::Assistant(_))).count();
    assert_eq!((thinking, assistant), (1, 1), "commit must not duplicate reasoning or the answer");
    assert!(matches!(
        app.convo.first().map(|item| (&item.id, item.block())),
        Some((ConversationItemId::Persisted { block_index: 0, .. }, ConvoBlock::Thinking { .. }))
    ));
    assert!(matches!(
        app.convo.get(1).map(|item| (&item.id, item.block())),
        Some((ConversationItemId::Persisted { block_index: 1, .. }, ConvoBlock::Assistant(_)))
    ));
}

/// `thinking = "hidden"` never renders reasoning, even though the committed message carries it.
#[tokio::test]
async fn test_thinking_hidden_config_drops_block() {
    use luxe_core::message::ContentBlock;
    let (tx, _rx) = mpsc::channel(8);
    let keymap = luxe_core::keymap::KeyMap::defaults();
    let mut config = luxe_core::config::Config::default();
    config.conversation.thinking = Some(luxe_core::config::ThinkingDisplay::Hidden);
    let mut app = App::new(tx, keymap, config);
    app.update(Msg::Engine(Event::ThinkingDelta { block: 0, delta: "hidden reasoning".into() }));
    assert!(!app.convo.iter().any(|i| matches!(i.block(), ConvoBlock::Thinking { .. })));

    app.update(Msg::Engine(Event::TokenDelta { block: 0, delta: "Answer".into() }));
    let committed = luxe_core::message::Message {
        role: luxe_core::message::Role::Assistant,
        content: vec![
            ContentBlock::Thinking {
                text: "hidden reasoning".into(),
                signature: None,
                id: None,
                encrypted: None,
                opaque: None,
            },
            ContentBlock::Text("Answer".into()),
        ],
    };
    app.update(Msg::Engine(Event::MessageCommitted {
        session_id: "test".into(),
        entry_id: luxe_core::session::EntryId("e1".into()),
        message: committed,
    }));
    assert!(!app.convo.iter().any(|i| matches!(i.block(), ConvoBlock::Thinking { .. })));
    assert_eq!(
        app.convo.iter().filter(|i| matches!(i.block(), ConvoBlock::Assistant(_))).count(),
        1
    );
}

/// `thinking = "collapsed"` opens streamed reasoning as a peek; the selection toggle expands it.
#[tokio::test]
async fn test_thinking_collapsed_default_and_toggle() {
    let (tx, _rx) = mpsc::channel(8);
    let keymap = luxe_core::keymap::KeyMap::defaults();
    let mut config = luxe_core::config::Config::default();
    config.conversation.thinking = Some(luxe_core::config::ThinkingDisplay::Collapsed);
    let mut app = App::new(tx, keymap, config);
    app.update(Msg::Engine(Event::ThinkingDelta { block: 0, delta: "reasoning".into() }));
    assert!(matches!(
        app.convo.first().map(|item| item.block()),
        Some(ConvoBlock::Thinking { expanded: false, .. })
    ));
    app.selected_block = Some(0);
    assert!(app.toggle_selected_tool_details(), "selecting + toggling expands reasoning");
    assert!(matches!(
        app.convo.first().map(|item| item.block()),
        Some(ConvoBlock::Thinking { expanded: true, .. })
    ));
}

/// Reloading a session replays persisted reasoning as a thinking block before the answer.
#[tokio::test]
async fn test_replay_renders_persisted_thinking() {
    use luxe_core::event::SessionMessage;
    use luxe_core::message::ContentBlock;
    let (tx, _rx) = mpsc::channel(8);
    let keymap = luxe_core::keymap::KeyMap::defaults();
    let config = luxe_core::config::Config::default();
    let mut app = App::new(tx, keymap, config);
    let message = luxe_core::message::Message {
        role: luxe_core::message::Role::Assistant,
        content: vec![
            ContentBlock::Thinking {
                text: "persisted thought".into(),
                signature: None,
                id: None,
                encrypted: None,
                opaque: None,
            },
            ContentBlock::Text("final".into()),
        ],
    };
    app.update(Msg::Engine(Event::SessionLoaded {
        session_id: "session-a".into(),
        messages: vec![SessionMessage {
            entry_id: luxe_core::session::EntryId("entry-1".into()),
            message,
        }],
    }));
    assert!(matches!(
        app.convo.first().map(|item| item.block()),
        Some(ConvoBlock::Thinking { text, .. }) if text == "persisted thought"
    ));
    assert!(
        matches!(app.convo.get(1).map(|item| item.block()), Some(ConvoBlock::Assistant(t)) if t == "final")
    );
}

// === StatusModel unit tests ===
#[test]
fn test_dispatch_action_search_convo() {
    let (tx, _rx) = mpsc::channel(100);
    let keymap = luxe_core::keymap::KeyMap::defaults();
    let config = luxe_core::config::Config::default();
    let mut app = App::new(tx, keymap, config);

    let result = app.dispatch_action(luxe_core::keymap::UiAction::SearchConvo);
    assert!(result, "SearchConvo should be handled");
    assert!(app.search_state.searching, "search should be active after SearchConvo");
    assert!(app.search_state.query.is_empty(), "query should start empty");
}

/// The repository this test runs in, whose files the fixtures' diffs name.
pub(super) fn repository_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().expect("root")
}

/// Alt+O in the preview opens the file on screen at the top line in view; the footer says so.
#[test]
fn peek_opens_its_file_in_the_editor_where_the_reader_is() {
    let mut app = crate::fixture::seeded_app("luxe", crate::fixture::FixtureScenario::ChangesWide)
        .expect("the fixture");
    app.project_root = Some(repository_root());
    let path = app.changes.as_ref().map(|view| view.path.clone()).expect("a peek");
    app.update(Msg::Key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::ALT)));
    let target = app.outbox.editor_open.take().expect("asked to open");
    assert_eq!(target.path, repository_root().join(&path));
    let first_hunk =
        app.changes.as_ref().and_then(|view| view.file()).map(|file| file.hunks[0].new_range.start);
    assert_eq!(target.line, first_hunk, "scrolled to the top: the first hunk's first line");

    // Somewhere the working tree does not have it, it says so instead of opening an empty buffer
    app.project_root = Some(std::path::PathBuf::from("/nonexistent-luxe-root"));
    app.update(Msg::DispatchUiAction(luxe_core::keymap::UiAction::ChangesOpenInEditor));
    assert!(app.outbox.editor_open.is_none());
}

/// A transcript of one exchange, the way the reader sees it once the answer has arrived.
pub(super) fn one_exchange(tx: mpsc::Sender<luxe_core::engine::Action>) -> App {
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    app.session_id = Some("session-a".into());
    app.status.model = "test-model".into();
    app.push_convo(ConvoBlock::User {
        text: "teh question".into(),
        attached: vec![],
        chips: vec![],
    });
    app.push_convo(ConvoBlock::Assistant("first answer".into()));
    app
}

pub(super) fn transcript(app: &App) -> Vec<String> {
    app.convo
        .iter()
        .map(|item| match item.block() {
            ConvoBlock::User { text, .. } => format!("you: {text}"),
            ConvoBlock::Assistant(text) => format!("agent: {text}"),
            other => format!("{other:?}"),
        })
        .collect()
}

/// Regenerating takes the old answer away; asking again repeats the question below it.
#[tokio::test]
async fn regenerating_and_asking_again_show_what_the_model_is_sent() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut app = one_exchange(tx);
    app.dispatch_action(UiAction::Regenerate);
    assert_eq!(transcript(&app), ["you: teh question"]);
    assert!(matches!(rx.try_recv(), Ok(luxe_core::engine::Action::Regenerate)));

    let (tx, mut rx) = mpsc::channel(8);
    let mut app = one_exchange(tx);
    app.dispatch_action(UiAction::Retry);
    assert_eq!(transcript(&app), ["you: teh question", "agent: first answer", "you: teh question"]);
    assert!(matches!(rx.try_recv(), Ok(luxe_core::engine::Action::Retry)));
}

/// None of the three rewrite a conversation a turn is still writing.
#[tokio::test]
async fn a_running_turn_is_not_rewritten_under_it() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut app = one_exchange(tx);
    app.status.state = AppState::Streaming;
    for action in [UiAction::Retry, UiAction::Regenerate, UiAction::EditResend] {
        app.dispatch_action(action);
    }
    assert_eq!(transcript(&app), ["you: teh question", "agent: first answer"]);
    assert!(rx.try_recv().is_err(), "nothing was sent");
    assert!(app.editor.current_input().is_empty(), "and nothing is waiting to be edited");
}

// === Modes ===
pub(super) fn click_at(app: &mut App, column: u16, row: u16) {
    let click = crossterm::event::MouseEvent {
        kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    };
    if let crate::main_loop::RouteAction::Msg(message) = crate::main_loop::route_mouse(app, &click)
    {
        app.update(*message);
    }
}
