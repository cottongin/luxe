//! Nested instructions — a folder's `AGENTS.md` joins the conversation when work reaches it, as an
//! append the cached prefix survives.

use std::sync::Arc;

use futures::stream::{self, BoxStream};
use luxe_core::engine::{Action, Engine};
use luxe_core::event::Event;
use luxe_core::message::ContentBlock;
use luxe_core::provider::{CompletionRequest, Provider, ProviderError, ProviderEvent, StopReason};
use luxe_core::session::{JsonlSessionStore, SessionStore};
use tokio::sync::RwLock;

use super::fixtures::*;

/// Calls one reply makes: `(tool, arguments)` each.
type Reply = Vec<(&'static str, serde_json::Value)>;

/// Answers each request with the next scripted reply, and ends the turn once the script runs out.
struct ScriptedCalls {
    replies: std::sync::Mutex<std::collections::VecDeque<Reply>>,
    requests: Arc<RwLock<Vec<CompletionRequest>>>,
}

#[async_trait::async_trait]
impl Provider for ScriptedCalls {
    fn name(&self) -> &str {
        "scripted-calls"
    }

    async fn stream(
        &self,
        request: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        let turn = self.requests.read().await.len();
        self.requests.write().await.push(request);
        let reply = self.replies.lock().unwrap().pop_front().unwrap_or_default();
        Ok(Box::pin(stream::iter(reply_events(turn, reply))))
    }
}

/// The stream for `reply`: its calls, or a plain answer when it has none.
fn reply_events(turn: usize, reply: Reply) -> Vec<ProviderEvent> {
    if reply.is_empty() {
        return vec![
            ProviderEvent::TextDelta { index: 0, delta: "done".to_string() },
            ProviderEvent::Done { reason: StopReason::EndTurn },
        ];
    }
    let mut events = vec![ProviderEvent::Start];
    for (index, (name, args)) in reply.into_iter().enumerate() {
        let id = format!("call_{turn}_{index}");
        events.push(ProviderEvent::ToolCallStart { index, id, name: name.to_string() });
        events.push(ProviderEvent::ToolCallDelta { index, json_delta: args.to_string() });
        events.push(ProviderEvent::ToolCallEnd { index });
    }
    events.push(ProviderEvent::Done { reason: StopReason::ToolCall });
    events
}

/// A checkout with root rules, `web/` rules and a file in `web/src`.
fn project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".git")).unwrap();
    std::fs::create_dir_all(dir.path().join("web/src")).unwrap();
    std::fs::write(dir.path().join("AGENTS.md"), "- ROOT-RULE\n").unwrap();
    std::fs::write(dir.path().join("web/AGENTS.md"), "- WEB-RULE\n").unwrap();
    std::fs::write(dir.path().join("web/src/app.ts"), "export {}\n").unwrap();
    dir
}

/// Runs one turn launched at `project` with `script`, and returns every request and event.
async fn run(
    project: &std::path::Path,
    script: Vec<Reply>,
) -> (Vec<CompletionRequest>, Vec<Event>) {
    run_then(project, script, Vec::new()).await
}

/// [`run`], then sends each of `after` and waits for the turn it starts to end.
async fn run_then(
    project: &std::path::Path,
    script: Vec<Reply>,
    after: Vec<Action>,
) -> (Vec<CompletionRequest>, Vec<Event>) {
    let tmp = tempfile::tempdir().unwrap();
    let (tx, mut rx, requests) = spawn(&tmp, project, script);
    let content = "go".to_string();
    tx.send(Action::SubmitUserMessage { content, images: vec![], remediation: None })
        .await
        .unwrap();
    let mut events = events_until_turn_end(&mut rx).await;
    for action in after {
        tx.send(action).await.unwrap();
        events.extend(events_until_turn_end(&mut rx).await);
    }
    tx.send(Action::Close).await.ok();
    let requests = requests.read().await.clone();
    (requests, events)
}

/// An engine launched at `project` that answers with `script`, recording what it's sent.
fn spawn(
    tmp: &tempfile::TempDir,
    project: &std::path::Path,
    script: Vec<Reply>,
) -> (
    tokio::sync::mpsc::Sender<Action>,
    tokio::sync::mpsc::Receiver<Event>,
    Arc<RwLock<Vec<CompletionRequest>>>,
) {
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let sid = store.create("/tmp/test").unwrap();
    let requests = Arc::new(RwLock::new(Vec::new()));
    let replies = std::sync::Mutex::new(script.into());
    let provider = Arc::new(ScriptedCalls { replies, requests: requests.clone() });
    let mut config = base_config(store, sid, provider);
    config.tool_registry.register(WitnessReadTool::new());
    config.invoked_from = Some(project.to_path_buf());
    let (tx, rx) = Engine::spawn(config);
    (tx, rx, requests)
}

/// A `read` of `relative` under `project`, by absolute path.
fn read(project: &std::path::Path, relative: &str) -> (&'static str, serde_json::Value) {
    ("read", serde_json::json!({ "path": project.join(relative).display().to_string() }))
}

/// How many tool results in `request` carry `needle`.
fn results_carrying(request: &CompletionRequest, needle: &str) -> usize {
    let outputs = request.messages.iter().flat_map(|message| &message.content);
    outputs
        .filter(|block| match block {
            ContentBlock::ToolResult { content, .. } => content.iter().any(|output| {
                matches!(output, luxe_core::message::ToolOutput::Text(text) if text.contains(needle))
            }),
            _ => false,
        })
        .count()
}

/// A folder's rules reach the model once, on the result of the call that reached the folder, while
/// every request keeps the same system prompt and extends the last request's messages exactly.
///
/// The break this catches: the nested file going into the system prompt, or being sent again on
/// every call into its folder. Either rewrites what the provider cached and re-bills the
/// conversation at the write rate.
#[tokio::test]
async fn a_nested_file_joins_once_and_every_request_extends_the_last() {
    let project = project();
    let p = project.path();
    let both = serde_json::json!({ "paths": [
        p.join("web/src/app.ts").display().to_string(),
        p.join("web/other.ts").display().to_string(),
    ] });
    let script = vec![
        vec![read(p, "web/src/app.ts")],
        vec![("read", both)],
        vec![read(p, "web/src/app.ts")],
    ];
    let (requests, events) = run(p, script).await;

    assert_eq!(requests.len(), 4, "three calls, then the answer");
    let system = requests[0].system.clone().unwrap_or_default();
    assert!(system.contains("ROOT-RULE") && !system.contains("WEB-RULE"), "{system}");
    for (n, request) in requests.iter().enumerate() {
        assert_eq!(request.system.as_deref().unwrap_or_default(), system, "request {n}");
    }
    assert_eq!(results_carrying(&requests[0], "WEB-RULE"), 0, "not before the folder is reached");
    assert_eq!(results_carrying(&requests[3], "WEB-RULE"), 1, "once, however often it's reached");
    assert_eq!(results_carrying(&requests[3], "[instructions: web/AGENTS.md]"), 1);
    for n in 1..requests.len() {
        let before = serde_json::to_value(&requests[n - 1].messages).unwrap();
        let after =
            serde_json::to_value(&requests[n].messages[..requests[n - 1].messages.len()]).unwrap();
        assert_eq!(before, after, "request {n} rewrote the messages request {} sent", n - 1);
    }
    let told = events
        .iter()
        .any(|event| matches!(event, Event::Notice(text) if text == "Following web/AGENTS.md"));
    assert!(told, "the person is told which file joined");
}

/// Two calls into the same folder in one reply carry its rules once, on the first.
///
/// The break this catches: checking only the committed conversation, so every call of a parallel
/// batch got its own copy before any of them committed.
#[tokio::test]
async fn one_batch_carries_a_folder_s_rules_once() {
    let project = project();
    let p = project.path();
    let script = vec![vec![read(p, "web/src/app.ts"), read(p, "web/other.ts")]];
    let (requests, _) = run(p, script).await;
    let last = requests.last().unwrap();
    assert_eq!(results_carrying(last, "WEB-RULE"), 1);
    let first =
        last.messages.iter().flat_map(|message| &message.content).find_map(|block| match block {
            ContentBlock::ToolResult { call_id, content, .. } if call_id == "call_0_0" => {
                Some(content)
            }
            _ => None,
        });
    let carried = format!("{:?}", first.unwrap());
    assert!(carried.contains("WEB-RULE"), "on the first call: {carried}");
}

/// A call that failed doesn't carry the rules; the next one into the folder that works does.
///
/// The break this catches: attaching to any call that named the folder, so the rules rode on an
/// error the model may skim past, and were then counted as delivered.
#[tokio::test]
async fn a_failed_call_leaves_the_rules_for_the_next_one() {
    let project = project();
    let p = project.path();
    let missing =
        ("missing", serde_json::json!({ "path": p.join("web/x.ts").display().to_string() }));
    let script = vec![vec![missing], vec![read(p, "web/x.ts")]];
    let (requests, _) = run(p, script).await;
    assert_eq!(results_carrying(&requests[1], "WEB-RULE"), 0, "not on the failure");
    assert_eq!(results_carrying(&requests[2], "WEB-RULE"), 1, "on the next call that worked");
}

/// After an edit branches away from the result that carried a folder's rules, the next call into
/// the folder carries them again.
///
/// The break this catches: remembering what was sent instead of reading it off the conversation,
/// so a rewind took the rules away and nothing ever brought them back.
#[tokio::test]
async fn a_rewind_that_drops_the_rules_lets_them_go_again() {
    let project = project();
    let p = project.path();
    let script = vec![vec![read(p, "web/x.ts")], vec![], vec![read(p, "web/x.ts")]];
    let edit = Action::EditResend { content: "go again".to_string() };
    let (requests, _) = run_then(p, script, vec![edit]).await;
    let retried = requests.last().unwrap();
    assert_eq!(results_carrying(&requests[1], "WEB-RULE"), 1, "the first branch had them");
    assert_eq!(results_carrying(retried, WITNESS_OUTPUT), 1, "the edit's branch dropped it");
    assert_eq!(results_carrying(retried, "WEB-RULE"), 1, "and its own call carries them");
}

/// A folder on the launch chain is the system prompt's, and a call into it adds nothing.
///
/// The break this catches: sending the launch folder's own file again as a nested one, so the
/// same rules rode twice.
#[tokio::test]
async fn the_launch_folder_s_rules_are_not_sent_again() {
    let project = project();
    let p = project.path();
    let (requests, _) = run(&p.join("web"), vec![vec![read(p, "web/src/app.ts")]]).await;
    let system = requests[0].system.clone().unwrap_or_default();
    assert!(system.contains("WEB-RULE"), "launched in web/, the prompt has it: {system}");
    assert_eq!(results_carrying(&requests[1], "[instructions: "), 0);
}
