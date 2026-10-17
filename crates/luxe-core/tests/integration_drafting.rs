//! Integration: a read-only turn reads, changes nothing, and answers.
//!
//! Drives the real engine with a scripted model that first tries a shell command and then answers.
//! Two turns are held to reading: a draft of a GitHub reply, and the answer to questions asked in
//! Guided Review. In both the shell must not run — refused at the dispatch choke point, which is
//! code, not the brief's instruction — and an ordinary turn after must be able to run tools again.

#![allow(clippy::unwrap_used)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use futures::stream;
use futures::stream::BoxStream;
use luxe_core::engine::{Action, DraftRequest, DraftTarget, Engine, EngineConfig};
use luxe_core::event::Event;
use luxe_core::github::overview::ConversationEntry;
use luxe_core::github::ConversationComment;
use luxe_core::message::{ContentBlock, Role};
use luxe_core::permission::{AskResolution, PermissionEngine, PermissionSettings};
use luxe_core::provider::{
    CompletionRequest, ModelInfo, Provider, ProviderError, ProviderEvent, StopReason,
};
use luxe_core::session::{JsonlSessionStore, SessionStore};
use luxe_core::tool::{Tool, ToolContext, ToolRegistry, ToolResultData};
use luxe_core::HookRegistry;
use tokio::sync::Mutex;

/// A `bash`-named tool that counts how often it actually ran.
struct CountingBash {
    runs: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl Tool for CountingBash {
    fn name(&self) -> &str {
        "bash"
    }

    fn schema(&self) -> luxe_core::provider::ToolSchema {
        luxe_core::provider::ToolSchema {
            name: "bash".to_string(),
            description: "Run a command.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": { "command": { "type": "string" } },
                "required": ["command"],
            }),
        }
    }

    async fn run(&self, _args: serde_json::Value, _cx: &ToolContext) -> ToolResultData {
        self.runs.fetch_add(1, Ordering::SeqCst);
        ToolResultData {
            content: vec![ContentBlock::Text("BASH_RAN".to_string())],
            is_error: false,
        }
    }
}

/// Odd calls try `bash`; even calls answer with a draft in its fence. Every request is kept.
struct Drafter {
    calls: Arc<Mutex<usize>>,
    requests: Arc<Mutex<Vec<CompletionRequest>>>,
}

#[async_trait::async_trait]
impl Provider for Drafter {
    fn name(&self) -> &str {
        "drafter"
    }

    async fn stream(
        &self,
        request: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        self.requests.lock().await.push(request);
        let n = {
            let mut calls = self.calls.lock().await;
            *calls += 1;
            *calls
        };
        let events = if n % 2 == 1 {
            vec![
                ProviderEvent::ToolCallStart {
                    index: 0,
                    id: format!("call_{n}"),
                    name: "bash".to_string(),
                },
                ProviderEvent::ToolCallDelta {
                    index: 0,
                    json_delta: serde_json::json!({ "command": "echo posted > /tmp/x" })
                        .to_string(),
                },
                ProviderEvent::ToolCallEnd { index: 0 },
                ProviderEvent::Usage(usage()),
                ProviderEvent::Done { reason: StopReason::ToolCall },
            ]
        } else {
            let answer =
                "Here is the reply.\n\n```luxe-draft\nFixed in `4c1e2a0`: the router reads \
                          the scheduler's schedule now.\n```";
            vec![
                ProviderEvent::TextStart { index: 0 },
                ProviderEvent::TextDelta { index: 0, delta: answer.to_string() },
                ProviderEvent::TextEnd { index: 0 },
                ProviderEvent::Usage(usage()),
                ProviderEvent::Done { reason: StopReason::EndTurn },
            ]
        };
        Ok(Box::pin(stream::iter(events)))
    }
}

/// What every scripted call reports spending.
fn usage() -> luxe_core::provider::Usage {
    luxe_core::provider::Usage {
        prompt_tokens: 1_000,
        completion_tokens: 50,
        total_tokens: 1_050,
        ..Default::default()
    }
}

fn comment(id: &str, author: &str, body: &str) -> ConversationComment {
    ConversationComment {
        id: id.into(),
        author: author.into(),
        bot: false,
        mine: false,
        body: body.into(),
        created_at: String::new(),
        url: String::new(),
    }
}

/// The text of every user message the model was sent, last request first.
fn user_texts(request: &CompletionRequest) -> Vec<String> {
    request
        .messages
        .iter()
        .filter(|message| message.role == Role::User)
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// Every tool result the model was sent.
fn tool_results(request: &CompletionRequest) -> Vec<String> {
    request
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            ContentBlock::ToolResult { content, .. } => Some(format!("{content:?}")),
            _ => None,
        })
        .collect()
}

type Spawned = (
    tokio::sync::mpsc::Sender<Action>,
    tokio::sync::mpsc::Receiver<Event>,
    Arc<AtomicUsize>,
    Arc<Mutex<Vec<CompletionRequest>>>,
    tempfile::TempDir,
);

/// The engine with a counting shell the permission engine allows outright, so the only thing that
/// can stop it is the read-only turn itself.
fn spawn() -> Spawned {
    let sessions = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(sessions.path());
    let session_id = store.create("/tmp").unwrap();
    let runs = Arc::new(AtomicUsize::new(0));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(CountingBash { runs: runs.clone() }));
    let permission = PermissionSettings {
        engine: PermissionEngine::new(Vec::new(), vec![], true),
        ask: AskResolution::AutoAllow,
    };
    let config = EngineConfig {
        session_store: std::sync::RwLock::new(Box::new(store)),
        provider: Arc::new(Drafter { calls: Arc::new(Mutex::new(0)), requests: requests.clone() }),
        providers: std::collections::HashMap::new(),
        model: ModelInfo { context_window: 200_000, ..ModelInfo::default() },
        tool_registry: tools,
        hook_registry: HookRegistry::new(),
        session_id,
        system_prompt: None,
        todo_store: None,
        compaction_settings: luxe_core::compaction::CompactionSettings::default(),
        permission,
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
        max_tool_iterations: Some(8),
        startup_notices: Vec::new(),
        action_tx: None,
    };
    let (actions, events) = Engine::spawn(config);
    (actions, events, runs, requests, sessions)
}

/// Waits for the turn in flight to end.
async fn turn_ended(events: &mut tokio::sync::mpsc::Receiver<Event>) {
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(20), events.recv()).await.unwrap()
        {
            Some(Event::TurnEnded { .. }) => break,
            Some(_) => {}
            None => panic!("engine stopped"),
        }
    }
}

#[tokio::test]
async fn a_drafting_turn_reads_changes_nothing_and_answers_with_a_draft() {
    let (actions, mut events, runs, requests, _sessions) = spawn();
    let wait = std::time::Duration::from_secs(20);

    let request = DraftRequest {
        request_id: "draft:142:1".into(),
        repository: None,
        number: 142,
        pull_request: true,
        target: DraftTarget::Comment(comment("c-jon", "jon", "Why is the fallback decided twice?")),
        conversation: vec![
            ConversationEntry::Comment(comment("c-mara", "mara", "Reads well.")),
            ConversationEntry::Comment(comment(
                "c-jon",
                "jon",
                "Why is the fallback decided twice?",
            )),
        ],
        finding: None,
        note: None,
        current: None,
    };
    actions.send(Action::DraftGithubReply(request)).await.unwrap();
    let (body, sources, error, note) = loop {
        match tokio::time::timeout(wait, events.recv()).await.unwrap() {
            Some(Event::GithubReplyDrafted { request_id, body, sources, error, note }) => {
                assert_eq!(request_id, "draft:142:1");
                break (body, sources, error, note);
            }
            Some(_) => {}
            None => panic!("engine stopped"),
        }
    };
    assert_eq!(error, None);
    assert_eq!(
        body.as_deref(),
        Some("Fixed in `4c1e2a0`: the router reads the scheduler's schedule now.")
    );
    assert_eq!(sources, ["this session", "the conversation"]);
    assert_eq!(note.as_deref(), Some("Here is the reply."), "what it said beside the draft");
    assert_eq!(runs.load(Ordering::SeqCst), 0, "the shell never ran while drafting");

    {
        let requests = requests.lock().await;
        let brief = user_texts(&requests[0]).join("\n");
        assert!(brief.contains("> Why is the fallback decided twice?"), "the comment, quoted");
        assert!(brief.contains("@mara: Reads well."), "the conversation's digest");
        assert!(brief.contains("```luxe-draft"));
        let refused = tool_results(&requests[1]).join("\n");
        assert!(refused.contains("drafts a reply and changes nothing"), "{refused}");
        assert!(brief.contains("The shell is off for this turn"));
    }

    // An ordinary turn after is not held to reading
    actions
        .send(Action::SubmitUserMessage {
            content: "Go on.".into(),
            images: vec![],
            remediation: None,
        })
        .await
        .unwrap();
    loop {
        match tokio::time::timeout(wait, events.recv()).await.unwrap() {
            Some(Event::TurnEnded { .. }) => break,
            Some(_) => {}
            None => panic!("engine stopped"),
        }
    }
    assert_eq!(runs.load(Ordering::SeqCst), 1, "the next turn runs tools as before");
}

/// A review's drafted summary is billed to the review, every call of it; a reply drafted from a
/// pull request's page and the turn after it are the agent's.
#[tokio::test]
async fn a_review_summary_draft_is_billed_to_the_review() {
    let (actions, mut events, _runs, _requests, _sessions) = spawn();
    let wait = std::time::Duration::from_secs(20);
    let request = DraftRequest {
        request_id: "draft:review:4:1".into(),
        repository: Some("acme/app".into()),
        number: 4,
        pull_request: true,
        target: DraftTarget::Review(luxe_core::engine::ReviewDraft {
            verdict: "Approve".into(),
            comments: Vec::new(),
            guide: None,
        }),
        conversation: Vec::new(),
        finding: None,
        note: None,
        current: None,
    };
    actions.send(Action::DraftGithubReply(request)).await.unwrap();
    let mut billed = Vec::new();
    loop {
        match tokio::time::timeout(wait, events.recv()).await.unwrap() {
            Some(Event::TurnUsage { source, .. }) => billed.push(source),
            Some(Event::GithubReplyDrafted { .. }) => break,
            Some(_) => {}
            None => panic!("engine stopped"),
        }
    }
    assert_eq!(billed, [luxe_core::usage::UsageSource::Review; 2], "both calls, the review's");

    actions
        .send(Action::SubmitUserMessage {
            content: "Go on.".into(),
            images: vec![],
            remediation: None,
        })
        .await
        .unwrap();
    let mut after = Vec::new();
    loop {
        match tokio::time::timeout(wait, events.recv()).await.unwrap() {
            Some(Event::TurnUsage { source, .. }) => after.push(source),
            Some(Event::TurnEnded { .. }) => break,
            Some(_) => {}
            None => panic!("engine stopped"),
        }
    }
    assert!(!after.is_empty());
    assert!(
        after.iter().all(|source| *source == luxe_core::usage::UsageSource::Agent),
        "the next turn is the agent's again: {after:?}"
    );
}

/// A question asked in Guided Review is answered and never acted on, whatever the model tries; a
/// fix request from the same review runs tools as it always did.
#[tokio::test]
async fn a_question_from_a_review_is_answered_and_never_acted_on() {
    let (actions, mut events, runs, requests, _sessions) = spawn();
    let asked = luxe_core::guided_review::SubmissionId::new();
    let remediation = |answers_only| luxe_core::engine::RemediationTurnContext {
        submission_id: Some(asked.clone()),
        review_id: luxe_core::guided_review::ReviewId::new(),
        revision_id: luxe_core::guided_review::ReviewRevisionId::from_canonical_bytes(b"review"),
        finding_ids: Vec::new(),
        answers_only,
        reports: false,
    };
    let submit = |content: &str, answers_only| Action::SubmitUserMessage {
        content: content.into(),
        images: vec![],
        remediation: Some(remediation(answers_only)),
    };

    actions.send(submit("Why is the fallback decided twice?", true)).await.unwrap();
    let (mut answered, mut announced) = (None, Vec::new());
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(20), events.recv()).await.unwrap()
        {
            Some(Event::ReviewQuestionsAnswered { submission_id, answer }) => {
                answered = Some((submission_id, answer));
            }
            Some(Event::MessageCommitted { message, .. })
                if message.role == luxe_core::message::Role::User =>
            {
                announced.push(message.text());
            }
            Some(Event::TurnEnded { .. }) => break,
            Some(_) => {}
            None => panic!("engine stopped"),
        }
    }
    // Nobody typed it, so no front-end has shown it: the engine says it was sent
    assert_eq!(announced, ["Why is the fallback decided twice?"]);
    let (submission_id, answer) = answered.expect("the answer goes to the questions");
    assert_eq!(submission_id, asked);
    assert!(answer.starts_with("Here is the reply."), "the turn's final reply: {answer}");
    assert_eq!(runs.load(Ordering::SeqCst), 0, "the shell never ran while answering");
    {
        let requests = requests.lock().await;
        let refused = tool_results(&requests[1]).join("\n");
        assert!(
            refused.contains("answers the reviewer's questions and changes nothing"),
            "{refused}"
        );
    }

    actions.send(submit("Fix R1.", false)).await.unwrap();
    turn_ended(&mut events).await;
    assert_eq!(runs.load(Ordering::SeqCst), 1, "a fix request runs tools");
}
