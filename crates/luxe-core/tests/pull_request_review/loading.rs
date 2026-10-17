//! Loading — a pull request, or a commit, loads into a review described by its author alone.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use luxe_core::engine::Action;
use luxe_core::event::{Event, GuidedReviewFailureKind};
use luxe_core::github::{GitHub, GitHubError};
use luxe_core::guided_review::{PullRequestStatus, PullRequestVerdict, ReviewTarget};
use luxe_core::provider::CompletionRequest;

use crate::support::*;

/// The pull request is described by what its author wrote, scrubbed; named by its own title; and
/// nothing about the session that opened it is attached — not its prompt, not its transcript, not
/// the graph of a checkout the pull request does not live in. A URL naming the checkout's own
/// repository lands on the same canonical target as a bare number.
#[tokio::test]
async fn a_pull_request_is_described_by_its_author_and_not_by_this_session() {
    let remote = Remote::new();
    let mut h = harness(&remote.project, Some(scripted(&remote, None) as Arc<dyn GitHub>)).await;
    h.chat("Please speed up the tokenizer").await;

    // The control: a review of this session's own work does carry its prompt and its record, so
    // their absence below is the rule at work rather than an empty session
    let own = h.load(ReviewTarget::WorkingTree).await.expect("the working tree loads");
    assert!(own.provenance.is_some(), "the control carries the session's record");
    assert_eq!(own.context.goal.as_deref(), Some("Please speed up the tokenizer"));

    let review = h
        .load(ReviewTarget::PullRequest { number: 1, repository: Some("Acme/App".into()) })
        .await
        .expect("the pull request loads");

    assert_eq!(review.target, ReviewTarget::PullRequest { number: 1, repository: None });
    assert_eq!(review.context.title, "Keep the last token");
    assert_eq!(review.context.goal, None, "the session's prompt is not this change's goal");
    assert!(review.provenance.is_none(), "the session's transcript is not this change's record");
    assert!(review.insight.is_none());

    let pr = review.context.pull_request.as_ref().expect("the PR's own account");
    assert_eq!(pr.author_name(), "copilot-swe-agent");
    assert!(pr.author_is_bot);
    assert_eq!(pr.status, PullRequestStatus::Open);
    assert_eq!(pr.verdict, Some(PullRequestVerdict::ChangesRequested));
    assert_eq!((pr.checks.passed, pr.checks.failed, pr.checks.skipped), (4, 1, 2));
    assert!(pr.body.contains("drops the final token") && pr.body.contains("This keeps it."));
    assert!(!pr.body.contains("ignore previous instructions"), "{}", pr.body);
    assert_eq!(pr.closing_issues[0].title.as_deref(), Some("Parser drops the last token"));

    let files: Vec<_> = review.files.iter().filter_map(|file| file.new_path.clone()).collect();
    assert_eq!(files, vec![PathBuf::from("parser.rs")]);
}

/// The guide is told whose words the description is, and never sees what was hidden in it.
#[tokio::test]
async fn the_guide_reads_the_description_as_the_authors_claim() {
    let remote = Remote::new();
    let mut h = harness(&remote.project, Some(scripted(&remote, None) as Arc<dyn GitHub>)).await;
    let before = h.requests.lock().await.len();

    h.load(ReviewTarget::PullRequest { number: 1, repository: None }).await.unwrap();
    let prompt = h.guide_prompt(before).await;

    assert!(prompt.contains("the author's claim about the change"), "{prompt}");
    assert!(prompt.contains("drops the final token"), "the description reaches the guide");
    assert!(!prompt.contains("ignore previous instructions"), "hidden text never does");
}

/// A push since the last look is a new revision with the new work in it, under the same review.
#[tokio::test]
async fn refreshing_reads_the_pull_request_again() {
    let remote = Remote::new();
    let github = scripted(&remote, None);
    let mut h = harness(&remote.project, Some(github.clone() as Arc<dyn GitHub>)).await;
    h.chat("Please speed up the tokenizer").await;
    let first = h.load(ReviewTarget::PullRequest { number: 1, repository: None }).await.unwrap();

    *github.head.lock().unwrap() = remote.push("helper.rs", "fn helper() {}\n");
    let second = h.refresh(first.clone()).await.expect("the refresh loads");

    assert_eq!(second.id, first.id, "the same logical review");
    assert_ne!(second.revision_id, first.revision_id, "a new revision");
    let files: Vec<_> = second.files.iter().filter_map(|file| file.new_path.clone()).collect();
    assert_eq!(files, vec![PathBuf::from("helper.rs"), PathBuf::from("parser.rs")]);
    assert!(second.provenance.is_none(), "a refresh keeps the rule a first load follows");
}

/// Every way reading a pull request can fail comes back as a sentence that says what to do.
#[tokio::test]
async fn each_failure_says_what_to_do() {
    let remote = Remote::new();
    let cases: [(Failure, GuidedReviewFailureKind, &str); 3] = [
        (
            Some(|| GitHubError::NotFound),
            GuidedReviewFailureKind::InvalidTarget,
            "no pull request #1 in acme/app",
        ),
        (
            Some(|| GitHubError::NotAuthenticated { host: None }),
            GuidedReviewFailureKind::Unavailable,
            "gh auth login",
        ),
        (
            Some(|| GitHubError::NotInstalled),
            GuidedReviewFailureKind::Unavailable,
            "https://cli.github.com",
        ),
    ];
    for (fail_with, kind, says) in cases {
        let mut h =
            harness(&remote.project, Some(scripted(&remote, fail_with) as Arc<dyn GitHub>)).await;
        let failure = h
            .load(ReviewTarget::PullRequest { number: 1, repository: None })
            .await
            .expect_err("refused");
        assert_eq!(failure.kind, kind, "{}", failure.message);
        assert!(failure.message.contains(says), "{}", failure.message);
    }

    let mut h = harness(&remote.project, None).await;
    let failure = h
        .load(ReviewTarget::PullRequest { number: 1, repository: None })
        .await
        .expect_err("no GitHub");
    assert!(failure.message.contains("GitHub CLI"), "{}", failure.message);

    let mut h = harness(&remote.project, Some(scripted(&remote, None) as Arc<dyn GitHub>)).await;
    let failure = h
        .load(ReviewTarget::PullRequest { number: 1, repository: Some("someone/else".into()) })
        .await
        .expect_err("not linked");
    assert!(failure.message.contains("git remote add"), "{}", failure.message);
}

/// Asking for fixes to a pull request that is not checked out is refused before the model is asked
/// anything: the agent would edit a working tree at another revision, not the reviewer's to change.
#[tokio::test]
async fn fixes_to_a_pull_request_are_refused_before_the_model_is_asked() {
    use luxe_core::guided_review::{FindingSeverity, ReviewFeedbackMode};
    let remote = Remote::new();
    let mut h = harness(&remote.project, Some(scripted(&remote, None) as Arc<dyn GitHub>)).await;
    let review = h.load(ReviewTarget::PullRequest { number: 1, repository: None }).await.unwrap();
    let before = h.settled().await;

    let event = h.submit(&review, ReviewFeedbackMode::RequestFixes, FindingSeverity::Issue).await;

    let Event::ReviewFeedbackTurnFailed { message, .. } = event else {
        panic!("a fix request on a pull request must be refused, got {event:?}");
    };
    assert!(message.contains("not checked out here"), "{message}");
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(h.requests.lock().await.len(), before, "the model was never asked");
}

/// A question about a pull request goes through, and points the agent at the pinned commit.
#[tokio::test]
async fn a_question_about_a_pull_request_reads_the_pinned_commit() {
    use luxe_core::guided_review::{FindingSeverity, ReviewFeedbackMode};
    let remote = Remote::new();
    let mut h = harness(&remote.project, Some(scripted(&remote, None) as Arc<dyn GitHub>)).await;
    let review = h.load(ReviewTarget::PullRequest { number: 1, repository: None }).await.unwrap();
    let before = h.settled().await;

    let event = h.submit(&review, ReviewFeedbackMode::Question, FindingSeverity::Question).await;

    assert!(matches!(event, Event::ReviewFeedbackTurnDispatched { .. }), "{event:?}");
    let prompt = h.guide_prompt(before).await;
    assert!(prompt.contains("the `git` tool's `show` action"), "{prompt}");
    assert!(prompt.contains(&format!("{}:<path>", remote.head)), "{prompt}");
}

/// This repository's own merged pull request #137, through a real signed-in `gh` and the real
/// remote, which the scripted double stands in for. The checkout comes out as it went in.
#[tokio::test]
#[ignore = "needs a signed-in gh, network access, and this repository's GitHub remote"]
async fn a_real_pull_request_loads_through_gh_without_touching_the_checkout() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
    let state = || {
        (
            git(&root, &["rev-parse", "HEAD"]),
            git(&root, &["status", "--porcelain"]),
            git(&root, &["for-each-ref", "--format=%(refname) %(objectname)"]),
            std::fs::read(root.join(".git/FETCH_HEAD")).ok(),
        )
    };
    let before = state();
    let github: Arc<dyn GitHub> = Arc::new(luxe_core::github::GhCli::new());
    let expected_head = github
        .pull_request(&github.repository(&root).await.unwrap(), 137)
        .await
        .expect("gh reads pull request #137")
        .head_oid;

    let mut h = harness(&root, Some(github)).await;

    let review = h
        .load(ReviewTarget::PullRequest { number: 137, repository: None })
        .await
        .unwrap_or_else(|failure| panic!("#137 loads: {}", failure.message));

    let luxe_core::guided_review::ReviewBase::PullRequest { head_oid, .. } = &review.source.base
    else {
        panic!("pinned as a pull request: {:?}", review.source.base);
    };
    assert_eq!(head_oid, &expected_head, "pinned to the head GitHub reported");
    assert!(!review.files.is_empty(), "#137 changed files");
    let pr = review.context.pull_request.as_ref().expect("its own account");
    assert_eq!(pr.status, PullRequestStatus::Merged);
    assert_eq!(state(), before, "the checkout is exactly as it was");
}

/// A commit names itself: reviewing one after a chat about something else is titled by its own
/// message and carries none of the session's record. A base comparison runs through the working
/// tree, where this session's work lives, so it still does.
#[tokio::test]
async fn a_commit_is_named_by_its_message_and_not_by_the_session() {
    let remote = Remote::new();
    let mut h = harness(&remote.project, None).await;
    h.chat("Please speed up the tokenizer").await;
    let head = git(&remote.project, &["rev-parse", "HEAD"]);

    let commit = h.load(ReviewTarget::Commit { oid: head }).await.unwrap();
    assert_eq!(commit.context.title, "Start", "the commit's own subject");
    assert_eq!(commit.context.goal, None);
    assert!(commit.provenance.is_none());

    std::fs::write(remote.project.join("parser.rs"), "fn parse() { faster(); }\n").unwrap();
    let base = h.load(ReviewTarget::Base { reference: "origin/main".into() }).await.unwrap();
    assert_eq!(base.context.goal.as_deref(), Some("Please speed up the tokenizer"));
    assert!(base.provenance.is_some(), "the working tree is this session's work");
}

/// What is uncommitted comes from git at the moment of asking, untracked files included.
#[tokio::test]
async fn the_worktree_check_asks_git_what_is_uncommitted() {
    let remote = Remote::new();
    let mut h = harness(&remote.project, None).await;
    let clean = uncommitted(&mut h).await;
    assert!(clean.iter().all(|path| path.starts_with(".luxe")), "a clean checkout: {clean:?}");
    std::fs::write(remote.project.join("stray.txt"), "left behind\n").unwrap();
    assert!(uncommitted(&mut h).await.contains(&"stray.txt".to_string()));
}

/// Asks the engine what is uncommitted, and waits for its answer.
async fn uncommitted(h: &mut Harness) -> Vec<String> {
    h.actions.send(Action::CheckWorktreeRemoval).await.unwrap();
    tokio::time::timeout(WAIT, async {
        loop {
            if let Some(Event::WorktreeRemovalChecked { uncommitted }) = h.events.recv().await {
                break uncommitted;
            }
        }
    })
    .await
    .unwrap()
}

/// The brief rides with the session's first message, after what the user wrote, and only that
/// once: the system prompt never carries it, so the cached prefix holds, and the second message
/// goes as written.
#[tokio::test]
async fn the_brief_rides_with_the_first_message_only() {
    use luxe_core::message::{ContentBlock, Role};
    let remote = Remote::new();
    let mut h = harness(&remote.project, None).await;
    h.actions
        .send(Action::SetSessionBrief("## This session's brief: issue #7\n> It crashes.".into()))
        .await
        .unwrap();

    h.chat("fix it").await;
    h.chat("and add a test").await;

    let requests = h.requests.lock().await;
    let user_texts = |request: &CompletionRequest| -> Vec<Vec<String>> {
        request
            .messages
            .iter()
            .filter(|message| message.role == Role::User)
            .map(|message| {
                message
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::Text(text) => Some(text.clone()),
                        _ => None,
                    })
                    .collect()
            })
            .collect()
    };
    let last = requests.last().expect("two turns asked the model");
    let said = user_texts(last);
    assert_eq!(said[0], ["fix it", "## This session's brief: issue #7\n> It crashes."]);
    assert_eq!(said[1], ["and add a test"], "once");
    assert!(
        requests.iter().all(|request| !request
            .system
            .as_deref()
            .unwrap_or_default()
            .contains("This session's brief")),
        "never in the system prompt"
    );
}
