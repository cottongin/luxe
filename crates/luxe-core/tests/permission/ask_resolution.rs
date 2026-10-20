//! Ask resolution — deny blocks, and an ask round-trips as reject, once or always.

use luxe_core::engine::{Action, Engine};
use luxe_core::hook::HookPoint;
use luxe_core::permission::{
    AskResolution, PermValue, PermissionAction, PermissionConfig, PermissionEngine,
    PermissionReply, PermissionSettings,
};
use luxe_core::session::{JsonlSessionStore, SessionStore};
use luxe_core::HookRegistry;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::fixtures::*;

#[tokio::test]
async fn deny_blocks_the_tool() {
    let (tx, mut rx, runs) =
        spawn(bash_policy(PermissionAction::Deny, AskResolution::Prompt)).await;
    let prompts = run_turn_answering(&tx, &mut rx, "go", None).await;
    assert_eq!(prompts, 0, "deny should not prompt");
    assert_eq!(runs.load(Ordering::SeqCst), 0, "denied tool must not run");
}

#[tokio::test]
async fn ask_reject_blocks_the_tool() {
    let (tx, mut rx, runs) = spawn(bash_policy(PermissionAction::Ask, AskResolution::Prompt)).await;
    let prompts = run_turn_answering(&tx, &mut rx, "go", Some(PermissionReply::Reject)).await;
    assert_eq!(prompts, 1, "ask should prompt once");
    assert_eq!(runs.load(Ordering::SeqCst), 0, "rejected tool must not run");
}

#[tokio::test]
async fn ask_once_allows_the_tool() {
    let (tx, mut rx, runs) = spawn(bash_policy(PermissionAction::Ask, AskResolution::Prompt)).await;
    let prompts = run_turn_answering(&tx, &mut rx, "go", Some(PermissionReply::Once)).await;
    assert_eq!(prompts, 1);
    assert_eq!(runs.load(Ordering::SeqCst), 1, "approved tool must run once");
}

/// Whichever front-end answers, every other one showing the prompt hears that it closed.
///
/// The break this catches: dropping the `PromptClosed` after the reply, which leaves a second
/// front-end (a phone joined to the session) holding a prompt nobody is waiting on.
#[tokio::test]
async fn an_answered_prompt_is_announced_closed() {
    use luxe_core::Event;
    let (tx, mut rx, _) = spawn(bash_policy(PermissionAction::Ask, AskResolution::Prompt)).await;
    tx.send(Action::SubmitUserMessage { content: "go".into(), images: vec![], remediation: None })
        .await
        .unwrap();
    let (mut asked, mut closed) = (None, None);
    loop {
        let wait = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv()).await;
        match wait.expect("the turn ends").expect("the engine is running") {
            Event::PermissionRequested { id, .. } => {
                asked = Some(id.as_str().to_string());
                tx.send(Action::RespondPermission { id, reply: PermissionReply::Once })
                    .await
                    .unwrap();
            }
            Event::PromptClosed { id } => closed = Some(id),
            Event::TurnEnded { .. } => break,
            _ => {}
        }
    }
    assert!(asked.is_some(), "the policy asks");
    assert_eq!(closed, asked);
}

#[tokio::test]
async fn ask_always_persists_across_the_session() {
    let (tx, mut rx, runs) = spawn(bash_policy(PermissionAction::Ask, AskResolution::Prompt)).await;
    // First turn: approve "always" — should prompt once and run.
    let first = run_turn_answering(&tx, &mut rx, "go", Some(PermissionReply::Always)).await;
    assert_eq!(first, 1);
    // Second turn, same command family: no prompt, auto-allowed by the session approval.
    let second = run_turn_answering(&tx, &mut rx, "again", Some(PermissionReply::Reject)).await;
    assert_eq!(second, 0, "always-approved family should not prompt again");
    assert_eq!(runs.load(Ordering::SeqCst), 2, "both calls run");
}

#[tokio::test]
async fn ask_prompt_fires_the_observational_permission_hook_points() {
    let (tx, mut rx, runs, points) = spawn_with_hooks(
        bash_policy(PermissionAction::Ask, AskResolution::Prompt),
        HookRegistry::new(),
    )
    .await;
    let prompts = run_turn_answering(&tx, &mut rx, "go", Some(PermissionReply::Once)).await;
    assert_eq!(prompts, 1);
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    // The interactive prompt must bracket the wait with requested → resolved so observers (e.g.
    // the Herdr integration) can flip blocked → working around the human decision.
    assert_eq!(
        *points.lock().await,
        vec![HookPoint::PermissionRequested, HookPoint::PermissionResolved],
    );
}

#[tokio::test]
async fn auto_allow_fires_no_permission_hook_points() {
    let (tx, mut rx, runs, points) = spawn_with_hooks(
        bash_policy(PermissionAction::Ask, AskResolution::AutoAllow),
        HookRegistry::new(),
    )
    .await;
    let prompts = run_turn_answering(&tx, &mut rx, "go", None).await;
    assert_eq!(prompts, 0);
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    // No interactive prompt → the turn never blocks → no permission hook points fire.
    assert!(points.lock().await.is_empty(), "auto-allow must not fire permission hook points");
}

#[tokio::test]
async fn auto_allow_runs_without_prompting() {
    let (tx, mut rx, runs) =
        spawn(bash_policy(PermissionAction::Ask, AskResolution::AutoAllow)).await;
    let prompts = run_turn_answering(&tx, &mut rx, "go", None).await;
    assert_eq!(prompts, 0, "auto-allow never prompts");
    assert_eq!(runs.load(Ordering::SeqCst), 1, "auto-allowed tool runs");
}

/// "Allow always" writes the approval to the permissions file, so a *new* engine over that file
/// runs the same call without prompting — the session-scoped tier can't do that.
#[tokio::test]
async fn always_persist_writes_a_rule_that_survives_a_restart() {
    let tmp = tempfile::tempdir().unwrap();
    let permissions_path = tmp.path().join("permissions.json");

    // First run: bash asks, the user saves the approval.
    let store_dir = tmp.path().join("sessions-a");
    let mut store = JsonlSessionStore::new(store_dir);
    let session_id = store.create("/tmp/perm-persist").unwrap();
    let runs = Arc::new(AtomicUsize::new(0));
    let provider = Arc::new(BashCaller::running("echo hi"));
    let (tx, mut rx) = Engine::spawn(config(
        store,
        session_id,
        provider,
        runs.clone(),
        Arc::new(Mutex::new(Vec::new())),
        bash_policy(PermissionAction::Ask, AskResolution::Prompt),
        HookRegistry::new(),
        Some(permissions_path.clone()),
    ));
    let prompts =
        run_turn_answering(&tx, &mut rx, "go", Some(PermissionReply::AlwaysPersist)).await;
    assert_eq!(prompts, 1);
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    let _ = tx.send(Action::Close).await;

    // The file now carries the approval.
    let saved = std::fs::read_to_string(&permissions_path).expect("the approval was written");
    let file = luxe_core::permission::PermissionsFile::from_json(&saved).expect("valid json");
    let persisted = file.ruleset();
    assert!(!persisted.is_empty(), "a rule was recorded: {saved}");

    // Second run: the same policy plus the persisted rules — no prompt this time.
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions-b"));
    let session_id = store.create("/tmp/perm-persist").unwrap();
    let runs = Arc::new(AtomicUsize::new(0));
    let provider = Arc::new(BashCaller::running("echo hi"));
    let mut global = PermissionConfig::Keyed(
        [("bash".to_string(), PermValue::Scalar(PermissionAction::Ask))].into_iter().collect(),
    )
    .to_ruleset();
    global.extend(persisted);
    let (tx, mut rx) = Engine::spawn(config(
        store,
        session_id,
        provider,
        runs.clone(),
        Arc::new(Mutex::new(Vec::new())),
        PermissionSettings {
            engine: PermissionEngine::new(global, vec![], false),
            ask: AskResolution::Prompt,
        },
        HookRegistry::new(),
        Some(permissions_path),
    ));
    let prompts = run_turn_answering(&tx, &mut rx, "go", None).await;
    assert_eq!(prompts, 0, "the saved approval applies without asking again");
    assert_eq!(runs.load(Ordering::SeqCst), 1, "and the tool still runs");
    let _ = tx.send(Action::Close).await;
}
