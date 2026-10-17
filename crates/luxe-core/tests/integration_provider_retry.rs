//! A provider that could not answer just now gets the same request again, and only then.
//!
//! What has to hold:
//!
//! 1. A refusal a second try can fix (overloaded, a 5xx, a rate limit, a request that never
//!    arrived) is sent again after a pause, and the turn finishes as if it had not happened, each
//!    try announced.
//! 2. A stream whose first event is that kind of failure (a stall before anything arrived) is too.
//! 3. A refusal a second try cannot fix, a bad key, is reported at once.
//! 4. A stream that has said something is never replayed, however it fails after.
//! 5. Past the last try, the failure is reported the way it always was.

#![allow(clippy::unwrap_used)]

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use futures::stream::{self, BoxStream};
use luxe_core::engine::{Action, Engine, EngineConfig};
use luxe_core::event::Event;
use luxe_core::provider::{
    CompletionRequest, ModelInfo, Provider, ProviderError, ProviderErrorKind, ProviderEvent,
    StopReason,
};
use luxe_core::session::{JsonlSessionStore, SessionStore};
use luxe_core::tool::ToolRegistry;
use luxe_core::HookRegistry;
use tokio::sync::Mutex;

/// What one call to the provider does.
enum Answer {
    Refuse(&'static str),
    Stream(Vec<ProviderEvent>),
}

/// Answers each call with the next scripted [`Answer`], then with a short reply for good.
struct Flaky {
    answers: Mutex<VecDeque<Answer>>,
    calls: Arc<AtomicUsize>,
    /// How many messages each request carried
    sizes: Arc<std::sync::Mutex<Vec<usize>>>,
}

#[async_trait::async_trait]
impl Provider for Flaky {
    fn name(&self) -> &str {
        "flaky"
    }

    async fn stream(
        &self,
        request: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.sizes.lock().unwrap().push(request.messages.len());
        match self.answers.lock().await.pop_front() {
            Some(Answer::Refuse(message)) => Err(ProviderError::Other(message.into())),
            Some(Answer::Stream(events)) => Ok(Box::pin(stream::iter(events))),
            None => Ok(Box::pin(stream::iter(reply("done")))),
        }
    }
}

fn reply(text: &str) -> Vec<ProviderEvent> {
    vec![
        ProviderEvent::TextDelta { index: 0, delta: text.into() },
        ProviderEvent::Done { reason: StopReason::EndTurn },
    ]
}

fn stalled() -> ProviderEvent {
    ProviderEvent::Error {
        message: "stream error: timed out after 180s with nothing from the provider".into(),
        kind: ProviderErrorKind::Transport,
    }
}

const OVERLOADED: &str =
    r#"Anthropic API error 529 <unknown status code>: {"type":"overloaded_error"}"#;

/// Runs one turn against `answers` and returns every event up to its end, and the call count.
async fn turn(answers: Vec<Answer>) -> (Vec<Event>, usize) {
    turn_after(answers, &[]).await
}

/// The same, with `before` submitted first: messages that should start nothing.
async fn turn_after(answers: Vec<Answer>, before: &[&str]) -> (Vec<Event>, usize) {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/test").unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let sizes = Arc::new(std::sync::Mutex::new(Vec::new()));
    let provider = Arc::new(Flaky {
        answers: Mutex::new(answers.into()),
        calls: calls.clone(),
        sizes: sizes.clone(),
    });
    let config = EngineConfig {
        session_store: std::sync::RwLock::new(Box::new(store)),
        provider,
        providers: std::collections::HashMap::new(),
        model: ModelInfo { context_window: 200_000, ..ModelInfo::default() },
        tool_registry: ToolRegistry::new(),
        hook_registry: HookRegistry::new(),
        session_id,
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
        remember_runtime_state: false,
        cache_warming: luxe_core::cache_warming::CacheWarming::Off,
        review_insight_provider: None,
        github: None,
        permissions_file: None,
        max_tool_iterations: luxe_core::engine::DEFAULT_MAX_TOOL_ITERATIONS,
        startup_notices: Vec::new(),
        action_tx: None,
    };
    let (actions, mut events) = Engine::spawn(config);
    for content in before {
        actions
            .send(Action::SubmitUserMessage {
                content: (*content).into(),
                images: vec![],
                remediation: None,
            })
            .await
            .unwrap();
    }
    actions
        .send(Action::SubmitUserMessage { content: "go".into(), images: vec![], remediation: None })
        .await
        .unwrap();
    let mut seen = Vec::new();
    while let Some(event) = events.recv().await {
        let ended = matches!(event, Event::TurnEnded { .. });
        seen.push(event);
        if ended {
            break;
        }
    }
    SIZES.with(|cell| *cell.borrow_mut() = sizes.lock().unwrap().clone());
    (seen, calls.load(Ordering::SeqCst))
}

thread_local! {
    /// The message counts of the last run's requests, for the one test that asks
    static SIZES: std::cell::RefCell<Vec<usize>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn retries(events: &[Event]) -> Vec<(u32, u32)> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::ProviderRetrying { attempt, attempts, .. } => Some((*attempt, *attempts)),
            _ => None,
        })
        .collect()
}

fn answered(events: &[Event]) -> bool {
    events.iter().any(|event| matches!(event, Event::TokenDelta { delta, .. } if delta == "done"))
}

fn errors(events: &[Event]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::Error(message) => Some(message.as_str()),
            _ => None,
        })
        .collect()
}

#[tokio::test(start_paused = true)]
async fn an_overloaded_provider_is_asked_again() {
    let (events, calls) =
        turn(vec![Answer::Refuse(OVERLOADED), Answer::Refuse("503 Service Unavailable: busy")])
            .await;
    assert_eq!(calls, 3, "two refusals and the answer");
    assert_eq!(retries(&events), [(1, 3), (2, 3)], "each try announced");
    assert!(answered(&events), "{events:?}");
    assert!(errors(&events).is_empty(), "nothing for the reader to act on: {events:?}");
}

#[tokio::test(start_paused = true)]
async fn a_stream_that_stalls_before_saying_anything_is_asked_again() {
    let (events, calls) = turn(vec![Answer::Stream(vec![stalled()])]).await;
    assert_eq!(calls, 2);
    assert_eq!(retries(&events), [(1, 3)]);
    assert!(answered(&events), "{events:?}");
}

#[tokio::test(start_paused = true)]
async fn a_refusal_a_retry_cannot_fix_is_reported_at_once() {
    let (events, calls) = turn(vec![Answer::Refuse("401 Unauthorized: invalid_api_key")]).await;
    assert_eq!(calls, 1);
    assert!(retries(&events).is_empty());
    assert!(!errors(&events).is_empty(), "{events:?}");
}

#[tokio::test(start_paused = true)]
async fn a_stream_that_has_said_something_is_never_replayed() {
    let (events, calls) = turn(vec![Answer::Stream(vec![
        ProviderEvent::TextDelta { index: 0, delta: "half an ans".into() },
        stalled(),
    ])])
    .await;
    assert_eq!(calls, 1, "the half it said would be drawn and billed twice");
    assert!(retries(&events).is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_stream_that_keeps_stalling_is_reported_after_the_last_try() {
    let stalls = (0..4).map(|_| Answer::Stream(vec![stalled()])).collect();
    let (events, calls) = turn(stalls).await;
    assert_eq!(calls, 4);
    assert_eq!(retries(&events), [(1, 3), (2, 3), (3, 3)]);
    assert!(errors(&events).iter().any(|message| message.contains("timed out")), "{events:?}");
}

#[tokio::test(start_paused = true)]
async fn past_the_last_try_the_failure_is_reported() {
    let refusals = (0..4).map(|_| Answer::Refuse(OVERLOADED)).collect();
    let (events, calls) = turn(refusals).await;
    assert_eq!(calls, 4, "the first and three more");
    assert_eq!(retries(&events), [(1, 3), (2, 3), (3, 3)]);
    assert!(errors(&events).iter().any(|message| message.contains("overloaded")), "{events:?}");
    assert!(!answered(&events));
}

/// A message with nothing in it starts nothing and commits nothing: committed, it would leave every
/// later request ending on the agent's reply, which Anthropic refuses for the rest of the session.
#[tokio::test(start_paused = true)]
async fn an_empty_message_is_not_sent() {
    let (events, calls) = turn_after(Vec::new(), &["", "  ", "\n\t"]).await;
    assert_eq!(calls, 1, "only the real message reached the provider");
    assert_eq!(SIZES.with(|cell| cell.borrow().clone()), [1], "and nothing empty went with it");
    assert!(answered(&events) && errors(&events).is_empty(), "{events:?}");
}
