//! Rules and modes — what an unattended run refuses, and what an allow rule or `--yolo` changes.

use luxe_core::engine::Action;
use luxe_core::permission::{
    AskResolution, PermValue, PermissionAction, PermissionConfig, PermissionEngine,
    PermissionReply, PermissionSettings,
};
use std::sync::atomic::Ordering;

use crate::fixtures::*;

/// The default policy, resolving `ask` the way an unattended run does.
fn unattended_defaults() -> PermissionSettings {
    PermissionSettings { engine: PermissionEngine::permissive(), ask: AskResolution::AutoAllow }
}

/// Nobody is watching, so a merge is refused rather than approved on nobody's behalf — and the
/// model is told why, because a bare "denied" gets the same call retried.
#[tokio::test]
async fn a_merge_is_refused_when_nobody_can_approve_it() {
    let (tx, mut rx, runs) = spawn_running(unattended_defaults(), "gh pr merge 3 --squash").await;
    let (prompts, results) = run_turn_collecting_results(&tx, &mut rx, "go").await;
    assert_eq!(prompts, 0, "nothing can answer a prompt here");
    assert_eq!(runs.load(Ordering::SeqCst), 0, "the merge must not run");
    let denial = results.iter().find(|text| text.contains("denied")).unwrap_or_else(|| {
        panic!("the model has to be told the call was refused; got {results:?}")
    });
    assert!(denial.contains("Merging a pull request"), "names the act: {denial}");
    assert!(denial.contains("nobody"), "and why: {denial}");
}

/// Opening a pull request is the loop's own workflow, so it resolves like every other `ask`.
#[tokio::test]
async fn opening_a_pull_request_still_goes_through_unattended() {
    let (tx, mut rx, runs) = spawn_running(unattended_defaults(), "gh pr create --fill").await;
    let prompts = run_turn_answering(&tx, &mut rx, "go", None).await;
    assert_eq!(prompts, 0);
    assert_eq!(runs.load(Ordering::SeqCst), 1, "the loop can open a PR");
}

/// `--auto-approve` is someone walking away, which is the same as nobody being there.
#[tokio::test]
async fn walking_away_does_not_approve_a_merge() {
    let settings =
        PermissionSettings { engine: PermissionEngine::permissive(), ask: AskResolution::Prompt };
    let (tx, mut rx, runs) = spawn_running(settings, "gh issue close 9").await;
    tx.send(Action::SetPermissionAutoApprove(true)).await.unwrap();
    let prompts = run_turn_answering(&tx, &mut rx, "go", Some(PermissionReply::Once)).await;
    assert_eq!(prompts, 0, "auto-approve never prompts");
    assert_eq!(runs.load(Ordering::SeqCst), 0, "and does not close the issue either");
}

/// An explicit `allow` rule is a person deciding ahead of time, so it gets through.
#[tokio::test]
async fn an_allow_rule_is_a_person_deciding_ahead_of_time() {
    let global = PermissionConfig::Keyed(
        [(
            "bash".to_string(),
            PermValue::Map(
                [("gh pr merge *".to_string(), PermissionAction::Allow)].into_iter().collect(),
            ),
        )]
        .into_iter()
        .collect(),
    )
    .to_ruleset();
    let settings = PermissionSettings {
        engine: PermissionEngine::new(global, vec![], false),
        ask: AskResolution::AutoAllow,
    };
    let (tx, mut rx, runs) = spawn_running(settings, "gh pr merge 3").await;
    run_turn_answering(&tx, &mut rx, "go", None).await;
    assert_eq!(runs.load(Ordering::SeqCst), 1);
}

/// `--yolo` already means everything.
#[tokio::test]
async fn yolo_still_means_everything() {
    let settings = PermissionSettings {
        engine: PermissionEngine::assemble(
            luxe_core::permission::PermissionMode::Yolo,
            vec![],
            vec![],
            false,
        ),
        ask: AskResolution::AutoAllow,
    };
    let (tx, mut rx, runs) = spawn_running(settings, "gh pr merge 3").await;
    run_turn_answering(&tx, &mut rx, "go", None).await;
    assert_eq!(runs.load(Ordering::SeqCst), 1);
}

/// With someone at the keyboard, a merge is an ordinary prompt and an approval runs it.
#[tokio::test]
async fn an_interactive_merge_is_an_ordinary_prompt() {
    let settings =
        PermissionSettings { engine: PermissionEngine::permissive(), ask: AskResolution::Prompt };
    let (tx, mut rx, runs) = spawn_running(settings, "gh pr merge 3").await;
    let prompts = run_turn_answering(&tx, &mut rx, "go", Some(PermissionReply::Once)).await;
    assert_eq!(prompts, 1);
    assert_eq!(runs.load(Ordering::SeqCst), 1);
}

/// luxe does not run commands as root, attended or not, and the refusal says how to hand the
/// command over rather than leaving the model to reword it past the rule.
#[tokio::test]
async fn running_as_root_is_refused_with_the_handoff_in_the_reply() {
    for ask in [AskResolution::Prompt, AskResolution::AutoAllow] {
        let settings = PermissionSettings { engine: PermissionEngine::permissive(), ask };
        let (tx, mut rx, runs) = spawn_running(settings, "sudo pacman -S jq").await;
        let (prompts, results) = run_turn_collecting_results(&tx, &mut rx, "go").await;
        assert_eq!(prompts, 0, "a refusal, not a question ({ask:?})");
        assert_eq!(runs.load(Ordering::SeqCst), 0, "the command must not start ({ask:?})");
        let denial = results
            .iter()
            .find(|text| text.contains("does not run commands as root"))
            .unwrap_or_else(|| panic!("the model is told why; got {results:?}"));
        assert!(denial.contains("fenced code block"), "and what to do instead: {denial}");
        assert!(denial.contains("their own terminal"), "{denial}");
    }
}

/// Each refusal says its own thing: a plain policy deny is not dressed up as a repeat or as root.
#[tokio::test]
async fn a_plain_deny_keeps_the_plain_wording() {
    let (tx, mut rx, runs) =
        spawn_running(bash_policy(PermissionAction::Deny, AskResolution::Prompt), "ls").await;
    let (_, results) = run_turn_collecting_results(&tx, &mut rx, "go").await;
    assert_eq!(runs.load(Ordering::SeqCst), 0);
    let denial = results
        .iter()
        .find(|text| text.contains("denied by policy"))
        .unwrap_or_else(|| panic!("the generic refusal; got {results:?}"));
    assert!(!denial.contains("already called"), "{denial}");
    assert!(!denial.contains("as root"), "{denial}");
}
