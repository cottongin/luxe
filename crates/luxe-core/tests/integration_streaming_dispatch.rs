//! A call runs as soon as the model has written it.
//!
//! A model writes a message's calls one after another, and luxe used to run none of them until the
//! last had arrived: each card opened, finished, and sat waiting while the next was written, then the
//! whole stack ran inside a second. Now a finished call runs while the model writes the next one.
//!
//! What has to hold, in the order the cases below take it:
//!
//! 1. A written call runs before the reply has finished arriving, and the results still commit as
//!    one Tool message after the assistant message, in call order.
//! 2. A lookup waits for the committed message, since its fold is declared there, and so does every
//!    call after it, so order is never broken.
//! 3. A reply the provider cuts off after some of its calls ran commits those calls with their
//!    results, and nothing else: the model has to be told what already changed.
//! 4. A cancel mid-reply answers the call that ran with what it did and the half-written one as never
//!    started, and does not blame a token limit.

#![allow(clippy::unwrap_used)]

use std::collections::VecDeque;
use std::sync::Arc;

use futures::stream::{self, BoxStream};
use luxe_core::engine::{Action, Engine, EngineConfig};
use luxe_core::event::Event;
use luxe_core::message::{ContentBlock, Message, Role};
use luxe_core::provider::{
    CompletionRequest, ModelInfo, Provider, ProviderError, ProviderEvent, StopReason, ToolSchema,
};
use luxe_core::session::{EntryKind, JsonlSessionStore, SessionId, SessionStore};
use luxe_core::tool::{Tool, ToolContext, ToolRegistry, ToolResultData};
use luxe_core::HookRegistry;
use tokio::sync::{Mutex, Notify};

const WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// A tool that records which call ran, in what order. `name` decides whether it is a lookup.
struct Recorder {
    name: &'static str,
    ran: Arc<Mutex<Vec<String>>>,
}

#[async_trait::async_trait]
impl Tool for Recorder {
    fn name(&self) -> &str {
        self.name
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: self.name.to_string(),
            description: "Record the call.".to_string(),
            parameters: serde_json::json!({"type": "object", "properties": {"n": {"type": "integer"}}}),
        }
    }

    async fn run(&self, args: serde_json::Value, _cx: &ToolContext) -> ToolResultData {
        let n = args.get("n").and_then(serde_json::Value::as_i64).unwrap_or_default();
        self.ran.lock().await.push(format!("{}#{n}", self.name));
        ToolResultData { content: vec![ContentBlock::Text(format!("ran {n}"))], is_error: false }
    }
}

/// One step of a scripted reply.
enum Step {
    Event(ProviderEvent),
    /// Hold the reply here until the test opens the gate, or until a cancel ends the stream.
    Hold(Arc<Notify>),
}

/// Replies with `first` on the first call and a short answer after, recording every request.
struct Scripted {
    first: Mutex<Option<Vec<Step>>>,
    requests: Arc<Mutex<Vec<CompletionRequest>>>,
}

#[async_trait::async_trait]
impl Provider for Scripted {
    fn name(&self) -> &str {
        "scripted"
    }

    async fn stream(
        &self,
        request: CompletionRequest,
        cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        self.requests.lock().await.push(request);
        let steps: VecDeque<Step> = match self.first.lock().await.take() {
            Some(steps) => steps.into(),
            None => vec![
                Step::Event(ProviderEvent::TextDelta { index: 0, delta: "done".into() }),
                Step::Event(ProviderEvent::Done { reason: StopReason::EndTurn }),
            ]
            .into(),
        };
        Ok(Box::pin(stream::unfold((steps, cancel), |(mut steps, mut cancel)| async move {
            loop {
                match steps.pop_front()? {
                    Step::Event(event) => return Some((event, (steps, cancel))),
                    Step::Hold(gate) => {
                        tokio::select! {
                            () = gate.notified() => {}
                            _ = cancel.wait_for(|cancelled| *cancelled) => return None,
                        }
                    }
                }
            }
        })))
    }
}

fn start(index: usize, id: &str, name: &str) -> Step {
    Step::Event(ProviderEvent::ToolCallStart { index, id: id.into(), name: name.into() })
}

fn args(index: usize, json: &str) -> Step {
    Step::Event(ProviderEvent::ToolCallDelta { index, json_delta: json.into() })
}

fn end(index: usize) -> Step {
    Step::Event(ProviderEvent::ToolCallEnd { index })
}

fn done() -> Step {
    Step::Event(ProviderEvent::Done { reason: StopReason::ToolCall })
}

struct Harness {
    actions: tokio::sync::mpsc::Sender<Action>,
    events: tokio::sync::mpsc::Receiver<Event>,
    ran: Arc<Mutex<Vec<String>>>,
    reader: JsonlSessionStore,
    session_id: SessionId,
    _tmp: tempfile::TempDir,
}

fn harness(first: Vec<Step>) -> Harness {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/test").unwrap();
    let ran = Arc::new(Mutex::new(Vec::new()));
    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(Recorder { name: "touch", ran: ran.clone() }));
    tools.register(Arc::new(Recorder { name: "read", ran: ran.clone() }));
    let provider = Arc::new(Scripted {
        first: Mutex::new(Some(first)),
        requests: Arc::new(Mutex::new(Vec::new())),
    });
    let config = EngineConfig {
        session_store: std::sync::RwLock::new(Box::new(store)),
        provider,
        providers: std::collections::HashMap::new(),
        model: ModelInfo { context_window: 200_000, ..ModelInfo::default() },
        tool_registry: tools,
        hook_registry: HookRegistry::new(),
        session_id: session_id.clone(),
        system_prompt: None,
        todo_store: None,
        compaction_settings: luxe_core::compaction::CompactionSettings {
            enabled: false,
            ..luxe_core::compaction::CompactionSettings::default()
        },
        permission: Default::default(),
        modes: Default::default(),
        reviewer: None,
        formatter_registry: None,
        spend_sink: None,
        guided_review_project_root: None,
        invoked_from: None,
        external_work: None,
        review_context_files: Vec::new(),
        review_exclude: Vec::new(),
        remember_choices_in: None,
        cache_warming: luxe_core::cache_warming::CacheWarming::Off,
        review_insight_provider: None,
        github: None,
        permissions_file: None,
        max_tool_iterations: luxe_core::engine::DEFAULT_MAX_TOOL_ITERATIONS,
        startup_notices: Vec::new(),
        action_tx: None,
    };
    let reader = JsonlSessionStore::new(tmp.path().join("sessions"));
    let (actions, events) = Engine::spawn(config);
    Harness { actions, events, ran, reader, session_id, _tmp: tmp }
}

impl Harness {
    async fn submit(&self) {
        self.actions
            .send(Action::SubmitUserMessage {
                content: "go".into(),
                images: vec![],
                remediation: None,
            })
            .await
            .unwrap();
    }

    /// Every event up to the first `stop` says yes to, which is returned last.
    async fn until(&mut self, stop: impl Fn(&Event) -> bool) -> Vec<Event> {
        let mut seen = Vec::new();
        loop {
            match tokio::time::timeout(WAIT, self.events.recv()).await {
                Ok(Some(event)) => {
                    let found = stop(&event);
                    seen.push(event);
                    if found {
                        return seen;
                    }
                }
                Ok(None) => panic!("the engine stopped; saw {seen:?}"),
                Err(_) => panic!("timed out; saw {seen:?}"),
            }
        }
    }

    fn messages(&self) -> Vec<Message> {
        self.reader
            .load(&self.session_id)
            .unwrap()
            .into_iter()
            .filter_map(|entry| match entry.kind {
                EntryKind::Message(message) => Some(message),
                _ => None,
            })
            .collect()
    }
}

fn completed(id: &'static str) -> impl Fn(&Event) -> bool {
    move |event| matches!(event, Event::ToolCompleted { call_id, .. } if call_id == id)
}

fn calls_in(message: &Message) -> Vec<String> {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolCall { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect()
}

fn results_in(message: &Message) -> Vec<(String, String)> {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolResult { call_id, content, .. } => {
                let text = content
                    .iter()
                    .map(|part| match part {
                        luxe_core::message::ToolOutput::Text(text) => text.clone(),
                        _ => String::new(),
                    })
                    .collect::<String>();
                Some((call_id.clone(), text))
            }
            _ => None,
        })
        .collect()
}

/// The first case: the reply is held after its first call, and that call runs inside the hold.
#[tokio::test]
async fn a_written_call_runs_while_the_next_is_written() {
    let gate = Arc::new(Notify::new());
    let mut h = harness(vec![
        start(0, "c1", "touch"),
        args(0, r#"{"n":1}"#),
        end(0),
        Step::Hold(gate.clone()),
        start(1, "c2", "touch"),
        args(1, r#"{"n":2}"#),
        end(1),
        done(),
    ]);
    h.submit().await;

    // The reply is still being held when the first call finishes, so it can only have run early
    let before = h.until(completed("c1")).await;
    assert!(
        !before.iter().any(|event| matches!(event, Event::MessageCommitted { message, .. }
            if message.role == Role::Assistant)),
        "nothing has committed yet: {before:?}"
    );
    assert_eq!(*h.ran.lock().await, ["touch#1"]);

    gate.notify_one();
    h.until(|event| matches!(event, Event::TurnEnded { .. })).await;
    assert_eq!(*h.ran.lock().await, ["touch#1", "touch#2"], "each ran once, in order");

    let messages = h.messages();
    let assistant = messages.iter().position(|m| !calls_in(m).is_empty()).expect("the calls");
    assert_eq!(calls_in(&messages[assistant]), ["c1", "c2"]);
    let results = results_in(&messages[assistant + 1]);
    assert_eq!(
        results,
        [("c1".into(), "ran 1".into()), ("c2".into(), "ran 2".into())],
        "one Tool message right after the calls, in call order"
    );
}

/// A lookup's fold is declared by the committed message, so it waits for it — and so does every
/// call after it, even one that could have run early on its own.
#[tokio::test]
async fn a_lookup_and_everything_after_it_waits_for_the_message() {
    let mut h = harness(vec![
        start(0, "c1", "read"),
        args(0, r#"{"n":1}"#),
        end(0),
        start(1, "c2", "touch"),
        args(1, r#"{"n":2}"#),
        end(1),
        done(),
    ]);
    h.submit().await;
    let seen = h.until(|event| matches!(event, Event::TurnEnded { .. })).await;
    let committed = seen
        .iter()
        .position(|event| {
            matches!(event, Event::MessageCommitted { message, .. }
            if message.role == Role::Assistant)
        })
        .expect("the assistant message");
    let started = |id: &str| {
        seen.iter().position(|e| matches!(e, Event::ToolStarted { call_id, .. } if call_id == id))
    };
    assert!(started("c1").expect("c1 ran") > committed, "the lookup waited: {seen:?}");
    assert!(started("c2").expect("c2 ran") > committed, "and so did what came after it");
    assert_eq!(*h.ran.lock().await, ["read#1", "touch#2"]);
}

/// A reply cut off by the provider after its first call ran: that call commits with its result, the
/// half-written one is dropped, and the turn still fails.
#[tokio::test]
async fn calls_that_ran_commit_when_the_reply_fails() {
    let gate = Arc::new(Notify::new());
    let mut h = harness(vec![
        start(0, "c1", "touch"),
        args(0, r#"{"n":1}"#),
        end(0),
        Step::Hold(gate.clone()),
        start(1, "c2", "touch"),
        args(1, r#"{"n":"#),
        Step::Event(ProviderEvent::Error {
            message: "overloaded".into(),
            kind: luxe_core::provider::ProviderErrorKind::Other("overloaded".into()),
        }),
    ]);
    h.submit().await;
    h.until(completed("c1")).await;
    gate.notify_one();
    h.until(|event| matches!(event, Event::Error(_))).await;

    let messages = h.messages();
    let assistant = messages.iter().position(|m| !calls_in(m).is_empty()).expect("the calls");
    assert_eq!(calls_in(&messages[assistant]), ["c1"], "only the call that ran");
    assert_eq!(results_in(&messages[assistant + 1]), [("c1".into(), "ran 1".into())]);
    assert_eq!(*h.ran.lock().await, ["touch#1"]);
}

/// A reply that finished but wrote one call's arguments as something other than JSON: that call
/// is answered with why, the others run, and the turn goes on. Ending it with "raise max_tokens"
/// blamed a limit the reply never reached.
#[tokio::test]
async fn a_malformed_call_is_answered_and_the_turn_goes_on() {
    let mut h = harness(vec![
        start(0, "c1", "touch"),
        args(0, r#"{"n":1}"#),
        end(0),
        start(1, "c2", "touch"),
        args(1, "{\"oldText\">\ntest(\"open sorts, \"n\": 2}"),
        end(1),
        done(),
    ]);
    h.submit().await;
    let seen = h.until(|event| matches!(event, Event::TurnEnded { .. })).await;
    assert!(
        !seen.iter().any(|event| matches!(event, Event::Error(_))),
        "nothing went wrong that the reader must act on: {seen:?}"
    );
    assert_eq!(*h.ran.lock().await, ["touch#1"], "only the call that parsed ran");

    let messages = h.messages();
    let assistant = messages.iter().position(|m| !calls_in(m).is_empty()).expect("the calls");
    let results = results_in(&messages[assistant + 1]);
    assert_eq!(results[0], ("c1".into(), "ran 1".into()));
    assert_eq!(results[1].0, "c2");
    assert!(results[1].1.contains("not valid JSON"), "{:?}", results[1]);
    assert!(
        messages
            .iter()
            .any(|m| m.content.iter().any(|b| matches!(b, ContentBlock::Text(t) if t == "done"))),
        "the model was asked again and answered"
    );
}

/// A model stuck writing whitespace after a call's last value is cut off where it stands, without
/// waiting out the stream, and the call is answered so the model can send it again. Waiting is
/// what it used to do: twenty minutes, to the output limit, billed.
#[tokio::test]
async fn a_call_trailing_endless_whitespace_is_cut_off_and_answered() {
    let never = Arc::new(Notify::new());
    let mut steps = vec![start(0, "c1", "touch"), args(0, r#"{"n":1"#)];
    steps.extend((0..200).map(|_| args(0, " \n")));
    steps.push(Step::Hold(never));
    steps.push(done());
    let mut h = harness(steps);
    h.submit().await;
    let seen = h.until(|event| matches!(event, Event::TurnEnded { .. })).await;
    assert!(
        !seen.iter().any(|event| matches!(event, Event::Error(_))),
        "nothing went wrong that the reader must act on: {seen:?}"
    );
    assert!(h.ran.lock().await.is_empty(), "unfinished JSON never runs");

    let messages = h.messages();
    let assistant = messages.iter().position(|m| !calls_in(m).is_empty()).expect("the call");
    let results = results_in(&messages[assistant + 1]);
    assert_eq!(results[0].0, "c1");
    assert!(results[0].1.contains("not valid JSON"), "{:?}", results[0]);
    assert!(
        messages
            .iter()
            .any(|m| m.content.iter().any(|b| matches!(b, ContentBlock::Text(t) if t == "done"))),
        "the model was asked again and answered"
    );
}

/// The same bad JSON in a reply stopped by its token limit is the cut-off it looks like.
#[tokio::test]
async fn arguments_cut_off_by_the_limit_still_stop_the_turn() {
    let mut h = harness(vec![
        start(0, "c1", "touch"),
        args(0, r#"{"n":"#),
        Step::Event(ProviderEvent::Done { reason: StopReason::Length }),
    ]);
    h.submit().await;
    let seen = h.until(|event| matches!(event, Event::TurnEnded { .. })).await;
    assert!(
        seen.iter()
            .any(|event| matches!(event, Event::Error(message) if message.contains("max_tokens"))),
        "{seen:?}"
    );
    assert!(h.ran.lock().await.is_empty());
}

/// Cancelled while the second call is half-written: the first is answered with what it did, the
/// second as never started, and nobody is told to raise a token limit.
#[tokio::test]
async fn a_cancel_mid_reply_answers_what_ran_and_what_did_not() {
    let gate = Arc::new(Notify::new());
    let mut h = harness(vec![
        start(0, "c1", "touch"),
        args(0, r#"{"n":1}"#),
        end(0),
        start(1, "c2", "touch"),
        args(1, r#"{"n":"#),
        Step::Hold(gate.clone()),
        done(),
    ]);
    h.submit().await;
    h.until(completed("c1")).await;
    h.actions.send(Action::Cancel).await.unwrap();
    let seen = h.until(|event| matches!(event, Event::TurnEnded { .. })).await;
    assert!(
        !seen
            .iter()
            .any(|event| matches!(event, Event::Error(text) if text.contains("max_tokens"))),
        "a cancel is not a token limit: {seen:?}"
    );

    let messages = h.messages();
    let assistant = messages.iter().position(|m| !calls_in(m).is_empty()).expect("the calls");
    assert_eq!(calls_in(&messages[assistant]), ["c1", "c2"]);
    let results = results_in(&messages[assistant + 1]);
    assert_eq!(results[0], ("c1".into(), "ran 1".into()));
    assert_eq!(results[1].0, "c2");
    assert!(results[1].1.contains("not executed"), "never started: {results:?}");
    assert_eq!(*h.ran.lock().await, ["touch#1"]);
}

/// The OpenAI shapes never close a call explicitly; the next one opening is what says the last is
/// written. That has to hand it over just the same.
#[tokio::test]
async fn the_next_call_opening_closes_the_last_one() {
    let gate = Arc::new(Notify::new());
    let mut h = harness(vec![
        start(0, "c1", "touch"),
        args(0, r#"{"n":1}"#),
        start(1, "c2", "touch"),
        Step::Hold(gate.clone()),
        args(1, r#"{"n":2}"#),
        done(),
    ]);
    h.submit().await;
    h.until(completed("c1")).await;
    assert_eq!(*h.ran.lock().await, ["touch#1"], "ran while the second was still arriving");
    gate.notify_one();
    h.until(|event| matches!(event, Event::TurnEnded { .. })).await;
    assert_eq!(*h.ran.lock().await, ["touch#1", "touch#2"]);
}

/// Text the model writes after a call joins the one text block, where text first appeared.
///
/// The reply keeps a single answer block placed at its first delta, so prose on either side of a
/// call commits as one block ahead of the call, in the order it arrived. Pinned so a change to how
/// blocks are accumulated cannot reorder a committed message unnoticed.
#[tokio::test]
async fn text_on_both_sides_of_a_call_commits_as_one_block_ahead_of_it() {
    let text =
        |delta: &str| Step::Event(ProviderEvent::TextDelta { index: 0, delta: delta.into() });
    let mut h = harness(vec![
        text("Before the call. "),
        start(0, "c1", "touch"),
        args(0, r#"{"n":1}"#),
        end(0),
        text("After it."),
        done(),
    ]);
    h.submit().await;
    h.until(|event| matches!(event, Event::TurnEnded { .. })).await;

    let messages = h.messages();
    let assistant = messages.iter().find(|m| !calls_in(m).is_empty()).expect("the reply");
    let shape: Vec<String> = assistant
        .content
        .iter()
        .map(|block| match block {
            ContentBlock::Text(text) => format!("text: {text}"),
            ContentBlock::ToolCall { id, .. } => format!("call: {id}"),
            other => format!("{other:?}"),
        })
        .collect();
    assert_eq!(shape, ["text: Before the call. After it.", "call: c1"]);
}
