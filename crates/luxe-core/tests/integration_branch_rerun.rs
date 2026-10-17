//! Integration: edit-and-resend, retry and regenerate put the right conversation in front of the
//! model, and run as ordinary turns.
//!
//! Each test reads what the provider was actually sent, because that is the whole contract: an
//! edit replaces the message it edits, a regeneration replaces the answer, and nothing is ever
//! sent twice in a row. The last test is the reason these three run through the same door as a
//! submitted message: run inline, a permission prompt inside one waited on an answer that nothing
//! read, and an interrupt never arrived either.

#![allow(clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use futures::stream;
use futures::stream::BoxStream;
use luxe_core::compaction::CompactionSettings;
use luxe_core::engine::{Action, Engine, EngineConfig};
use luxe_core::event::Event;
use luxe_core::message::{ContentBlock, Role};
use luxe_core::permission::{
    AskResolution, PermValue, PermissionAction, PermissionConfig, PermissionEngine,
    PermissionReply, PermissionSettings,
};
use luxe_core::provider::{
    CompletionRequest, ModelInfo, Provider, ProviderError, ProviderEvent, StopReason,
};
use luxe_core::session::{JsonlSessionStore, SessionStore};
use luxe_core::tool::{Tool, ToolContext, ToolRegistry, ToolResultData};
use luxe_core::HookRegistry;
use tokio::sync::mpsc;

/// Every conversation the model was sent, each as its messages' roles and text.
type Sent = Arc<Mutex<Vec<Vec<(Role, String)>>>>;

/// One scripted reply: text, or a call to the `bash` tool.
#[derive(Clone)]
enum Reply {
    Text(&'static str),
    Bash,
    Fail,
}

/// Answers from a script, in order, and keeps every conversation it was sent.
struct Scripted {
    script: Mutex<Vec<Reply>>,
    sent: Sent,
}

#[async_trait::async_trait]
impl Provider for Scripted {
    fn name(&self) -> &str {
        "scripted"
    }

    async fn stream(
        &self,
        req: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        let conversation = req
            .messages
            .iter()
            .map(|message| {
                let text = message
                    .content
                    .iter()
                    .map(|block| match block {
                        ContentBlock::Text(text) => text.clone(),
                        ContentBlock::ToolCall { name, .. } => format!("<call {name}>"),
                        ContentBlock::ToolResult { .. } => "<result>".to_string(),
                        _ => String::new(),
                    })
                    .collect::<String>();
                (message.role.clone(), text)
            })
            .collect();
        self.sent.lock().unwrap().push(conversation);
        let next = {
            let mut script = self.script.lock().unwrap();
            if script.is_empty() {
                Reply::Text("done")
            } else {
                script.remove(0)
            }
        };
        let events = match next {
            Reply::Text(text) => vec![
                ProviderEvent::TextDelta { index: 0, delta: text.to_string() },
                ProviderEvent::Done { reason: StopReason::EndTurn },
            ],
            Reply::Bash => vec![
                ProviderEvent::ToolCallStart {
                    index: 0,
                    id: "call_bash".to_string(),
                    name: "bash".to_string(),
                },
                ProviderEvent::ToolCallDelta {
                    index: 0,
                    json_delta: r#"{"command":"git commit -m more"}"#.to_string(),
                },
                ProviderEvent::ToolCallEnd { index: 0 },
                ProviderEvent::Done { reason: StopReason::ToolCall },
            ],
            Reply::Fail => return Err(ProviderError::Auth("the key was refused".to_string())),
        };
        Ok(Box::pin(stream::iter(events)))
    }
}

/// A `bash`-named tool that never needs to run in these tests.
struct Bash;

#[async_trait::async_trait]
impl Tool for Bash {
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
            }),
        }
    }

    async fn run(&self, _args: serde_json::Value, _cx: &ToolContext) -> ToolResultData {
        ToolResultData { content: vec![ContentBlock::Text("ran".to_string())], is_error: false }
    }
}

struct Session {
    actions: mpsc::Sender<Action>,
    events: mpsc::Receiver<Event>,
    sent: Sent,
}

/// An engine whose model answers from `script`, asking before every `bash` call.
fn session(script: Vec<Reply>) -> Session {
    let dir = tempfile::tempdir().unwrap().keep();
    let mut store = JsonlSessionStore::new(dir.join("sessions"));
    let session_id = store.create("/tmp/branch-rerun").unwrap();
    let sent = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(Scripted { script: Mutex::new(script), sent: sent.clone() });
    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(Bash));
    let asks = PermissionConfig::Keyed(
        [("bash".to_string(), PermValue::Scalar(PermissionAction::Ask))].into_iter().collect(),
    )
    .to_ruleset();
    let config = EngineConfig {
        session_store: std::sync::RwLock::new(Box::new(store)),
        provider,
        providers: std::collections::HashMap::new(),
        model: ModelInfo::default(),
        tool_registry: tools,
        hook_registry: HookRegistry::new(),
        session_id,
        system_prompt: None,
        todo_store: None,
        compaction_settings: CompactionSettings::default(),
        permission: PermissionSettings {
            engine: PermissionEngine::new(asks, vec![], false),
            ask: AskResolution::Prompt,
        },
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
    Session { actions, events, sent }
}

impl Session {
    /// Sends `action` and waits for the turn it starts to end, answering any prompt with `reply`.
    async fn turn(&mut self, action: Action, reply: PermissionReply) {
        self.actions.send(action).await.unwrap();
        loop {
            let event = tokio::time::timeout(std::time::Duration::from_secs(5), self.events.recv())
                .await
                .expect("the turn ended rather than hanging")
                .expect("the engine is running");
            match event {
                Event::PermissionRequested { id, .. } => {
                    self.actions.send(Action::RespondPermission { id, reply }).await.unwrap();
                }
                Event::TurnEnded { .. } => return,
                _ => {}
            }
        }
    }

    async fn say(&mut self, text: &str) {
        let submit = Action::SubmitUserMessage {
            content: text.to_string(),
            images: Vec::new(),
            remediation: None,
        };
        self.turn(submit, PermissionReply::Reject).await;
    }

    /// The conversation the last provider call carried, user and assistant text only.
    fn last_sent(&self) -> Vec<(Role, String)> {
        self.sent.lock().unwrap().last().cloned().unwrap_or_default()
    }
}

fn user(text: &str) -> (Role, String) {
    (Role::User, text.to_string())
}

fn assistant(text: &str) -> (Role, String) {
    (Role::Assistant, text.to_string())
}

/// An edit replaces the message it edits: the model sees the edited message and not the original
/// or its reply, which it used to answer as a pair.
#[tokio::test]
async fn an_edit_replaces_the_message_it_edits() {
    let mut session = session(vec![Reply::Text("first answer"), Reply::Text("second answer")]);
    session.say("teh question").await;
    session
        .turn(Action::EditResend { content: "the question".into() }, PermissionReply::Reject)
        .await;
    assert_eq!(session.last_sent(), [user("the question")]);

    // And the conversation goes on from the edit
    session.say("and then").await;
    assert_eq!(
        session.last_sent(),
        [user("the question"), assistant("second answer"), user("and then")]
    );
}

/// Regenerating replaces the answer, and the question goes in once.
#[tokio::test]
async fn a_regeneration_replaces_the_answer_and_asks_once() {
    let mut session = session(vec![Reply::Text("first answer")]);
    session.say("the question").await;
    session.turn(Action::Regenerate, PermissionReply::Reject).await;
    assert_eq!(session.last_sent(), [user("the question")]);
}

/// Asking again after an answer puts the question after it, once.
#[tokio::test]
async fn asking_again_follows_the_answer() {
    let mut session = session(vec![Reply::Text("first answer")]);
    session.say("the question").await;
    session.turn(Action::Retry, PermissionReply::Reject).await;
    assert_eq!(
        session.last_sent(),
        [user("the question"), assistant("first answer"), user("the question")]
    );
}

/// Asking again after a turn that failed asks in place, rather than sending the question twice in
/// a row.
#[tokio::test]
async fn asking_again_after_a_failure_asks_in_place() {
    let mut session = session(vec![Reply::Fail]);
    session.say("the question").await;
    session.turn(Action::Retry, PermissionReply::Reject).await;
    assert_eq!(session.last_sent(), [user("the question")]);
}

/// A permission prompt inside an edited turn is answered, and the turn ends. Run inline, the
/// engine read no actions until the turn was over, so the answer never arrived and the turn waited
/// on it for good.
#[tokio::test]
async fn a_prompt_inside_an_edited_turn_is_answered() {
    let mut session =
        session(vec![Reply::Text("first answer"), Reply::Bash, Reply::Text("left it")]);
    session.say("commit it").await;
    session
        .turn(Action::EditResend { content: "commit it now".into() }, PermissionReply::Reject)
        .await;
    let sent = session.last_sent();
    assert_eq!(sent.first(), Some(&user("commit it now")));
    assert!(sent.iter().any(|(_, text)| text == "<result>"), "the refusal reached the model");
}
