//! GitHub surface — the `/github` sections and overviews, answered through the engine.

use std::path::Path;
use std::sync::Arc;

use luxe_core::engine::Action;
use luxe_core::event::Event;
use luxe_core::github::GitHub;
use luxe_core::session::SessionStore;

use crate::support::*;

/// Each section's search runs with `{repo}` as the checkout's repository, and one GitHub refuses
/// says why in its own place while the others list their rows.
#[tokio::test]
async fn github_sections_are_searched_each_on_its_own() {
    let remote = Remote::new();
    let github = scripted(&remote, None);
    let mut h = harness(&remote.project, Some(github.clone() as Arc<dyn GitHub>)).await;
    let section = |name: &str, query: &str| luxe_core::config::GithubSection {
        name: name.into(),
        query: query.into(),
    };
    let sections = vec![section("Mine", "is:pr author:@me repo:{repo}"), section("Bad", "refused")];
    h.actions.send(Action::ListGithub { sections }).await.unwrap();
    let listed = tokio::time::timeout(WAIT, async {
        loop {
            if let Some(Event::GithubListed { repository, sections }) = h.events.recv().await {
                assert_eq!(repository.as_deref(), Some("acme/app"));
                break sections;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(*github.searched.lock().unwrap(), ["is:pr author:@me repo:acme/app", "refused"]);
    assert_eq!(
        (listed[0].name.as_str(), listed[0].items.len(), &listed[0].error),
        ("Mine", 1, &None)
    );
    assert!(listed[1].items.is_empty());
    assert!(
        listed[1].error.as_deref().is_some_and(|error| error.contains("not valid")),
        "{listed:?}"
    );
    assert_eq!((listed[0].total, listed[0].next.as_deref()), (Some(31), Some("1")));
    assert_eq!((listed[1].total, &listed[1].next), (None, &None), "a refusal has no count");
}

/// A search on its own answers under the id it was asked with, `{repo}` read as the checkout's,
/// and a next page starts after the cursor it was given.
#[tokio::test]
async fn a_search_on_its_own_answers_its_id_and_follows_the_cursor() {
    let remote = Remote::new();
    let github = scripted(&remote, None);
    let mut h = harness(&remote.project, Some(github.clone() as Arc<dyn GitHub>)).await;
    let section = luxe_core::config::GithubSection {
        name: "Pull requests".into(),
        query: "is:merged is:pr repo:{repo}".into(),
    };
    let action = Action::SearchGithub { request: 9, section, after: Some("1".into()) };
    h.actions.send(action).await.unwrap();
    let (request, repository, section) = tokio::time::timeout(WAIT, async {
        loop {
            if let Some(Event::GithubSearched { request, repository, section }) =
                h.events.recv().await
            {
                break (request, repository, section);
            }
        }
    })
    .await
    .unwrap();
    assert_eq!((request, repository.as_deref()), (9, Some("acme/app")));
    assert_eq!(*github.searched.lock().unwrap(), ["is:merged is:pr repo:acme/app after:1"]);
    assert_eq!(section.name, "Pull requests");
    assert_eq!(section.items[0].number, 2, "the page after the first");
    assert_eq!((section.total, section.next.as_deref()), (Some(31), Some("2")));
}

async fn overview_read(harness: &mut Harness) -> Event {
    loop {
        match tokio::time::timeout(WAIT, harness.events.recv()).await.unwrap() {
            Some(event @ Event::GithubOverviewRead { .. }) => return event,
            Some(_) => {}
            None => panic!("engine stopped"),
        }
    }
}

/// An overview GitHub cannot find comes back as a sentence, keyed to what was asked, so `/github`
/// shows why in the detail of the row that asked rather than in some other row's.
#[tokio::test]
async fn an_overview_github_cannot_find_says_so_against_what_was_asked() {
    let remote = Remote::new();
    let github: Arc<dyn GitHub> = scripted(&remote, None);
    let mut harness = harness(&remote.project, Some(github)).await;
    let asked = Action::ReadGithubOverview {
        repository: Some("other/lib".into()),
        number: 9,
        pull_request: true,
    };
    harness.actions.send(asked).await.unwrap();
    let Event::GithubOverviewRead { repository, number, overview, error, .. } =
        overview_read(&mut harness).await
    else {
        unreachable!()
    };
    assert_eq!((repository.as_deref(), number), (Some("other/lib"), 9));
    assert!(overview.is_none());
    assert_eq!(
        error.as_deref(),
        Some("GitHub has no such pull request that this account can see.")
    );
}

/// The whole road with the real `gh`: the action, the engine, the query, the event. A public pull
/// request on another repository, which needs no remote here, only a signed-in account.
#[tokio::test]
#[ignore = "needs a signed-in gh and network access"]
async fn a_real_gh_answers_an_overview_through_the_engine() {
    let project = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let github: Arc<dyn GitHub> = Arc::new(luxe_core::github::GhCli::new());
    let mut harness = harness_on_issue(&project, Some(github), None).await;
    let asked = Action::ReadGithubOverview {
        repository: Some("cli/cli".into()),
        number: 8_474,
        pull_request: true,
    };
    harness.actions.send(asked).await.unwrap();
    let Event::GithubOverviewRead { number, overview, error, .. } =
        overview_read(&mut harness).await
    else {
        unreachable!()
    };
    assert_eq!((number, error), (8_474, None));
    let Some(luxe_core::github::overview::Overview::PullRequest(pr)) = overview else {
        panic!("a pull request")
    };
    assert_eq!(pr.repository, "cli/cli");

    let asked = Action::ReadGithubOverview {
        repository: Some("cli/cli".into()),
        number: 14_015,
        pull_request: false,
    };
    harness.actions.send(asked).await.unwrap();
    let Event::GithubOverviewRead { overview, error, .. } = overview_read(&mut harness).await
    else {
        unreachable!()
    };
    assert_eq!(error, None);
    assert!(
        matches!(overview, Some(luxe_core::github::overview::Overview::Issue(issue)) if issue.number == 14_015)
    );

    let asked = Action::ReadGithubConversation {
        repository: Some("cli/cli".into()),
        number: 13_982,
        pull_request: true,
    };
    harness.actions.send(asked).await.unwrap();
    let entries = loop {
        match tokio::time::timeout(WAIT, harness.events.recv()).await.unwrap() {
            Some(Event::GithubConversationRead { entries, error, .. }) => {
                assert_eq!(error, None);
                break entries.expect("a conversation");
            }
            Some(_) => {}
            None => panic!("engine stopped"),
        }
    };
    assert!(matches!(
        entries.first(),
        Some(luxe_core::github::overview::ConversationEntry::Opening { .. })
    ));
}

/// An issue's overview names your other session on it — the latest, with its worktree branch —
/// and never the session you are in, which there is nothing to resume.
#[tokio::test]
async fn an_issue_overview_names_your_other_session_on_it() {
    let remote = Remote::new();
    let github: Arc<dyn GitHub> = scripted(&remote, None);
    let cwd = remote.project.to_str().unwrap().to_string();
    let mut other = None;
    let mut harness = harness_with(&remote.project, Some(github), Some(318), |store| {
        let session = store.create(&cwd).unwrap();
        let leaf = luxe_core::session::latest_leaf(&store.load(&session).unwrap());
        let link = luxe_core::session::EntryKind::IssueLink {
            number: 318,
            url: "https://github.com/acme/app/issues/318".into(),
        };
        let leaf = store.append(&session, leaf, link).unwrap();
        let worktree = luxe_core::session::EntryKind::Worktree {
            branch: "318-phantom-category".into(),
            path: ".luxe/worktrees/318-phantom-category".into(),
        };
        store.append(&session, Some(leaf), worktree).unwrap();
        other = Some(session);
    })
    .await;
    let asked = Action::ReadGithubOverview { repository: None, number: 318, pull_request: false };
    harness.actions.send(asked).await.unwrap();
    let Event::GithubOverviewRead { local, error, .. } = overview_read(&mut harness).await else {
        unreachable!()
    };
    assert_eq!(error.as_deref(), Some("GitHub has no such issue that this account can see."));
    let local = local.expect("the other session that took it up");
    assert_eq!(Some(local.session), other.map(|session| session.0));
    assert_eq!(local.branch.as_deref(), Some("318-phantom-category"));

    // Elsewhere, no session here could have taken it up
    let asked = Action::ReadGithubOverview {
        repository: Some("other/lib".into()),
        number: 318,
        pull_request: false,
    };
    harness.actions.send(asked).await.unwrap();
    let Event::GithubOverviewRead { local, .. } = overview_read(&mut harness).await else {
        unreachable!()
    };
    assert!(local.is_none());
}

/// The next `GithubCommentPosted`: the thread it answered, where it landed, why it did not.
async fn comment_posted(harness: &mut Harness) -> (Option<String>, Option<String>, Option<String>) {
    loop {
        match tokio::time::timeout(WAIT, harness.events.recv()).await.unwrap() {
            Some(Event::GithubCommentPosted { thread, url, error, .. }) => {
                return (thread, url, error)
            }
            Some(_) => {}
            None => panic!("engine stopped"),
        }
    }
}

/// A reply goes into its thread as written; a comment outside one goes under a quote of the line
/// it answers; and a thread GitHub refuses comes back as a sentence, keyed to the thread.
#[tokio::test]
async fn a_reply_lands_in_its_thread_and_a_comment_quotes_what_it_answers() {
    let remote = Remote::new();
    let github = scripted(&remote, None);
    let mut harness = harness(&remote.project, Some(github.clone())).await;
    let post = |thread: Option<&str>, answering: Option<&str>, body: &str| {
        Action::PostGithubComment(luxe_core::engine::GithubPost {
            repository: None,
            number: 1,
            thread: thread.map(str::to_string),
            answering: answering.map(str::to_string),
            body: body.into(),
        })
    };
    harness.actions.send(post(Some("PRRT_1"), None, "Fixed in 4c1e2a0.")).await.unwrap();
    let (thread, url, error) = comment_posted(&mut harness).await;
    assert_eq!((thread.as_deref(), error), (Some("PRRT_1"), None));
    assert!(url.unwrap().ends_with("discussion_PRRT_1"));

    harness
        .actions
        .send(post(None, Some("\nReads well.\nThe fallback is right."), "Thanks — kept it."))
        .await
        .unwrap();
    let (_, url, _) = comment_posted(&mut harness).await;
    assert!(url.is_some());

    harness.actions.send(post(Some("refused"), None, "Done.")).await.unwrap();
    let (thread, url, error) = comment_posted(&mut harness).await;
    assert_eq!((thread.as_deref(), url), (Some("refused"), None));
    assert!(error.unwrap().contains("Thread is locked"));

    let answered = github.answered.lock().unwrap().clone();
    assert_eq!(
        answered,
        [
            "reply:PRRT_1:Fixed in 4c1e2a0.",
            "comment:1:> Reads well.\n\nThanks — kept it.",
            "reply:refused:Done.",
        ]
    );
}

/// The next `PullRequestChanged`: what changed, or why nothing did.
async fn pull_request_changed(
    harness: &mut Harness,
) -> (Option<luxe_core::engine::PullRequestChange>, Option<String>) {
    loop {
        match tokio::time::timeout(WAIT, harness.events.recv()).await.unwrap() {
            Some(Event::PullRequestChanged { change, error, .. }) => return (change, error),
            Some(_) => {}
            None => panic!("engine stopped"),
        }
    }
}

/// A change the reader made comes back as what changed: a merge with its commit and whether the
/// branch went, a draft marked ready.
#[tokio::test]
async fn a_merge_and_a_ready_come_back_as_what_changed() {
    use luxe_core::engine::{PullRequestChange, PullRequestWrite};
    use luxe_core::github::overview::{MergeMethod, MergeRequest};
    let remote = Remote::new();
    let github: Arc<dyn GitHub> = scripted(&remote, None);
    let mut harness = harness(&remote.project, Some(github)).await;
    let merge = PullRequestWrite::Merge(MergeRequest {
        method: MergeMethod::Merge,
        head_oid: "abc".into(),
        delete_branch: Some("fix-parser".into()),
    });
    harness
        .actions
        .send(Action::ChangePullRequest { repository: None, number: 1, write: merge })
        .await
        .unwrap();
    assert_eq!(
        pull_request_changed(&mut harness).await,
        (Some(PullRequestChange::Merged { sha: "merged".into(), branch_deleted: true }), None)
    );
    harness
        .actions
        .send(Action::ChangePullRequest {
            repository: None,
            number: 1,
            write: PullRequestWrite::MarkReady,
        })
        .await
        .unwrap();
    assert_eq!(
        pull_request_changed(&mut harness).await,
        (Some(PullRequestChange::MarkedReady), None)
    );
}
