//! A restack whose push lands while the branches here cannot follow. Unix only: the pre-push hook
//! that makes it happen is a shell script.
#![cfg(unix)]

use super::*;

/// The push lands and the branches here do not follow (the checkout was edited while it ran): the
/// engine answers `Restacked` with what moved, says the branches were pushed, and never that
/// nothing was.
#[tokio::test]
async fn a_push_that_lands_before_the_checkout_can_follow_is_reported_as_landed() {
    let remote = Remote::new();
    let project = &remote.project;
    git(project, &["switch", "--quiet", "-c", "one", "origin/main"]);
    commit(project, "one.txt", "one\n", "One");
    git(project, &["push", "--quiet", "--set-upstream", "origin", "one"]);
    git(project, &["switch", "--quiet", "-c", "two"]);
    commit(project, "two.txt", "two\n", "Two");
    git(project, &["push", "--quiet", "--set-upstream", "origin", "two"]);
    git(&remote.seed, &["fetch", "--quiet", "origin", "one:one"]);
    git(&remote.seed, &["switch", "--quiet", "one"]);
    commit(&remote.seed, "fix.txt", "fixed\n", "Fix");
    git(&remote.seed, &["push", "--quiet", "origin", "one"]);

    let github = scripted(&remote, None);
    let mut status = branch_pull_request(PullRequestState::Open);
    status.number = 2;
    let below = luxe_core::github::StackEntry {
        number: 1,
        title: "One".into(),
        head_ref: "one".into(),
        draft: false,
        checked: true,
    };
    let stack = luxe_core::github::Stack {
        trunk: "main".into(),
        trunk_is_default: true,
        below: vec![below],
        above: Vec::new(),
    };
    status.stack = Some(stack);
    *github.branch_pull_request.lock().unwrap() = Some(status);
    let mut h = harness(project, Some(github.clone() as Arc<dyn GitHub>)).await;
    let planned = RestackId::new();
    h.actions.send(Action::PlanRestack { request_id: planned.clone() }).await.unwrap();
    tokio::time::timeout(WAIT, async {
        while !matches!(h.events.recv().await, Some(Event::RestackPlanned { plan: Some(_), .. })) {}
    })
    .await
    .unwrap();

    // the push runs this, after every check before it passed
    let hooks = remote.seed.parent().unwrap().join("hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    let hook = hooks.join("pre-push");
    std::fs::write(
        &hook,
        format!("#!/bin/sh\necho edited > '{}'\n", project.join("two.txt").display()),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    git(project, &["config", "core.hooksPath", hooks.to_str().unwrap()]);
    let before = git(project, &["rev-parse", "HEAD"]);

    h.actions.send(Action::Restack { request_id: planned }).await.unwrap();
    let (moved, said) = tokio::time::timeout(WAIT, async {
        loop {
            match h.events.recv().await {
                Some(Event::Restacked { moved, retarget_failed, .. }) => {
                    break (moved, retarget_failed)
                }
                Some(Event::RestackFailed { message, .. }) => panic!("{message}"),
                _ => {}
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(moved, [2]);
    let said = said.expect("what did not happen here is said");
    assert!(said.starts_with("The branches were pushed"), "{said}");
    assert!(!said.contains("Nothing was pushed"), "{said}");
    git(project, &["fetch", "--quiet", "origin"]);
    assert_ne!(git(project, &["rev-parse", "origin/two"]), before, "the push landed");
    assert_eq!(git(project, &["rev-parse", "HEAD"]), before, "the edited checkout stayed put");
}
