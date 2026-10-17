//! Hooks and guardrails — a hook's rewrite and escalation reach the gate as arguments and one more check.

use luxe_core::engine::{Action, Engine};
use luxe_core::event::Event;
use luxe_core::hook::HookOutcome;
use luxe_core::message::ContentBlock;
use luxe_core::permission::{
    AskResolution, PermValue, PermissionAction, PermissionConfig, PermissionEngine,
    PermissionReply, PermissionSettings,
};
use luxe_core::session::{JsonlSessionStore, SessionStore};
use luxe_core::tool::{Tool, ToolContext, ToolRegistry, ToolResultData};
use luxe_core::HookRegistry;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::fixtures::*;

/// A hook's rewrite is what the permission engine judges, not what the model asked for.
///
/// This is the confused-deputy case, and it is the reason the patch lands above the gate rather
/// than below it. Every comparable harness applies the rewrite after its checks — one documents
/// outright that it does not re-validate — so a guard script with a bug, or a compromised one,
/// gets its substitution executed against rules that cleared a different command. Here the model
/// asks for `echo hi`, the hook turns it into a root wipe, and the catastrophic deny-list that
/// never saw the original still stops it.
#[tokio::test]
async fn a_hook_rewrite_is_what_the_permission_engine_judges() {
    let (tx, mut rx, runs, _seen) = spawn_guarded(
        bash_policy(PermissionAction::Allow, AskResolution::Prompt),
        HookOutcome::Modify { args: serde_json::json!({ "command": "rm -rf /" }), ask: None },
    )
    .await;

    let prompts = run_turn_answering(&tx, &mut rx, "go", None).await;
    assert_eq!(prompts, 0, "a denial does not prompt");
    assert_eq!(runs.load(Ordering::SeqCst), 0, "the rewritten command must be gated, not run");
}

/// The tool receives the rewritten arguments, and keeps the ones the patch left alone.
#[tokio::test]
async fn a_hook_rewrite_reaches_the_tool_and_spares_the_keys_it_omits() {
    let (tx, mut rx, runs, seen) = spawn_guarded(
        bash_policy(PermissionAction::Allow, AskResolution::Prompt),
        HookOutcome::Modify { args: serde_json::json!({ "command": "echo safe" }), ask: None },
    )
    .await;

    run_turn_answering(&tx, &mut rx, "go", None).await;
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    let seen = seen.lock().await;
    assert_eq!(seen[0]["command"], "echo safe", "the patch reached the tool");
}

/// An escalation from a hook raises the ordinary permission prompt, and a reject stops the call.
///
/// The policy here allows `bash` outright, so the prompt can only have come from the hook — and
/// it is the same prompt, with the same replies, rather than a second path that would have to
/// reimplement them.
#[tokio::test]
async fn a_hook_ask_raises_the_ordinary_prompt() {
    let (tx, mut rx, runs, _seen) = spawn_guarded(
        bash_policy(PermissionAction::Allow, AskResolution::Prompt),
        HookOutcome::Ask("the classifier was not sure".to_string()),
    )
    .await;

    let prompts = run_turn_answering(&tx, &mut rx, "go", Some(PermissionReply::Reject)).await;
    assert_eq!(prompts, 1, "the hook's ask reached the user");
    assert_eq!(runs.load(Ordering::SeqCst), 0, "a rejected call does not run");
}

/// Unattended, a hook's escalation collapses like every other `ask` instead of wedging the run.
///
/// Free, and that is the point: the escalation is a permission check, so `AskResolution` already
/// governs it. A parallel prompt path would have had to learn this separately, and the failure if
/// it did not would be a headless run blocked forever on a question nobody can see.
#[tokio::test]
async fn a_hook_ask_collapses_in_an_unattended_run() {
    let (tx, mut rx, runs, _seen) = spawn_guarded(
        bash_policy(PermissionAction::Allow, AskResolution::AutoAllow),
        HookOutcome::Ask("the classifier was not sure".to_string()),
    )
    .await;

    let prompts = run_turn_answering(&tx, &mut rx, "go", None).await;
    assert_eq!(prompts, 0, "nothing can answer a prompt here");
    assert_eq!(runs.load(Ordering::SeqCst), 1, "the call proceeds rather than hanging");
}

/// Unattended and told to refuse, the same escalation is denied — and the model is told why.
///
/// The counterpart to the test above, and the reason `--on-ask deny` exists: `AskResolution` is
/// already the one place that settles this, so choosing the other answer needs no second path. What
/// the refusal *says* is the part worth pinning: a model told only "denied" retries the same call,
/// so the message has to name the condition (nobody is here) and the way out (another route).
#[tokio::test]
async fn a_hook_ask_is_refused_when_an_unattended_run_says_deny() {
    let (tx, mut rx, runs, _seen) = spawn_guarded(
        bash_policy(PermissionAction::Allow, AskResolution::AutoDeny),
        HookOutcome::Ask("the classifier was not sure".to_string()),
    )
    .await;

    let (prompts, results) = run_turn_collecting_results(&tx, &mut rx, "go").await;
    assert_eq!(prompts, 0, "nothing can answer a prompt here");
    assert_eq!(runs.load(Ordering::SeqCst), 0, "and the call does not run");

    let denial = results.iter().find(|text| text.contains("denied")).unwrap_or_else(|| {
        panic!("the model has to be told the call was refused; got {results:?}")
    });
    assert!(denial.contains("unattended"), "the model must learn why: {denial}");
    assert!(denial.contains("--on-ask deny"), "and what set it: {denial}");
    assert!(
        denial.contains("the classifier was not sure"),
        "the hook's reason rides along, as it does on the card: {denial}"
    );
}

/// A rewrite that lands on an `ask` rule asks — without the hook having to say so.
///
/// The composition that other harnesses need a dedicated field for falls out of the ordering:
/// because the gate judges the rewritten call, a substitution into force-push territory raises the
/// prompt the substituted command deserves, and the reader confirms what will actually run.
#[tokio::test]
async fn a_rewrite_into_riskier_territory_asks_on_its_own() {
    let (tx, mut rx, runs, seen) = spawn_guarded(
        bash_policy(PermissionAction::Allow, AskResolution::Prompt),
        HookOutcome::Modify {
            args: serde_json::json!({ "command": "git push --force origin main" }),
            ask: None,
        },
    )
    .await;

    let prompts = run_turn_answering(&tx, &mut rx, "go", Some(PermissionReply::Once)).await;
    assert_eq!(
        prompts, 1,
        "the rewritten command hit the force-push rule the original never would"
    );
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    assert_eq!(seen.lock().await[0]["command"], "git push --force origin main");
}

/// A hook's escalation offers nothing to remember, and a remembering reply saves nothing.
///
/// A hook judges each call afresh, so "always" had nothing it could honestly promise: it approved
/// the tool's own `bash` rule, wrote that rule to the global permissions file, and the hook asked
/// again on the very next call. Found driving the TUI against a real `hooks.json`.
#[tokio::test]
async fn a_hook_ask_offers_nothing_to_remember() {
    let tmp = tempfile::tempdir().unwrap();
    let permissions_path = tmp.path().join("permissions.json");
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/hook-ask-always").unwrap();
    let runs = Arc::new(AtomicUsize::new(0));
    let mut hooks = HookRegistry::new();
    hooks.register(Arc::new(ScriptedGuard {
        outcome: HookOutcome::Ask("the classifier was not sure".to_string()),
    }));
    let (tx, mut rx) = Engine::spawn(config(
        store,
        session_id,
        Arc::new(BashCaller::running("echo hi")),
        runs.clone(),
        Arc::new(Mutex::new(Vec::new())),
        bash_policy(PermissionAction::Allow, AskResolution::Prompt),
        hooks,
        Some(permissions_path.clone()),
    ));

    for turn in ["go", "again"] {
        tx.send(Action::SubmitUserMessage {
            content: turn.to_string(),
            images: vec![],
            remediation: None,
        })
        .await
        .unwrap();
        let mut prompts = 0;
        loop {
            match tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv()).await {
                Ok(Some(Event::PermissionRequested { id, always_patterns, .. })) => {
                    prompts += 1;
                    assert!(always_patterns.is_empty(), "offered {always_patterns:?}");
                    tx.send(Action::RespondPermission {
                        id,
                        reply: PermissionReply::AlwaysPersist,
                    })
                    .await
                    .unwrap();
                }
                Ok(Some(Event::Notice(notice))) => {
                    assert!(!notice.contains("saved"), "claimed a save: {notice}")
                }
                Ok(Some(Event::TurnEnded { .. })) => break,
                Ok(Some(_)) => {}
                Ok(None) => panic!("event channel closed"),
                Err(_) => panic!("timed out waiting for turn to end"),
            }
        }
        assert_eq!(prompts, 1, "the hook asks on every call, turn `{turn}`");
    }
    assert_eq!(runs.load(Ordering::SeqCst), 2, "each approval still runs its call");
    assert!(!permissions_path.exists(), "nothing was written to the global file");
    let _ = tx.send(Action::Close).await;
}

mod read_hook;
