//! Restacking — a stack is planned from the forge and pushed as planned.

use std::sync::Arc;

use luxe_core::engine::Action;
use luxe_core::event::Event;
use luxe_core::github::{GitHub, PullRequestState};
use luxe_core::guided_review::ReviewTarget;
use luxe_core::restack::RestackId;

use crate::support::*;

/// The bottom of a two-layer stack squash-merged, its branch kept: nothing open is under the top
/// any more, so the status finds no stack, but its branch still carries the merged commits and
/// still targets the merged branch. Planning moves it onto the trunk and retargets it there; a
/// pull request on a branch nothing merged out from under is still no stack.
#[tokio::test]
async fn the_last_layer_left_after_a_merge_is_restacked_onto_the_trunk() {
    let remote = Remote::new();
    let project = &remote.project;
    git(project, &["switch", "--quiet", "-c", "one", "origin/main"]);
    let one = commit(project, "one.txt", "one\n", "One");
    git(project, &["push", "--quiet", "--set-upstream", "origin", "one"]);
    git(project, &["switch", "--quiet", "-c", "two"]);
    commit(project, "two.txt", "two\n", "Two");
    git(project, &["push", "--quiet", "--set-upstream", "origin", "two"]);
    // `one` squash-merged into main from elsewhere
    git(&remote.seed, &["fetch", "--quiet", "origin", "main:merged-main", "one:one"]);
    git(&remote.seed, &["switch", "--quiet", "merged-main"]);
    git(&remote.seed, &["merge", "--quiet", "--squash", "one"]);
    git(&remote.seed, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "One (#1)"]);
    git(&remote.seed, &["push", "--quiet", "origin", "merged-main:main"]);

    let github = scripted(&remote, None);
    let mut status = branch_pull_request(PullRequestState::Open);
    status.number = 2;
    status.stack = None;
    *github.branch_pull_request.lock().unwrap() = Some(status);
    *github.pull_request_base.lock().unwrap() = "one".into();
    let merged =
        luxe_core::restack::MergedBelow { number: 1, head_oid: one, base_ref: "main".into() };
    *github.merged.lock().unwrap() = Some(("one".into(), merged));
    let mut h = harness(project, Some(github.clone() as Arc<dyn GitHub>)).await;

    h.actions.send(Action::PlanRestack { request_id: RestackId::new() }).await.unwrap();
    let (plan, refusal) = h.next_plan().await;
    let plan = plan.unwrap_or_else(|| panic!("{refusal:?}"));
    assert_eq!(plan.steps.len(), 1);
    assert_eq!(plan.steps[0].number, 2);
    assert_eq!(plan.steps[0].retarget.as_deref(), Some("main"), "{plan:?}");

    // Nothing merged out from under it: a lone pull request, as before
    *github.merged.lock().unwrap() = None;
    h.actions.send(Action::PlanRestack { request_id: RestackId::new() }).await.unwrap();
    let (plan, refusal) = h.next_plan().await;
    assert!(plan.is_none());
    assert!(refusal.as_deref().is_some_and(|refusal| refusal.contains("is not in a stack")));
}

/// A fix pushed to the bottom of a two-layer stack: planning moves the top onto it, and restacking
/// pushes that and moves the checked-out branch with it.
#[tokio::test]
async fn a_stack_is_planned_from_the_forge_and_pushed_as_planned() {
    let remote = Remote::new();
    let project = &remote.project;
    git(project, &["switch", "--quiet", "-c", "one", "origin/main"]);
    commit(project, "one.txt", "one\n", "One");
    git(project, &["push", "--quiet", "--set-upstream", "origin", "one"]);
    git(project, &["switch", "--quiet", "-c", "two"]);
    commit(project, "two.txt", "two\n", "Two");
    git(project, &["push", "--quiet", "--set-upstream", "origin", "two"]);
    // The fix, pushed to `one` from elsewhere
    git(&remote.seed, &["fetch", "--quiet", "origin", "one:one"]);
    git(&remote.seed, &["switch", "--quiet", "one"]);
    let fixed = commit(&remote.seed, "fix.txt", "fixed\n", "Fix");
    git(&remote.seed, &["push", "--quiet", "origin", "one"]);

    let github = scripted(&remote, None);
    let mut status = branch_pull_request(PullRequestState::Open);
    status.number = 2;
    status.stack = Some(luxe_core::github::Stack {
        trunk: "main".into(),
        trunk_is_default: true,
        below: vec![luxe_core::github::StackEntry {
            number: 1,
            title: "One".into(),
            head_ref: "one".into(),
            draft: false,
            checked: true,
        }],
        above: Vec::new(),
    });
    *github.branch_pull_request.lock().unwrap() = Some(status);
    let mut h = harness(project, Some(github.clone() as Arc<dyn GitHub>)).await;

    let planned = RestackId::new();
    h.actions.send(Action::PlanRestack { request_id: planned.clone() }).await.unwrap();
    let plan = tokio::time::timeout(WAIT, async {
        loop {
            if let Some(Event::RestackPlanned { plan, refusal, .. }) = h.events.recv().await {
                break plan.unwrap_or_else(|| panic!("{refusal:?}"));
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(plan.steps.len(), 1);
    assert_eq!((plan.steps[0].number, plan.steps[0].onto.as_str()), (2, "one"));
    assert!(plan.steps[0].checked_out.is_some(), "the checkout is on it");

    // A push naming any plan but the one shown is refused, and pushes nothing
    let stranger = RestackId::new();
    h.actions.send(Action::Restack { request_id: stranger.clone() }).await.unwrap();
    let refused = tokio::time::timeout(WAIT, async {
        loop {
            match h.events.recv().await {
                Some(Event::RestackFailed { request_id, message }) => break (request_id, message),
                Some(Event::Restacked { .. }) => panic!("pushed a plan nobody was shown"),
                _ => {}
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(refused.0, stranger);
    assert!(refused.1.contains("nothing was pushed"), "{}", refused.1);
    assert_ne!(git(project, &["rev-parse", "origin/two~1"]), fixed, "nothing moved yet");

    h.actions.send(Action::Restack { request_id: planned }).await.unwrap();
    let moved = tokio::time::timeout(WAIT, async {
        loop {
            match h.events.recv().await {
                Some(Event::Restacked { moved, .. }) => break moved,
                Some(Event::RestackFailed { message, .. }) => panic!("{message}"),
                _ => {}
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(moved, [2]);
    assert_eq!(git(project, &["rev-parse", "HEAD~1"]), fixed, "the checkout sits on the fix");
    assert_eq!(git(project, &["rev-parse", "origin/two"]), git(project, &["rev-parse", "HEAD"]));
}

/// Two chapters of uncommitted work open as a stack of two: each committed, the lower one on a
/// branch of its own at its commit and the checked-out branch on top, both pushed, and the pull
/// requests opened bottom first, each on the one below. With nothing uncommitted, it refuses.
#[tokio::test]
async fn a_reviews_chapters_open_as_a_stack() {
    let remote = Remote::new();
    let project = &remote.project;
    git(project, &["push", "--quiet", "origin", "main:release"]);
    git(project, &["fetch", "--quiet", "origin"]);
    git(project, &["switch", "--quiet", "-c", "fix", "origin/release"]);
    let github = scripted(&remote, None);
    let mut h = harness(project, Some(github.clone() as Arc<dyn GitHub>)).await;

    let clean = h.load(ReviewTarget::Base { reference: "origin/release".into() }).await;
    if let Ok(review) = clean {
        h.actions
            .send(Action::PrepareStack {
                request_id: luxe_core::guided_review::PullRequestOpenId::new(),
                review_id: review.id.clone(),
            })
            .await
            .unwrap();
        let refusal = h.next_stack().await;
        assert!(matches!(&refusal, Event::StackPrepared { layers: None, .. }), "{refusal:?}");
    }

    std::fs::write(project.join("parser.rs"), "fn parse() { keep_last_token(); }\n").unwrap();
    std::fs::write(project.join("notes.md"), "Why the last token matters.\n").unwrap();
    // Against `origin/release`, which starts at HEAD here: its chapters still split, and the
    // bottom merges into `release`
    let review = h.load(ReviewTarget::Base { reference: "origin/release".into() }).await.unwrap();
    assert!(
        review.chapters.len() >= 2,
        "{:?}",
        review.chapters.iter().map(|c| &c.title).collect::<Vec<_>>()
    );

    // A branch with an open pull request already has one: no stack on top of it
    *github.branch_pull_request.lock().unwrap() = Some(branch_pull_request(PullRequestState::Open));
    h.actions
        .send(Action::PrepareStack {
            request_id: luxe_core::guided_review::PullRequestOpenId::new(),
            review_id: review.id.clone(),
        })
        .await
        .unwrap();
    match h.next_stack().await {
        Event::StackPrepared { refusal: Some(refusal), .. } => {
            assert!(refusal.starts_with("#40 is already open for this branch"), "{refusal}");
        }
        other => panic!("{other:?}"),
    }
    *github.branch_pull_request.lock().unwrap() =
        Some(branch_pull_request(PullRequestState::Closed));

    // Something staged would ride along into a chapter commit unreviewed: said before Enter
    git(project, &["add", "notes.md"]);
    h.actions
        .send(Action::PrepareStack {
            request_id: luxe_core::guided_review::PullRequestOpenId::new(),
            review_id: review.id.clone(),
        })
        .await
        .unwrap();
    match h.next_stack().await {
        Event::StackPrepared { refusal: Some(refusal), .. } => {
            assert!(refusal.starts_with("Some changes are staged"), "{refusal}");
        }
        other => panic!("{other:?}"),
    }
    git(project, &["restore", "--staged", "notes.md"]);

    let request_id = luxe_core::guided_review::PullRequestOpenId::new();
    h.actions
        .send(Action::PrepareStack { request_id: request_id.clone(), review_id: review.id.clone() })
        .await
        .unwrap();
    let layers = match h.next_stack().await {
        Event::StackPrepared { layers: Some(layers), .. } => layers,
        other => panic!("{other:?}"),
    };
    let shape: Vec<(&str, &str)> =
        layers.iter().map(|layer| (layer.head.as_str(), layer.base.as_str())).collect();
    assert_eq!(shape, [("fix-part-1", "release"), ("fix", "fix-part-1")]);

    h.actions
        .send(Action::OpenStack { request_id: request_id.clone(), draft: false })
        .await
        .unwrap();
    match h.next_stack().await {
        Event::StackOpened { numbers, failed: None, .. } => assert_eq!(numbers.len(), 2),
        other => panic!("{other:?}"),
    }
    assert_eq!(*github.opened.lock().unwrap(), layers, "opened exactly as shown, bottom first");
    let bottom = git(project, &["rev-parse", "origin/fix-part-1"]);
    assert_eq!(git(project, &["rev-parse", "origin/fix~1"]), bottom, "the top sits on the bottom");
    assert_eq!(git(project, &["rev-parse", "origin/fix"]), git(project, &["rev-parse", "HEAD"]));
    assert_eq!(git(project, &["status", "--porcelain"]), "", "all of it committed");

    // What opened is on GitHub: the same confirm again opens nothing a second time
    h.actions.send(Action::OpenStack { request_id, draft: false }).await.unwrap();
    match h.next_stack().await {
        Event::StackOpened { numbers, failed: Some(failed), .. } => {
            assert!(numbers.is_empty());
            assert!(failed.contains("already opened"), "{failed}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(github.opened.lock().unwrap().len(), 2, "nothing opened twice");
}

mod push_landed;
