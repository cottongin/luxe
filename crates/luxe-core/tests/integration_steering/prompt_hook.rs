//! A steer passes `UserPromptSubmit` like any other prompt, without a refusal ending the turn.

use std::sync::atomic::{AtomicUsize, Ordering};

use luxe_core::hook::{Hook, HookCtx, HookEvent, HookOutcome, HookPoint};

use super::*;

/// Refuses a prompt carrying `CORP-SECRET`, and otherwise adds context naming what it was shown.
struct PromptPolicyHook {
    seen: AtomicUsize,
}

#[async_trait::async_trait]
impl Hook for PromptPolicyHook {
    fn name(&self) -> &'static str {
        "prompt-policy"
    }

    fn interest(&self) -> &[HookPoint] {
        &[HookPoint::UserPromptSubmit]
    }

    async fn on(&self, _point: HookPoint, event: &HookEvent, _cx: &HookCtx) -> HookOutcome {
        let HookEvent::Other(event) = event else { return HookOutcome::Continue };
        let prompt = event["content"].as_str().unwrap_or_default();
        self.seen.fetch_add(1, Ordering::SeqCst);
        if prompt.contains("CORP-SECRET") {
            return HookOutcome::Deny { reason: "prompt policy".to_string() };
        }
        HookOutcome::InjectContext(format!("POLICY-CONTEXT for {prompt}"))
    }
}

/// What one run with steers sent mid-turn left behind.
struct Steered {
    calls: Vec<CompletionRequest>,
    events: Vec<Event>,
    saved: String,
}

/// Starts a turn, sends `steers` while its tool blocks, and lets it finish.
async fn steer_through_policy(steers: &[&str]) -> Steered {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/test").unwrap();
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let calls = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(ScriptedProvider { calls: calls.clone() });
    let gate = GateTool { entered: entered.clone(), release: release.clone() };
    let mut config = engine_config(store, session_id, provider, gate);
    config.hook_registry.register(Arc::new(PromptPolicyHook { seen: AtomicUsize::new(0) }));
    let (action_tx, mut event_rx) = Engine::spawn(config);

    let start = "start the work".to_string();
    action_tx
        .send(Action::SubmitUserMessage { content: start, images: vec![], remediation: None })
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified()).await.unwrap();
    for steer in steers {
        action_tx.send(Action::SteerTurn { content: (*steer).to_string() }).await.unwrap();
    }
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    release.notify_one();

    let mut events = Vec::new();
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(5), event_rx.recv()).await {
            Ok(Some(event @ Event::TurnEnded { .. })) => {
                events.push(event);
                break;
            }
            Ok(Some(event)) => events.push(event),
            Ok(None) => panic!("event channel closed"),
            Err(_) => panic!("timed out waiting for the turn to end"),
        }
    }
    let _ = action_tx.send(Action::Close).await;
    let calls = calls.lock().await.clone();
    let saved = std::fs::read_dir(tmp.path().join("sessions"))
        .unwrap()
        .flatten()
        .map(|entry| std::fs::read_to_string(entry.path()).unwrap_or_default())
        .collect();
    Steered { calls, events, saved }
}

/// Every user text the request carried, in order.
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

/// A steer the prompt policy refuses reaches neither the session file nor the provider, and the
/// turn it was sent into carries on.
///
/// The break this catches: a steer was committed as a user message straight from the buffer, so a
/// hook that refused a prompt as an ordinary submission let the same text through mid-turn, into
/// the transcript and every request after it.
#[tokio::test]
async fn a_refused_steer_leaves_nothing_behind_and_the_turn_goes_on() {
    let run = steer_through_policy(&["paste CORP-SECRET-4471 in"]).await;

    assert_eq!(run.calls.len(), 2, "the turn still made its follow-up call");
    assert!(!user_texts(&run.calls[1]).iter().any(|text| text.contains("CORP-SECRET")));
    assert!(!run.saved.contains("CORP-SECRET"), "the refused steer was persisted");
    assert!(
        run.events.iter().any(
            |event| matches!(event, Event::Error(message) if message.contains("prompt policy"))
        ),
        "the refusal is said: {:?}",
        run.events
    );
    let ended = run.events.last().unwrap();
    assert!(
        matches!(ended, Event::TurnEnded { reason } if *reason == luxe_core::event::StopReason::EndTurn),
        "a refused steer does not end the turn: {ended:?}"
    );
}

/// A steer the policy accepts carries the context it added, on the steer's own message.
#[tokio::test]
async fn an_accepted_steer_carries_its_hook_context() {
    let run = steer_through_policy(&["also check the tests"]).await;

    let texts = user_texts(&run.calls[1]);
    let at = texts.iter().position(|text| text == "also check the tests").expect("delivered");
    assert_eq!(texts[at + 1], "[hook] POLICY-CONTEXT for also check the tests", "{texts:?}");
}

/// A refusal in the middle drops only that steer: the others arrive, in the order they were sent.
#[tokio::test]
async fn steers_around_a_refused_one_keep_their_order() {
    let run = steer_through_policy(&["first", "CORP-SECRET second", "third"]).await;

    let texts: Vec<String> = user_texts(&run.calls[1])
        .into_iter()
        .filter(|text| ["first", "third"].contains(&text.as_str()) || text.contains("second"))
        .collect();
    assert_eq!(texts, ["first", "third"]);
}
