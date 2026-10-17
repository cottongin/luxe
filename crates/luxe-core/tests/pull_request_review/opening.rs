//! Opening — a branch opens its pull request once, and finished work is committed first.

use std::sync::Arc;

use luxe_core::engine::Action;
use luxe_core::event::Event;
use luxe_core::github::{GitHub, PullRequestState};
use luxe_core::guided_review::{GuidedReview, ReviewTarget};
use luxe_core::session::{JsonlSessionStore, SessionStore};

use crate::support::*;

/// A session is linked to a pull request once, however often it is asked, and the link is what
/// `luxe --from-pr` finds; the conversation's own leaf is untouched by it.
#[tokio::test]
async fn a_session_is_linked_to_its_pull_request_once() {
    let remote = Remote::new();
    let mut h = harness(&remote.project, None).await;
    h.chat("fix the parser").await;
    for _ in 0..2 {
        h.actions
            .send(Action::LinkPullRequest {
                number: 146,
                url: "https://github.com/acme/app/pull/146".into(),
            })
            .await
            .unwrap();
    }
    h.actions
        .send(Action::LinkPullRequest {
            number: 7,
            url: "https://github.com/acme/app/pull/7".into(),
        })
        .await
        .unwrap();
    // Every action is handled in order, so a later chat turn means the links are written
    h.chat("and again").await;

    let store = JsonlSessionStore::new(h.sessions.path());
    let sessions = store.list_sessions();
    let entries = store.load(&sessions[0]).unwrap();
    assert_eq!(luxe_core::session::linked_pull_requests(&entries), [146, 7]);
    assert_eq!(
        luxe_core::session::session_for_pull_request(&store, &sessions, 146),
        Some(sessions[0].clone())
    );
    assert_eq!(luxe_core::session::session_for_pull_request(&store, &sessions, 8), None);
    // The second turn chained onto the first, not onto a link
    let leaf = luxe_core::session::latest_leaf(&entries).unwrap();
    let path = store.active_path(&sessions[0], &leaf).unwrap();
    let said: Vec<String> = path
        .iter()
        .filter_map(|entry| match &entry.kind {
            luxe_core::session::EntryKind::Message(message)
                if message.role == luxe_core::message::Role::User =>
            {
                Some(message.text())
            }
            _ => None,
        })
        .collect();
    assert_eq!(said, ["fix the parser", "and again"]);
}

/// The verified fixes on your own pull request are committed from its review like any other: the
/// check that the working tree still matches captures against the review's own base, without
/// resolving the pull request again. Found for real, when this was refused on a live one.
#[tokio::test]
async fn your_own_pull_requests_fixes_commit_from_its_review() {
    let remote = Remote::new();
    let (mut h, review) = own_review(&remote, scripted(&remote, None)).await;
    std::fs::write(
        remote.project.join("parser.rs"),
        "fn parse() { keep_last_token(); }\n// fixed\n",
    )
    .unwrap();
    let review = h.refresh(review).await.unwrap();
    let before = git(&remote.project, &["rev-parse", "HEAD"]);

    // One commit per chapter would stage patches counted from the merge base; refused, pointing
    // at the one that works
    let refused = commit_review(&mut h, &review, false).await.unwrap_err();
    assert!(refused.contains("s squashes"), "{refused}");
    assert_eq!(git(&remote.project, &["rev-parse", "HEAD"]), before);

    let commits = commit_review(&mut h, &review, true).await.expect("the commit is made");
    assert_eq!(commits.len(), 1);
    assert_ne!(git(&remote.project, &["rev-parse", "HEAD"]), before);
    let committed = git(&remote.project, &["show", "--format=", "--", "parser.rs"]);
    assert!(committed.contains("+// fixed"), "{committed}");
    assert!(git(&remote.project, &["status", "--porcelain", "--untracked-files=no"]).is_empty());
}

/// The pull request is drafted from the branch's review and closes the session's issue; opening
/// pushes the branch, which had never been pushed, and sends exactly what the reader confirmed.
#[tokio::test]
async fn a_branchs_review_opens_its_pull_request() {
    let remote = Remote::new();
    git(&remote.project, &["push", "--quiet", "origin", "main:release"]);
    git(&remote.project, &["fetch", "--quiet", "origin"]);
    git(&remote.project, &["switch", "--quiet", "-c", "7-keep-the-last-token"]);
    commit(&remote.project, "parser.rs", "fn parse() { keep_last_token(); }\n", "Keep it");
    let github = scripted(&remote, None);
    let mut h =
        harness_on_issue(&remote.project, Some(github.clone() as Arc<dyn GitHub>), Some(7)).await;
    h.chat("fix it").await;
    let review = h.load(ReviewTarget::Base { reference: "origin/release".into() }).await.unwrap();

    let request_id = luxe_core::guided_review::PullRequestOpenId::new();
    h.actions
        .send(Action::PreparePullRequest {
            request_id: request_id.clone(),
            review_id: review.id.clone(),
        })
        .await
        .unwrap();
    let (draft, publication, uncommitted, commits) = match h.next_opening().await {
        Event::PullRequestPrepared { pull_request, publication, uncommitted, commits, .. } => {
            (pull_request, publication, uncommitted, commits)
        }
        other => panic!("{other:?}"),
    };
    assert_eq!(
        (draft.head.as_str(), draft.base.as_str()),
        ("7-keep-the-last-token", "release"),
        "merging into the branch it was reviewed against"
    );
    assert!(draft.body.ends_with("Closes #7"), "{}", draft.body);
    assert_eq!(draft.title, "Parser drops the last token", "the issue's title, read fresh");
    assert!(!draft.draft, "the reader already reviewed it");
    assert_eq!(publication, luxe_core::github::Publication::Unpublished);
    assert_eq!((uncommitted, commits.len()), (0, 0), "committed already: nothing to write");

    let confirmed =
        luxe_core::github::NewPullRequest { body: "In my own words.\n\nCloses #7".into(), ..draft };
    h.actions
        .send(Action::OpenPullRequest {
            request_id,
            title: confirmed.title.clone(),
            body: confirmed.body.clone(),
            draft: false,
        })
        .await
        .unwrap();
    match h.next_opening().await {
        Event::PullRequestOpened { number: 151, committed, .. } => assert!(committed.is_empty()),
        other => panic!("{other:?}"),
    }
    assert_eq!(*github.opened.lock().unwrap(), [confirmed]);
    assert_eq!(
        git(&remote.project, &["rev-parse", "@{push}"]),
        git(&remote.project, &["rev-parse", "HEAD"]),
        "pushed, and tracking where it went"
    );
    let entries = JsonlSessionStore::new(h.sessions.path()).load(&h.session).unwrap();
    assert!(
        luxe_core::session::linked_pull_requests(&entries).contains(&151),
        "the session remembers the pull request it opened"
    );
}

/// On the branch it would merge into there is nothing to open, and nothing is pushed.
#[tokio::test]
async fn nothing_opens_from_the_base_branch() {
    let remote = Remote::new();
    let github = scripted(&remote, None);
    let mut h = harness(&remote.project, Some(github.clone() as Arc<dyn GitHub>)).await;
    std::fs::write(remote.project.join("parser.rs"), "fn parse() { changed(); }\n").unwrap();
    let review = h.load(ReviewTarget::Base { reference: "origin/main".into() }).await.unwrap();
    h.actions
        .send(Action::PreparePullRequest {
            request_id: luxe_core::guided_review::PullRequestOpenId::new(),
            review_id: review.id.clone(),
        })
        .await
        .unwrap();
    match h.next_opening().await {
        Event::PullRequestOpenFailed { message, .. } => {
            assert!(message.starts_with("You're on `main`"), "{message}");
        }
        other => panic!("{other:?}"),
    }
    assert!(github.opened.lock().unwrap().is_empty());
}

/// A branch with an open pull request already has one; a closed one is no reason not to open
/// another. Pushed, it opens without pushing again.
#[tokio::test]
async fn a_branch_with_an_open_pull_request_opens_no_other() {
    let remote = Remote::new();
    let github = scripted(&remote, None);
    *github.branch_pull_request.lock().unwrap() = Some(branch_pull_request(PullRequestState::Open));
    let (mut h, review) = published_branch(&remote, github.clone()).await;
    let prepare = luxe_core::guided_review::PullRequestOpenId::new();
    h.actions
        .send(Action::PreparePullRequest { request_id: prepare, review_id: review.id.clone() })
        .await
        .unwrap();
    match h.next_opening().await {
        Event::PullRequestOpenFailed { message, .. } => {
            assert!(message.starts_with("#40 is already open for this branch"), "{message}");
        }
        other => panic!("{other:?}"),
    }

    *github.branch_pull_request.lock().unwrap() =
        Some(branch_pull_request(PullRequestState::Closed));
    h.actions
        .send(Action::PreparePullRequest {
            request_id: luxe_core::guided_review::PullRequestOpenId::new(),
            review_id: review.id.clone(),
        })
        .await
        .unwrap();
    match h.next_opening().await {
        Event::PullRequestPrepared { publication, .. } => {
            assert_eq!(publication, luxe_core::github::Publication::Pushed);
        }
        other => panic!("{other:?}"),
    }
}

/// An answer that never arrived is looked for before anything is said: the pull request that
/// landed is the one opened, and without one the reader is told to look before trying again.
#[tokio::test]
async fn an_opening_whose_answer_is_lost_is_found_not_repeated() {
    let remote = Remote::new();
    let github = scripted(&remote, None);
    github.opening_lost.store(true, std::sync::atomic::Ordering::SeqCst);
    let (mut h, review) = published_branch(&remote, github.clone()).await;
    // Prepared once; each confirm below is the same preview tried again after a failure
    let request_id = luxe_core::guided_review::PullRequestOpenId::new();
    h.actions
        .send(Action::PreparePullRequest {
            request_id: request_id.clone(),
            review_id: review.id.clone(),
        })
        .await
        .unwrap();
    assert!(matches!(h.next_opening().await, Event::PullRequestPrepared { .. }));
    let open = |h: &Harness| {
        let actions = h.actions.clone();
        let request_id = request_id.clone();
        async move {
            actions
                .send(Action::OpenPullRequest {
                    request_id,
                    title: "Keep the token".into(),
                    body: "Why.".into(),
                    draft: false,
                })
                .await
                .unwrap();
        }
    };

    open(&h).await;
    match h.next_opening().await {
        Event::PullRequestOpenFailed { message, .. } => {
            assert!(message.contains("look on GitHub before trying again"), "{message}");
        }
        other => panic!("{other:?}"),
    }

    *github.branch_pull_request.lock().unwrap() =
        Some(branch_pull_request(PullRequestState::Closed));
    open(&h).await;
    assert!(
        matches!(h.next_opening().await, Event::PullRequestOpenFailed { .. }),
        "not a closed one"
    );

    *github.branch_pull_request.lock().unwrap() = Some(branch_pull_request(PullRequestState::Open));
    open(&h).await;
    match h.next_opening().await {
        Event::PullRequestOpened { number: 40, .. } => {}
        other => panic!("{other:?}"),
    }
}

/// Work that was reviewed and not committed is committed first — the commits the preview named —
/// then pushed and opened, on one confirm.
#[tokio::test]
async fn reviewed_work_not_committed_is_committed_then_opened() {
    let remote = Remote::new();
    git(&remote.project, &["switch", "--quiet", "-c", "keep-token"]);
    std::fs::write(remote.project.join("parser.rs"), "fn parse() { keep_last_token(); }\n")
        .unwrap();
    let github = scripted(&remote, None);
    let mut h = harness(&remote.project, Some(github.clone() as Arc<dyn GitHub>)).await;
    let review = h.load(ReviewTarget::WorkingTree).await.unwrap();
    let prepared = luxe_core::guided_review::PullRequestOpenId::new();
    let prepare = |review: &GuidedReview| Action::PreparePullRequest {
        request_id: prepared.clone(),
        review_id: review.id.clone(),
    };

    // Staged already, it would ride along into the commit unreviewed: said before Enter
    git(&remote.project, &["add", "parser.rs"]);
    h.actions.send(prepare(&review)).await.unwrap();
    match h.next_opening().await {
        Event::PullRequestOpenFailed { message, .. } => {
            assert!(message.starts_with("Some changes are staged"), "{message}");
        }
        other => panic!("{other:?}"),
    }
    git(&remote.project, &["restore", "--staged", "parser.rs"]);

    h.actions.send(prepare(&review)).await.unwrap();
    let (draft, uncommitted, commits) = match h.next_opening().await {
        Event::PullRequestPrepared { pull_request, uncommitted, commits, .. } => {
            (pull_request, uncommitted, commits)
        }
        other => panic!("{other:?}"),
    };
    assert_eq!(uncommitted, 1);
    assert_eq!(commits.len(), review.chapters.len(), "one per chapter: {commits:?}");

    // The preview said the work is committed first, so the confirm commits it: nothing to pass
    h.actions
        .send(Action::OpenPullRequest {
            request_id: prepared.clone(),
            title: draft.title.clone(),
            body: draft.body.clone(),
            draft: false,
        })
        .await
        .unwrap();
    let committed = match h.next_opening().await {
        Event::PullRequestOpened { committed, .. } => committed,
        other => panic!("{other:?}"),
    };
    assert_eq!(committed.len(), commits.len());
    assert_eq!(git(&remote.project, &["status", "--porcelain"]), "");
    assert_eq!(git(&remote.project, &["log", "-1", "--format=%s"]), commits[commits.len() - 1]);
    assert_eq!(
        git(&remote.project, &["rev-parse", "@{push}"]),
        git(&remote.project, &["rev-parse", "HEAD"]),
        "the commits went with it"
    );
    assert_eq!(github.opened.lock().unwrap().len(), 1);
}

/// Before the session is left to fix a pull request in a worktree, the engine says whether it
/// can be: the branch it would be checked out on, or why not — a push that could not land, a pull
/// request on another repository, or one a session is already fixing.
#[tokio::test]
async fn whether_a_pull_request_can_be_fixed_here_is_asked_first() {
    let remote = Remote::new();
    let github = scripted(&remote, None);
    let mut h = harness(&remote.project, Some(github.clone() as Arc<dyn GitHub>)).await;
    let mut ask = async |repository: Option<&str>| {
        let repository = repository.map(str::to_string);
        h.actions.send(Action::CheckPullRequestFix { number: 1, repository }).await.unwrap();
        tokio::time::timeout(WAIT, async {
            loop {
                if let Some(Event::PullRequestFixChecked { number: 1, branch, refusal }) =
                    h.events.recv().await
                {
                    break (branch, refusal);
                }
            }
        })
        .await
        .unwrap()
    };
    assert_eq!(ask(None).await, (Some("fix-parser".into()), None));
    assert_eq!(ask(Some("acme/app")).await.0.as_deref(), Some("fix-parser"), "its own name");

    let (branch, refusal) = ask(Some("other/lib")).await;
    assert!(
        branch.is_none() && refusal.unwrap().contains("only be fixed from a checkout of other/lib")
    );

    *github.push.lock().unwrap() = luxe_core::github::PushAccess::MaintainerEditsOff;
    let (_, refusal) = ask(None).await;
    assert!(refusal.unwrap().contains("did not allow edits by maintainers"));

    *github.push.lock().unwrap() = luxe_core::github::PushAccess::Allowed;
    std::fs::create_dir_all(remote.project.join(".luxe/worktrees/fix-parser")).unwrap();
    let (_, refusal) = ask(None).await;
    assert!(refusal.unwrap().contains("`luxe --from-pr 1` resumes"), "one worktree per session");
}
