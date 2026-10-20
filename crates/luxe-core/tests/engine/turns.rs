//! Turns — a tool call streams, runs, and its result reaches the next request.

use futures::StreamExt;
use std::sync::Arc;

use futures::stream::{self, BoxStream};
use luxe_core::engine::{Action, Engine, EngineConfig};
use luxe_core::event::Event;
use luxe_core::hook::{HookPoint, HookRegistry};
use luxe_core::provider::{CompletionRequest, Provider, ProviderError, ProviderEvent, StopReason};
use luxe_core::session::{JsonlSessionStore, SessionStore};
use luxe_core::tool::ToolRegistry;
use tokio::sync::RwLock;

use crate::fixtures::*;

/// Streams a `read` and a `write` the way a provider does: the name up front, then the arguments
/// in fragments, with the file's content dwarfing everything else.
struct SlowWriteProvider {
    turns: Arc<RwLock<usize>>,
}

#[async_trait::async_trait]
impl Provider for SlowWriteProvider {
    fn name(&self) -> &str {
        "slow-write"
    }

    async fn stream(
        &self,
        _request: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        let mut turns = self.turns.write().await;
        *turns += 1;
        if *turns > 1 {
            return Ok(Box::pin(stream::iter(vec![
                ProviderEvent::TextDelta { index: 0, delta: "written".into() },
                ProviderEvent::Done { reason: StopReason::EndTurn },
            ])));
        }
        // `(event, pause before it)`. The body is rate-limited, so the fragments carrying the
        // file are spaced past that window — which is also how a real model delivers them.
        let mut script: Vec<(ProviderEvent, u64)> = vec![
            // A lookup, whose card must *not* open early: three of them fold into one line, and
            // the fold is declared by the committed message.
            (
                ProviderEvent::ToolCallStart {
                    index: 0,
                    id: "call-read".into(),
                    name: "read".into(),
                },
                0,
            ),
            (
                ProviderEvent::ToolCallDelta {
                    index: 0,
                    json_delta: r#"{"path":"notes.txt"}"#.into(),
                },
                0,
            ),
            (
                ProviderEvent::ToolCallStart {
                    index: 1,
                    id: "call-write".into(),
                    name: "write".into(),
                },
                0,
            ),
        ];
        for (fragment, pause) in [
            (r#"{"pa"#, 0),
            (r#"th":"notes"#, 0),
            (r#".txt","#, 0),
            (r#""content":"one\n"#, 0),
            (r#"two\n"#, 150),
            (r#"three\n"}"#, 150),
        ] {
            script.push((
                ProviderEvent::ToolCallDelta { index: 1, json_delta: fragment.into() },
                pause,
            ));
        }
        script.push((ProviderEvent::Done { reason: StopReason::ToolCall }, 0));
        Ok(Box::pin(stream::iter(script).then(|(event, pause)| async move {
            if pause > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(pause)).await;
            }
            event
        })))
    }
}

/// A `write`'s content *is* its argument, so the model spends far longer writing the call than the
/// tool spends running it. Nothing used to reach a front-end for that whole stretch, so the card
/// landed at the moment the work finished — which reads as a tool that never ran.
#[tokio::test]
async fn a_tool_call_announces_itself_while_the_model_is_still_writing_it() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(temp_dir.path().to_path_buf());
    let session_id = store.create(&temp_dir.path().display().to_string()).unwrap();

    let config = EngineConfig {
        guided_review_project_root: Some(temp_dir.path().to_path_buf()),
        ..base_config(
            store,
            session_id,
            Arc::new(SlowWriteProvider { turns: Arc::new(RwLock::new(0)) }),
        )
    };
    let (action_tx, mut event_rx) = Engine::spawn(config);
    action_tx
        .send(Action::SubmitUserMessage {
            content: "write notes.txt".to_string(),
            images: vec![],
            remediation: None,
        })
        .await
        .unwrap();

    let mut lifecycle = Vec::new();
    loop {
        match tokio::time::timeout(EVENT_WAIT, event_rx.recv()).await {
            Ok(Some(Event::ToolCallComposing { call_id, name, subject, body })) => {
                lifecycle.push(format!(
                    "composing {call_id} {name} {} {}",
                    subject.unwrap_or_else(|| "-".into()),
                    body.map_or_else(|| "-".to_string(), |body| body.total_lines.to_string()),
                ));
            }
            Ok(Some(Event::ToolStarted { call_id, name })) => {
                lifecycle.push(format!("started {call_id} {name}"));
            }
            Ok(Some(Event::ToolEnded { call_id, .. })) => {
                lifecycle.push(format!("ended {call_id}"));
                if call_id == "call-write" {
                    break;
                }
            }
            Ok(Some(_)) => {}
            Ok(None) => panic!("the engine closed the stream before the call ran"),
            Err(_) => panic!("timed out waiting for the tool lifecycle; got {lifecycle:#?}"),
        }
    }

    assert_eq!(
        lifecycle,
        [
            // The `write` is watched as it is written — opened, named once its path closes, then
            // filling in. The `read` in the same message says nothing until it is dispatched:
            // composing a lookup would put a card on screen that the message's own declared run
            // then takes straight back.
            "composing call-write write - -",
            "composing call-write write notes.txt -",
            "composing call-write write - 2",
            "composing call-write write - 3",
            // Dispatch is after the whole message, which is why watching it arrive was worth an
            // event of its own.
            "started call-read read",
            "started call-write write",
            "ended call-read",
            "ended call-write",
        ],
        "the card opens on the first fragment, is named as soon as the path closes, and fills in \
         as the file is written",
    );

    action_tx.send(Action::Close).await.unwrap();
}

/// What the reader sends is announced like any other committed message, before the reply.
///
/// The break this catches: announcing only messages luxe composed, so a client joined to the
/// session (the phone app, `luxe join`) shows answers to questions it never saw, and its snapshot
/// for a later joiner leaves them out too.
#[tokio::test]
async fn a_sent_message_is_announced_to_every_client() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().to_path_buf());
    let session_id = store.create(&tmp.path().display().to_string()).unwrap();
    let config = base_config(store, session_id, Arc::new(MockProvider::new()));
    let (action_tx, mut event_rx) = Engine::spawn(config);
    let content = "Read test.txt".to_string();
    action_tx
        .send(Action::SubmitUserMessage { content, images: vec![], remediation: None })
        .await
        .unwrap();
    let events = events_until_turn_end(&mut event_rx).await;
    let first = first_committed(&events).expect("something was committed");
    assert_eq!(first.role, luxe_core::message::Role::User);
    assert_eq!(first.content, [luxe_core::message::ContentBlock::Text("Read test.txt".into())]);
}

/// A turn says it started, once, before anything else of it, the user's message included.
///
/// The break this catches: no start signal at all, so a client that didn't send the message (the
/// phone, the TUI beside it) has to guess from the first token; or one sent after the message
/// commits, which leaves that client showing a saved message with no turn running.
#[tokio::test]
async fn a_turn_says_it_started_before_anything_else_of_it() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().to_path_buf());
    let session_id = store.create(&tmp.path().display().to_string()).unwrap();
    let config = base_config(store, session_id, Arc::new(MockProvider::new()));
    let (action_tx, mut event_rx) = Engine::spawn(config);
    let content = "Read test.txt".to_string();
    action_tx
        .send(Action::SubmitUserMessage { content, images: vec![], remediation: None })
        .await
        .unwrap();
    let events = events_until_turn_end(&mut event_rx).await;
    let started = |event: &Event| matches!(event, Event::TurnStarted);
    let committed = |event: &Event| matches!(event, Event::MessageCommitted { .. });
    assert_eq!(events.iter().filter(|event| started(event)).count(), 1);
    let start = events.iter().position(started).expect("turn_started");
    assert!(start < events.iter().position(committed).expect("a commit"), "{events:?}");
}

fn first_committed(events: &[Event]) -> Option<&luxe_core::message::Message> {
    events.iter().find_map(|event| match event {
        Event::MessageCommitted { message, .. } => Some(message),
        _ => None,
    })
}

/// A tool call the model makes runs the tool, and what it returned is in the next request.
///
/// Three claims, asserted apart so a failure says which link broke: dispatch ran the registered
/// tool (it counted the call), the call and its result were paired and committed as one unit (the
/// lifecycle events and the second request agree), and the answer the tool gave is what the model
/// reads next. Unregister the tool and the first fails; drop the result from the transcript and
/// the last does.
#[tokio::test]
async fn a_tool_call_runs_and_its_result_reaches_the_next_request() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().to_path_buf());
    let session_id = store.create(&tmp.path().display().to_string()).unwrap();

    let witness = WitnessReadTool::new();
    let mut tool_registry = ToolRegistry::new();
    tool_registry.register(witness.clone());
    let (recording_hook, recorded_points) = RecordingHook::new();
    let mut hook_registry = HookRegistry::new();
    hook_registry.register(Arc::new(recording_hook));
    let provider = Arc::new(MockProvider::new());
    let requests = provider.requests();

    let config =
        EngineConfig { tool_registry, hook_registry, ..base_config(store, session_id, provider) };
    let (action_tx, mut event_rx) = Engine::spawn(config);
    action_tx
        .send(Action::SubmitUserMessage {
            content: "Read test.txt".to_string(),
            images: vec![],
            remediation: None,
        })
        .await
        .unwrap();
    let events = events_until_turn_end(&mut event_rx).await;

    // dispatch ran the tool, once, with the arguments the model streamed
    assert_eq!(witness.invocations(), [serde_json::json!({"path": "test.txt"})]);

    // the call started and ended without an error
    let lifecycle: Vec<String> = events
        .iter()
        .filter_map(|event| match event {
            Event::ToolStarted { call_id, name } => Some(format!("started {call_id} {name}")),
            Event::ToolEnded { call_id, is_error } => {
                Some(format!("ended {call_id} error={is_error}"))
            }
            _ => None,
        })
        .collect();
    assert_eq!(lifecycle, ["started call_1 read", "ended call_1 error=false"]);

    // the next request carries the call and its answer, paired, with the tool's own words
    let requests = requests.read().await;
    assert_eq!(requests.len(), 2, "the result sent the model back for another reply");
    let (calls, results) = pairing(&requests[1].messages);
    assert_eq!((calls, results), (vec!["call_1".to_string()], vec!["call_1".to_string()]));
    assert_eq!(result_text(&requests[1].messages, "call_1"), WITNESS_OUTPUT);
    let first_request_results = pairing(&requests[0].messages).1;
    assert!(first_request_results.is_empty(), "the first request had nothing to answer");

    // the hooks around the call fired; `AgentEnd` comes after the turn's last event, so close the
    // engine and wait for its stream to end before reading them
    action_tx.send(Action::Close).await.unwrap();
    tokio::time::timeout(EVENT_WAIT, async { while event_rx.recv().await.is_some() {} })
        .await
        .expect("the engine closes its stream after Close");
    let points = recorded_points.read().await;
    for point in [
        HookPoint::SessionStart,
        HookPoint::UserPromptSubmit,
        HookPoint::BeforeToolUse,
        HookPoint::AfterToolUse,
        HookPoint::AgentEnd,
    ] {
        assert!(points.contains(&point), "{point:?} should have fired: {points:?}");
    }
}
