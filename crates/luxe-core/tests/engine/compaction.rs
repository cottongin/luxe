//! Regression: compaction has to change the request, not just the log.
//!
//! It summarized, billed for the call, wrote the summary to disk — and then sent the whole
//! transcript anyway, because a session log is append-only and nothing applied the cut. Two
//! independent breaks did it: the `Compaction` entry was chained under `first_kept` instead of
//! after the leaf, so it sat on a fork nothing walked; and the engine discarded the id, so the
//! next request was built from the pre-compaction leaf regardless. Occupancy never moved, so the
//! threshold fired again on the following turn, and the next, each one paying for a summarizer
//! call that could not take effect.
//!
//! Everything here is asserted against the request the mock provider actually received.

use std::sync::Arc;

use crate::fixtures::base_config;
use futures::stream;
use futures::stream::BoxStream;
use luxe_core::compaction::CompactionSettings;
use luxe_core::engine::{Action, Engine, EngineConfig};
use luxe_core::event::Event;
use luxe_core::message::{ContentBlock, Message, Role};
use luxe_core::provider::{
    CompletionRequest, ModelInfo, ModelRef, PromptReuse, Provider, ProviderError, ProviderEvent,
    StopReason, Usage,
};
use luxe_core::session::{EntryKind, JsonlSessionStore, SessionId, SessionStore};
use luxe_core::tool::ToolRegistry;
use tokio::sync::Mutex;

/// `usable = 27_700 - min(1_000, 20_000)` = 26 700, and the trigger is 90% of that.
const CONTEXT_WINDOW: u32 = 27_700;
const MAX_TOKENS: u32 = 1_000;
/// What the settings above work out to, quoted so an assertion can name it.
const THRESHOLD: u64 = 24_030;

/// Roughly what one fixture prompt costs, at four characters to the token.
const PROMPT_TOKENS: u64 = 2_000;

/// The text the fake summarizer produces, so an assertion can find it in the next request.
const SUMMARY_MARKER: &str = "Summarized-Goal-Marker";

/// Enough turns that occupancy climbs past the threshold, compaction fires, and several turns run
/// *after* it without firing again — which is the property that was broken.
const TURNS: usize = 15;

/// Records every request and answers two kinds of call: a summarization (no tools, and a system
/// prompt that says what it is) and an ordinary turn.
///
/// Reports the occupancy of the request it was **actually given**, the way a real provider does.
/// A mock that reports a fixed number can't tell a compaction that worked from one that didn't:
/// the reading stays pinned either way, and the threshold trips again on the next turn.
struct CompactingProvider {
    calls: Arc<Mutex<Vec<CompletionRequest>>>,
}

impl CompactingProvider {
    fn is_summarization(request: &CompletionRequest) -> bool {
        request.system.as_deref().is_some_and(|system| system.contains("session compactor"))
    }
}

#[async_trait::async_trait]
impl Provider for CompactingProvider {
    fn name(&self) -> &str {
        "compacting-mock"
    }

    async fn stream(
        &self,
        request: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        let summarizing = Self::is_summarization(&request);
        let occupancy = occupancy_of(&request);
        self.calls.lock().await.push(request);

        // Summary text a real summarizer would plausibly return: the engine refuses one that
        // carries no prose, because an empty summary silently deletes everything it replaces.
        let text = match summarizing {
            true => format!(
                "## Goal\n{SUMMARY_MARKER}: keep the conversation going while the window stays \
                 under its limit.\n\n## Next Steps\nCarry on with the remaining turns."
            ),
            false => "ok".to_string(),
        };
        let mut events = vec![ProviderEvent::TextDelta { index: 0, delta: text }];
        // A summarization call measures its own prompt, not the turn's context — reporting the
        // turn's occupancy from it would keep the reading pinned at the pre-compaction figure.
        if !summarizing {
            events.push(ProviderEvent::Usage(Usage {
                prompt_tokens: occupancy,
                completion_tokens: 1,
                total_tokens: occupancy,
                ..Usage::default()
            }));
        }
        events.push(ProviderEvent::Done { reason: StopReason::EndTurn });
        Ok(Box::pin(stream::iter(events)))
    }
}

/// What a provider reports for a request: the conversation *and* the prefix above it. A mock that
/// counts only the messages under-reports the way no real endpoint does, and compaction reads that
/// figure to work out how much of the window its own prompt is already using.
fn occupancy_of(request: &CompletionRequest) -> u32 {
    let messages = luxe_core::compaction::estimate_message_tokens(&request.messages);
    let prefix =
        luxe_core::compaction::estimate_prefix_tokens(request.system.as_deref(), &request.tools);
    (messages + prefix).min(u32::MAX.into()) as u32
}

fn model() -> ModelInfo {
    ModelInfo {
        r#ref: ModelRef::new("mock", "compacting"),
        context_window: CONTEXT_WINDOW,
        max_tokens: MAX_TOKENS,
        ..Default::default()
    }
}

fn settings() -> CompactionSettings {
    CompactionSettings {
        enabled: true,
        reserve_tokens: 20_000,
        // Several fixture prompts' worth, deliberately. A `keep_recent` smaller than one message
        // puts the cut on the leaf itself, and that hides the fork the old code created — with
        // nothing after `first_kept`, there is nothing left to strand.
        keep_recent_tokens: 9_000,
        compact_at_fraction: 0.9,
        // Off: pruning would shrink the same tool output this test is measuring.
        prune_protect_tokens: u32::MAX,
        prune_min_tokens: u32::MAX,
        prune_at_fraction: 1.0,
        summarizer: None,
    }
}

/// `invoked_from` is the test's own directory on purpose.
///
/// Left unset, the engine takes the *process's* directory — this repository — so the system prompt
/// these tests measure against a threshold quietly includes luxe's own `AGENTS.md`, its skills, and
/// the name of the branch they run on. Editing `AGENTS.md` then fails three compaction tests, which
/// is a fixture reading the room rather than a defect.
fn config(
    store: JsonlSessionStore,
    session_id: SessionId,
    provider: Arc<dyn Provider + Send + Sync>,
    invoked_from: &std::path::Path,
) -> EngineConfig {
    EngineConfig {
        model: model(),
        compaction_settings: settings(),
        invoked_from: Some(invoked_from.to_path_buf()),
        ..base_config(store, session_id, provider)
    }
}

async fn run_turn(
    action_tx: &tokio::sync::mpsc::Sender<Action>,
    event_rx: &mut tokio::sync::mpsc::Receiver<Event>,
    prompt: String,
) {
    action_tx
        .send(Action::SubmitUserMessage { content: prompt, images: Vec::new(), remediation: None })
        .await
        .unwrap();
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(10), event_rx.recv()).await {
            Ok(Some(Event::TurnEnded { .. })) => break,
            Ok(Some(_)) => {}
            Ok(None) => panic!("event channel closed"),
            Err(_) => panic!("timed out waiting for the turn to end"),
        }
    }
}

/// Every text block in a request, in order.
fn texts(request: &CompletionRequest) -> Vec<String> {
    request
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// A prompt worth [`PROMPT_TOKENS`], so a dozen of them clear the threshold.
fn bulky(label: &str) -> String {
    format!("{label} {}", "x".repeat(PROMPT_TOKENS as usize * 4))
}

/// Which turns' prompts survived into a set of sent texts, in order.
fn turns_present(sent: &[String]) -> Vec<usize> {
    sent.iter()
        .filter_map(|text| text.strip_prefix("TURN-"))
        .filter_map(|rest| rest.split_whitespace().next()?.parse().ok())
        .collect()
}

/// The kept side of a cut is a *contiguous* suffix of the conversation, ending at the live turn.
///
/// The assertion the obvious ones miss. Chaining the summary under `first_kept` instead of after
/// the leaf forks the branch, and a fork does not look like data loss from either end: the oldest
/// turns are gone because they were summarized, the newest is there because it was committed after
/// the fork — and the turns in between quietly fall off the path.
fn assert_kept_tail_is_intact(sent: &[String]) {
    let present = turns_present(sent);
    assert!(present.len() > 1, "more than the live turn is kept: {present:?}");
    let expected: Vec<usize> = (present[0]..TURNS).collect();
    assert_eq!(present, expected, "every turn since the cut is still in the conversation");
}

#[tokio::test]
async fn a_compacted_turn_sends_the_summary_instead_of_the_history() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/test").unwrap();

    let calls = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(CompactingProvider { calls: calls.clone() });
    let (action_tx, mut event_rx) =
        Engine::spawn(config(store, session_id.clone(), provider, tmp.path()));

    // Occupancy climbs with each turn; partway through it clears the threshold and compaction
    // runs before that turn's provider call.
    for turn in 0..TURNS {
        run_turn(&action_tx, &mut event_rx, bulky(&format!("TURN-{turn}"))).await;
    }

    let calls = calls.lock().await;
    let summarizations =
        calls.iter().filter(|call| CompactingProvider::is_summarization(call)).count();
    // Once. Not once per turn from there on — which is what a compaction that never reached the
    // wire produced, because the context it was meant to free never actually freed.
    assert_eq!(summarizations, 1, "compaction ran exactly once in {TURNS} turns");

    // The summarizer's prompt is a whole context window by construction, assembled once and never
    // sent again — so asking to cache it is a quarter more than sending it plainly, for nothing.
    // Every turn is the opposite: the next one extends its prefix.
    for call in calls.iter() {
        let expected = match CompactingProvider::is_summarization(call) {
            true => PromptReuse::OneOff,
            false => PromptReuse::Conversation,
        };
        assert_eq!(
            call.reuse,
            expected,
            "summarizing={}",
            CompactingProvider::is_summarization(call)
        );
    }

    let last = calls.last().expect("a final turn request");
    assert!(!CompactingProvider::is_summarization(last), "the last call is the turn itself");
    let sent = texts(last);

    // The summary leads, framed so the model reads it as a report rather than a turn to answer.
    assert!(sent[0].contains(SUMMARY_MARKER), "the summary is sent");
    assert!(sent[0].contains("<summary>"), "and says what it is: {}", &sent[0][..60]);
    assert_eq!(last.messages[0].role, Role::User, "as a user message, not a stray system turn");

    // What it replaced is gone …
    assert!(
        !sent.iter().any(|text| text.starts_with("TURN-0")),
        "the summarized turn must not also be sent"
    );
    // … and everything since the cut is there, verbatim and unbroken.
    assert_kept_tail_is_intact(&sent);
    // The whole point: the request got smaller.
    assert!(
        luxe_core::compaction::estimate_message_tokens(&last.messages) < THRESHOLD,
        "the compacted request is back under the threshold"
    );

    action_tx.send(Action::Close).await.unwrap();
}

/// `/compact` is the user overruling the threshold, and the only feedback they get is the event.
#[tokio::test]
async fn compacting_on_request_ignores_the_threshold_and_says_what_it_freed() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/test").unwrap();

    let calls = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(CompactingProvider { calls: calls.clone() });
    let (action_tx, mut event_rx) = Engine::spawn(config(store, session_id, provider, tmp.path()));

    // Enough that there is something older than the recent tail to summarize, and nowhere near
    // the threshold — auto-compaction would not fire here.
    for turn in 0..6 {
        run_turn(&action_tx, &mut event_rx, bulky(&format!("TURN-{turn}"))).await;
    }
    assert!(
        !calls.lock().await.iter().any(CompactingProvider::is_summarization),
        "nothing has tripped the threshold yet"
    );

    action_tx.send(Action::CompactNow { focus: Some("the API decisions".into()) }).await.unwrap();
    let (mut started, mut freed) = (None, None);
    while let Ok(Some(event)) =
        tokio::time::timeout(std::time::Duration::from_secs(10), event_rx.recv()).await
    {
        match event {
            Event::CompactionStarted { tokens_before } => started = Some(tokens_before),
            Event::CompactionDone { tokens_before, tokens_after } => {
                freed = Some((tokens_before, tokens_after));
                break;
            }
            _ => {}
        }
    }
    let (before, after) = freed.expect("a compaction_done event");
    assert!(before > after, "it reports what it freed: {before} -> {after}");
    // The summary call can take a minute, and without the start a client shows nothing meanwhile
    assert_eq!(started, Some(before), "a compaction_started came first, naming what it replaces");

    let calls = calls.lock().await;
    let summarization =
        calls.iter().find(|call| CompactingProvider::is_summarization(call)).expect("it ran");
    // The steer reaches the summarizer, and reaches it before the transcript — after fifty
    // thousand characters of conversation an instruction reads as more conversation.
    let prompt = texts(summarization).join("\n");
    assert!(prompt.contains("the API decisions"), "the focus is passed through");
    assert!(
        prompt.find("the API decisions") < prompt.find("TURN-0"),
        "the instruction leads the transcript it is about"
    );

    action_tx.send(Action::Close).await.unwrap();
}

/// The summary has to be the tip of the session, or a resume walks a path that misses it — and the
/// entries committed after it are stranded on the far side of a fork.
#[tokio::test]
async fn a_compacted_session_resumes_through_its_summary() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("sessions");
    let mut store = JsonlSessionStore::new(root.clone());
    let session_id = store.create("/tmp/test").unwrap();

    let calls = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(CompactingProvider { calls: calls.clone() });
    let (action_tx, mut event_rx) =
        Engine::spawn(config(store, session_id.clone(), provider, tmp.path()));
    for turn in 0..TURNS {
        run_turn(&action_tx, &mut event_rx, bulky(&format!("TURN-{turn}"))).await;
    }
    action_tx.send(Action::Close).await.unwrap();

    let reopened = JsonlSessionStore::new(root);
    let entries = reopened.load(&session_id).unwrap();
    let leaf = luxe_core::session::latest_leaf(&entries).expect("a leaf");
    let path = reopened.active_path(&session_id, &leaf).unwrap();

    // Everything ever written is still reachable — compaction is a cut, not a delete.
    let compactions = path
        .iter()
        .filter(|entry| matches!(entry.kind, EntryKind::Compaction { .. }))
        .collect::<Vec<_>>();
    assert_eq!(compactions.len(), 1, "the compaction is on the resumed path");
    assert!(
        path.iter().any(|entry| matches!(&entry.kind,
            EntryKind::Message(message) if message.content.iter().any(|block|
                matches!(block, ContentBlock::Text(text) if text.starts_with("TURN-0"))))),
        "the summarized message is kept in the log for audit and /tree"
    );

    // A `tokens_before` of zero was the old placeholder; it is what the summary replaced.
    match &compactions[0].kind {
        EntryKind::Compaction { tokens_before, .. } => {
            assert!(*tokens_before > 0, "the compaction records what it replaced")
        }
        other => panic!("expected a compaction, got {other:?}"),
    }

    // And the request that resume would build is the cut one.
    let messages = luxe_core::compaction::context_messages(&path);
    let sent: Vec<String> = messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert!(sent[0].contains(SUMMARY_MARKER));
    assert!(!sent.iter().any(|text| text.starts_with("TURN-0")));
    assert_kept_tail_is_intact(&sent);
    assert!(
        luxe_core::compaction::context_tokens(&path)
            < luxe_core::compaction::estimate_tokens(&path),
        "the resumed occupancy is the compacted one"
    );
}

/// Compacting twice must fold the previous summary in, not restart from the beginning of a session
/// whose opening messages were replaced two compactions ago.
#[tokio::test]
async fn a_second_compaction_summarizes_only_what_the_first_one_kept() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/test").unwrap();

    let mut parent = luxe_core::session::latest_leaf(&store.load(&session_id).unwrap());
    let mut ids = Vec::new();
    for index in 0..8 {
        let message = Message::user(bulky(&format!("M{index}")));
        let id = store.append(&session_id, parent, EntryKind::Message(message)).unwrap();
        parent = Some(id.clone());
        ids.push(id);
    }

    let budget = luxe_core::compaction::ContextBudget {
        context_window: CONTEXT_WINDOW as u64,
        max_tokens: MAX_TOKENS,
        observed_tokens: Some(THRESHOLD + PROMPT_TOKENS),
        ..Default::default()
    };
    let settings = settings();

    let path = store.active_path(&session_id, ids.last().unwrap()).unwrap();
    let first = luxe_core::compaction::find_compaction_point(
        &path,
        &settings,
        budget,
        luxe_core::compaction::CompactionTrigger::Threshold,
    )
    .unwrap();
    let compacted = store
        .compact(
            &session_id,
            ids.last().cloned(),
            "## Goal\nFIRST-SUMMARY\n",
            &first.first_kept,
            first.tokens_before,
        )
        .unwrap();

    // More conversation, then a second compaction.
    let mut parent = Some(compacted);
    for index in 8..14 {
        let message = Message::user(bulky(&format!("M{index}")));
        parent = Some(store.append(&session_id, parent, EntryKind::Message(message)).unwrap());
    }

    let path = store.active_path(&session_id, parent.as_ref().unwrap()).unwrap();
    let second = luxe_core::compaction::find_compaction_point(
        &path,
        &settings,
        budget,
        luxe_core::compaction::CompactionTrigger::Threshold,
    )
    .unwrap();

    // It continues the first summary rather than ignoring it …
    assert_eq!(second.previous_summary.as_deref(), Some("## Goal\nFIRST-SUMMARY\n"));
    // … and its input starts at what the first compaction kept, not at the session's first message.
    let summarized: Vec<String> = second
        .messages_to_summarize
        .iter()
        .filter_map(|entry| match &entry.kind {
            EntryKind::Message(message) => match message.content.first() {
                Some(ContentBlock::Text(text)) => {
                    text.split_whitespace().next().map(str::to_string)
                }
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert!(!summarized.is_empty(), "there is something to summarize");
    assert!(
        !summarized.contains(&"M0".to_string()),
        "the first compaction already replaced M0: {summarized:?}"
    );
}

/// A tool whose output is large enough that a handful of calls fills a window.
struct BulkyTool;

#[async_trait::async_trait]
impl luxe_core::tool::Tool for BulkyTool {
    fn name(&self) -> &str {
        "read"
    }

    fn schema(&self) -> luxe_core::provider::ToolSchema {
        luxe_core::provider::ToolSchema {
            name: "read".to_string(),
            description: "Read a file.".to_string(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        }
    }

    async fn run(
        &self,
        _args: serde_json::Value,
        _cx: &luxe_core::tool::ToolContext,
    ) -> luxe_core::tool::ToolResultData {
        luxe_core::tool::ToolResultData {
            content: vec![ContentBlock::Text(bulky("RESULT"))],
            is_error: false,
        }
    }
}

/// Calls the bulky tool over and over inside one turn, reporting the occupancy it was handed.
///
/// `overflow_first` makes the very first call fail the way an endpoint refuses an oversized
/// prompt, which is the one provider error the harness can do something about.
struct AgenticProvider {
    calls: Arc<Mutex<Vec<CompletionRequest>>>,
    iterations: usize,
    overflow_first: bool,
    refused: Arc<Mutex<bool>>,
}

#[async_trait::async_trait]
impl Provider for AgenticProvider {
    fn name(&self) -> &str {
        "agentic-mock"
    }

    async fn stream(
        &self,
        request: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        let summarizing = CompactingProvider::is_summarization(&request);
        if self.overflow_first && !summarizing {
            let mut refused = self.refused.lock().await;
            if !*refused {
                *refused = true;
                return Err(ProviderError::Other(
                    "400: prompt is too long: maximum context length exceeded".to_string(),
                ));
            }
        }
        let occupancy = occupancy_of(&request);
        let call = {
            let mut calls = self.calls.lock().await;
            calls.push(request);
            calls.len()
        };

        if summarizing {
            return Ok(Box::pin(stream::iter(vec![
                ProviderEvent::TextDelta {
                    index: 0,
                    delta: format!(
                        "## Goal\n{SUMMARY_MARKER}: keep the conversation going while the window \
                         stays under its limit.\n\n## Next Steps\nCarry on with the remaining \
                         turns."
                    ),
                },
                ProviderEvent::Done { reason: StopReason::EndTurn },
            ])));
        }

        let mut events = vec![ProviderEvent::Usage(Usage {
            prompt_tokens: occupancy,
            completion_tokens: 1,
            total_tokens: occupancy,
            ..Usage::default()
        })];
        if call <= self.iterations {
            events.extend([
                ProviderEvent::ToolCallStart {
                    index: 0,
                    id: format!("call-{call}"),
                    name: "read".to_string(),
                },
                ProviderEvent::ToolCallDelta {
                    index: 0,
                    json_delta: format!(r#"{{"path": "f{call}.rs"}}"#),
                },
                ProviderEvent::ToolCallEnd { index: 0 },
                ProviderEvent::Done { reason: StopReason::ToolCall },
            ]);
        } else {
            events.push(ProviderEvent::TextDelta { index: 0, delta: "done".to_string() });
            events.push(ProviderEvent::Done { reason: StopReason::EndTurn });
        }
        Ok(Box::pin(stream::iter(events)))
    }
}

fn agentic_config(
    store: JsonlSessionStore,
    session_id: SessionId,
    provider: Arc<dyn Provider + Send + Sync>,
    invoked_from: &std::path::Path,
) -> EngineConfig {
    let mut config = config(store, session_id, provider, invoked_from);
    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(BulkyTool));
    config.tool_registry = tools;
    config
}

/// The window fills *inside* a turn, and that is where it has to be dealt with.
///
/// The check used to run once, before the loop, so a turn asked to build a feature could spend
/// hundreds of calls growing a context nobody was watching and only compact when the user typed
/// again — on exactly the turns that need it, and on the model's last chance to have any room.
#[tokio::test]
async fn a_long_turn_compacts_without_waiting_for_the_next_message() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/test").unwrap();

    let calls = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(AgenticProvider {
        calls: calls.clone(),
        iterations: 30,
        overflow_first: false,
        refused: Arc::new(Mutex::new(false)),
    });
    let (action_tx, mut event_rx) =
        Engine::spawn(agentic_config(store, session_id, provider, tmp.path()));

    // One turn. Everything below happens without the user saying anything else.
    run_turn(&action_tx, &mut event_rx, bulky("TURN-0")).await;

    let calls = calls.lock().await;
    let summarizations =
        calls.iter().filter(|call| CompactingProvider::is_summarization(call)).count();
    assert!(summarizations >= 1, "the turn has to summarize itself: {} calls", calls.len());

    // And the point of doing it: the requests after it are back under the window.
    let last = calls.last().expect("a final request");
    assert!(!CompactingProvider::is_summarization(last));
    assert!(
        luxe_core::compaction::estimate_message_tokens(&last.messages) < THRESHOLD,
        "the turn ended under the threshold rather than over it"
    );
    assert!(texts(last).iter().any(|text| text.contains(SUMMARY_MARKER)), "the summary is sent");

    action_tx.send(Action::Close).await.unwrap();
}

/// Every figure on the way in is an estimate, so a prompt can clear the threshold and still be
/// refused. That refusal is the one the harness already knows how to fix.
#[tokio::test]
async fn a_prompt_the_provider_refuses_as_too_long_is_summarized_and_retried() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/test").unwrap();

    let calls = Arc::new(Mutex::new(Vec::new()));
    // Enough turns to leave something older than `keep_recent` worth summarizing, while staying
    // under the threshold — so the only compaction in this test is the one the refusal forces.
    let provider = Arc::new(AgenticProvider {
        calls: calls.clone(),
        iterations: 0,
        overflow_first: false,
        refused: Arc::new(Mutex::new(false)),
    });
    let (action_tx, mut event_rx) =
        Engine::spawn(agentic_config(store, session_id.clone(), provider, tmp.path()));
    for turn in 0..8 {
        run_turn(&action_tx, &mut event_rx, bulky(&format!("TURN-{turn}"))).await;
    }
    action_tx.send(Action::Close).await.unwrap();

    // Same session, a provider that refuses the next prompt exactly once.
    let store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let calls = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(AgenticProvider {
        calls: calls.clone(),
        iterations: 0,
        overflow_first: true,
        refused: Arc::new(Mutex::new(false)),
    });
    let (action_tx, mut event_rx) =
        Engine::spawn(agentic_config(store, session_id, provider, tmp.path()));
    run_turn(&action_tx, &mut event_rx, "and now this one".to_string()).await;

    let calls = calls.lock().await;
    assert!(
        calls.iter().any(CompactingProvider::is_summarization),
        "the refusal has to be answered by making room"
    );
    let last = calls.last().expect("the retry happened");
    assert!(!CompactingProvider::is_summarization(last), "and the turn ran after it");
    assert!(
        texts(last).iter().any(|text| text.contains(SUMMARY_MARKER)),
        "the retry carries the summary rather than the prompt that was refused"
    );

    action_tx.send(Action::Close).await.unwrap();
}

mod cancel;
mod refusals;
mod summaries;
