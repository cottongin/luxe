//! Posting — a review goes to GitHub exactly as confirmed, and a confirm names what was shown.

use std::sync::Arc;

use luxe_core::engine::Action;
use luxe_core::event::Event;
use luxe_core::github::GitHub;
use luxe_core::guided_review::{GuidedReview, ReviewTarget};

use crate::support::*;

async fn pull_request_review(
    github: Arc<ScriptedGitHub>,
    remote: &Remote,
) -> (Harness, GuidedReview) {
    let mut h = harness(&remote.project, Some(github as Arc<dyn GitHub>)).await;
    let review = h.load(ReviewTarget::PullRequest { number: 1, repository: None }).await.unwrap();
    (h, review)
}

/// Preparing says what the preview needs: GitHub's patches, whether the author pushed since the
/// review was pinned, and whether the reader is the author.
#[tokio::test]
async fn preparing_reads_the_patches_the_head_and_whose_pull_request_it_is() {
    let remote = Remote::new();
    let github = scripted(&remote, None);
    let (mut h, review) = pull_request_review(github.clone(), &remote).await;

    let Event::PullRequestPostPrepared { patches, head_moved, own_pull_request, .. } =
        h.prepare(&review).await
    else {
        panic!("prepared");
    };
    assert_eq!(patches[0].path, "parser.rs");
    assert!(!head_moved && !own_pull_request);

    *github.head.lock().unwrap() = remote.push("later.rs", "fn later() {}\n");
    let Event::PullRequestPostPrepared { head_moved, .. } = h.prepare(&review).await else {
        panic!("prepared");
    };
    assert!(head_moved, "the author pushed after the review was pinned");

    let own = Arc::new(ScriptedGitHub {
        viewer: "APP/copilot-swe-agent".into(),
        ..Arc::into_inner(scripted(&remote, None)).unwrap()
    });
    let (mut h, review) = pull_request_review(own, &remote).await;
    let Event::PullRequestPostPrepared { own_pull_request, .. } = h.prepare(&review).await else {
        panic!("prepared");
    };
    assert!(own_pull_request, "GitHub will refuse this reader's approval");
}

/// What is posted is exactly what was handed over, anchored to the revision read.
#[tokio::test]
async fn a_review_is_posted_exactly_as_confirmed() {
    let remote = Remote::new();
    let github = scripted(&remote, None);
    let (mut h, review) = pull_request_review(github.clone(), &remote).await;

    let Event::PullRequestReviewPosted { github_review_id, url, finding_ids, .. } =
        h.post(&review, "Looks right.").await
    else {
        panic!("posted");
    };
    assert_eq!(github_review_id, 777);
    assert!(url.ends_with("pullrequestreview-777"));
    assert_eq!(finding_ids.len(), 1);
    let submitted = github.submitted.lock().unwrap();
    assert_eq!(submitted.len(), 1);
    assert_eq!(submitted[0].body, "Looks right.", "nothing added to the reader's words");
    assert_eq!(submitted[0].commit_id, remote.head);
}

/// A post whose answer never arrived is found rather than repeated.
#[tokio::test]
async fn a_post_that_landed_silently_is_found_not_repeated() {
    let remote = Remote::new();
    let github = scripted(&remote, None);
    *github.posting.lock().unwrap() = Posting::LandsSilently;
    let (mut h, review) = pull_request_review(github.clone(), &remote).await;

    let event = h.post(&review, "Looks right.").await;

    assert!(
        matches!(event, Event::PullRequestReviewPosted { github_review_id: 777, .. }),
        "{event:?}"
    );
    assert_eq!(github.submitted.lock().unwrap().len(), 1, "posted once");
}

/// A post that did not land says nothing was posted, so trying again is safe.
#[tokio::test]
async fn a_lost_post_says_nothing_was_posted() {
    let remote = Remote::new();
    let github = scripted(&remote, None);
    *github.posting.lock().unwrap() = Posting::Lost;
    let (mut h, review) = pull_request_review(github.clone(), &remote).await;

    let Event::PullRequestPostFailed { message, .. } = h.post(&review, "Looks right.").await else {
        panic!("failed");
    };
    assert!(message.contains("nothing was posted"), "{message}");
    assert_eq!(
        github.submitted.lock().unwrap().len(),
        1,
        "and it was not retried behind the reader's back"
    );
}

/// A refusal comes back as GitHub's reason, and a review of anything else cannot be posted.
#[tokio::test]
async fn a_refusal_says_why_and_only_a_pull_request_can_be_posted() {
    let remote = Remote::new();
    let github = scripted(&remote, None);
    *github.posting.lock().unwrap() = Posting::Refused;
    let (mut h, review) = pull_request_review(github, &remote).await;
    let Event::PullRequestPostFailed { message, .. } = h.post(&review, "Approved.").await else {
        panic!("failed");
    };
    assert!(message.contains("Can not approve your own pull request"), "{message}");

    let head = git(&remote.project, &["rev-parse", "HEAD"]);
    let commit = h.load(ReviewTarget::Commit { oid: head }).await.unwrap();
    let request_id = luxe_core::guided_review::PullRequestPostId::new();
    h.actions
        .send(Action::PreparePullRequestPost {
            request_id: request_id.clone(),
            review_id: commit.id.clone(),
        })
        .await
        .unwrap();
    let Event::PullRequestPostFailed { message, .. } = h.post_answer(&request_id).await else {
        panic!("failed");
    };
    assert!(message.contains("Only a pull request"), "{message}");
}

/// A review that appeared while the answer was lost is only the reader's if it is by the reader,
/// on the same commit, with the same body. Anything short of all three is someone else's, and
/// taking it as posted would mark comments sent that never were.
#[tokio::test]
async fn a_review_someone_else_posted_meanwhile_is_not_taken_for_ours() {
    let remote = Remote::new();
    let body = "Looks right.";
    let impostor = |author: &str, commit: &str, text: &str| luxe_core::github::ExistingReview {
        id: 900,
        author: author.into(),
        commit_id: commit.into(),
        body: text.into(),
        submitted_at: String::new(),
        url: String::new(),
    };
    for other in [
        impostor("someone-else", &remote.head, body),
        impostor("reader", &remote.head, "A different review."),
        impostor("reader", &"f".repeat(40), body),
    ] {
        let github = scripted(&remote, None);
        *github.posting.lock().unwrap() = Posting::LostWhileAnotherAppears(other.clone());
        let (mut h, review) = pull_request_review(github, &remote).await;
        let event = h.post(&review, body).await;
        assert!(
            matches!(event, Event::PullRequestPostFailed { .. }),
            "{other:?} is not the reader's review: {event:?}"
        );
    }
}

/// Your own pull request is compared to the working tree, not to its head: an edit you have not
/// committed is part of it, since that is where a fix to one of its threads lands. The facts and
/// the threads come along as for anyone's pull request, and nothing in the checkout moves.
#[tokio::test]
async fn your_own_pull_request_runs_to_the_working_tree() {
    let remote = Remote::new();
    git(&remote.project, &["fetch", "--quiet", "origin", "refs/pull/1/head:fix-parser"]);
    git(&remote.project, &["switch", "--quiet", "fix-parser"]);
    std::fs::write(
        remote.project.join("parser.rs"),
        "fn parse() { keep_last_token(); }\n// fixed\n",
    )
    .unwrap();
    let head_before = git(&remote.project, &["rev-parse", "HEAD"]);
    let mut h = harness(&remote.project, Some(scripted(&remote, None) as Arc<dyn GitHub>)).await;

    let review = h.load(ReviewTarget::OwnPullRequest { number: 1 }).await.unwrap();

    assert_eq!(review.target, ReviewTarget::OwnPullRequest { number: 1 });
    assert!(
        matches!(&review.source.base, luxe_core::guided_review::ReviewBase::MergeBase { oid, .. }
            if *oid == remote.base),
        "{:?}",
        review.source.base
    );
    let added: Vec<&str> = review
        .files
        .iter()
        .flat_map(|file| &file.hunks)
        .flat_map(|hunk| &hunk.lines)
        .filter(|line| line.kind == luxe_core::guided_review::ReviewLineKind::Addition)
        .map(|line| line.text.as_str())
        .collect();
    assert!(added.iter().any(|text| text.contains("// fixed")), "the uncommitted fix: {added:?}");
    assert!(
        added.iter().any(|text| text.contains("keep_last_token")),
        "the pull request's own change"
    );
    assert_eq!(review.context.pull_request.as_ref().map(|pr| pr.number), Some(1));
    assert!(review.conversation.is_some(), "its threads came along");
    assert_eq!(git(&remote.project, &["rev-parse", "HEAD"]), head_before);

    // A second edit, and a refresh, which resolves the pull request again the same way
    std::fs::write(remote.project.join("notes.rs"), "// second fix\n").unwrap();
    let refreshed = h.refresh(review).await.unwrap();
    assert_eq!(refreshed.target, ReviewTarget::OwnPullRequest { number: 1 });
    assert!(refreshed
        .files
        .iter()
        .any(|file| file.new_path.as_deref() == Some(std::path::Path::new("notes.rs"))));
}

/// Reviewing a pull request as your own on the wrong branch would compare and fix the wrong code,
/// so it is refused, saying which branch to check out and how to read it instead.
#[tokio::test]
async fn your_own_pull_request_needs_its_branch_checked_out() {
    let remote = Remote::new();
    let mut h = harness(&remote.project, Some(scripted(&remote, None) as Arc<dyn GitHub>)).await;

    let failure = h.load(ReviewTarget::OwnPullRequest { number: 1 }).await.unwrap_err();

    let message = format!("{failure:?}");
    assert!(message.contains("comes from `fix-parser`"), "{message}");
    assert!(message.contains("this checkout is on `main`"), "{message}");
    assert!(message.contains("/diff pr 1"), "{message}");
}

/// A post whose comments moved after the preview was drawn is refused, and GitHub hears nothing:
/// what posts is what the reader saw.
#[tokio::test]
async fn a_post_whose_comments_moved_since_the_preview_is_refused() {
    let remote = Remote::new();
    let github = scripted(&remote, None);
    let (mut h, review) = pull_request_review(github.clone(), &remote).await;
    let request_id = luxe_core::guided_review::PullRequestPostId::new();
    assert!(matches!(
        h.prepare_as(request_id.clone(), &review).await,
        Event::PullRequestPostPrepared { .. }
    ));
    let shown = h.comment(&review, "Looks right.").await;
    h.comment(&review, "One more thing.").await;
    h.actions
        .send(Action::PostPullRequestReview {
            request_id: request_id.clone(),
            verdict: luxe_core::github::ReviewEvent::Comment,
            session_revision: shown.revision,
        })
        .await
        .unwrap();
    let Event::PullRequestPostFailed { message, .. } = h.post_answer(&request_id).await else {
        panic!("refused");
    };
    assert!(message.contains("comments changed"), "{message}");
    assert!(github.submitted.lock().unwrap().is_empty(), "nothing reached GitHub");
}

/// On your own pull request only comments go: GitHub refuses the author's approval, so luxe says so
/// before asking it.
#[tokio::test]
async fn approving_your_own_pull_request_is_refused_before_github_is_asked() {
    let remote = Remote::new();
    let own = Arc::new(ScriptedGitHub {
        viewer: "APP/copilot-swe-agent".into(),
        ..Arc::into_inner(scripted(&remote, None)).unwrap()
    });
    let (mut h, review) = pull_request_review(own.clone(), &remote).await;
    let request_id = luxe_core::guided_review::PullRequestPostId::new();
    h.prepare_as(request_id.clone(), &review).await;
    let session = h.comment(&review, "Ship it.").await;
    h.actions
        .send(Action::PostPullRequestReview {
            request_id: request_id.clone(),
            verdict: luxe_core::github::ReviewEvent::Approve,
            session_revision: session.revision,
        })
        .await
        .unwrap();
    let Event::PullRequestPostFailed { message, .. } = h.post_answer(&request_id).await else {
        panic!("refused");
    };
    assert!(message.contains("your own pull request"), "{message}");
    assert!(own.submitted.lock().unwrap().is_empty());
}

/// A confirm with no draft behind it opens nothing: there is no pull request it could have meant.
#[tokio::test]
async fn an_open_with_no_draft_behind_it_opens_nothing() {
    let remote = Remote::new();
    let github = scripted(&remote, None);
    let (mut h, _review) = published_branch(&remote, github.clone()).await;
    h.actions
        .send(Action::OpenPullRequest {
            request_id: luxe_core::guided_review::PullRequestOpenId::new(),
            title: "Anything".into(),
            body: "At all.".into(),
            draft: false,
        })
        .await
        .unwrap();
    match h.next_opening().await {
        Event::PullRequestOpenFailed { message, .. } => {
            assert!(message.contains("not the current one"), "{message}");
        }
        other => panic!("{other:?}"),
    }
    assert!(github.opened.lock().unwrap().is_empty());
}

/// A commit asked of a revision the review has moved past is refused, and nothing is written: the
/// reader confirmed chapters they may not be looking at any more.
#[tokio::test]
async fn committing_a_revision_the_review_has_moved_past_writes_nothing() {
    let remote = Remote::new();
    std::fs::write(remote.project.join("parser.rs"), "fn parse() { keep_last_token(); }\n")
        .unwrap();
    let mut h = harness(&remote.project, None).await;
    let review = h.load(ReviewTarget::WorkingTree).await.unwrap();
    let head = git(&remote.project, &["rev-parse", "HEAD"]);
    let mut seen_before = review.clone();
    seen_before.revision_id =
        luxe_core::guided_review::ReviewRevisionId::from_canonical_bytes(b"older");
    let refused = commit_review(&mut h, &seen_before, false).await.expect_err("refused");
    assert!(refused.contains("refreshed since"), "{refused}");
    assert_eq!(git(&remote.project, &["rev-parse", "HEAD"]), head, "nothing committed");
    assert!(commit_review(&mut h, &review, false).await.is_ok(), "the revision on screen commits");
}

/// Someone else's pull request takes the verdict the reader chose, and it reaches GitHub as chosen.
#[tokio::test]
async fn a_verdict_on_someone_elses_pull_request_reaches_github_as_chosen() {
    let remote = Remote::new();
    let github = scripted(&remote, None);
    let (mut h, review) = pull_request_review(github.clone(), &remote).await;
    let event = h
        .post_with(&review, "Bound the retries.", luxe_core::github::ReviewEvent::RequestChanges)
        .await;
    assert!(matches!(event, Event::PullRequestReviewPosted { .. }), "{event:?}");
    let submitted = github.submitted.lock().unwrap();
    assert_eq!(submitted[0].event, luxe_core::github::ReviewEvent::RequestChanges);
    assert_eq!(submitted[0].body, "Bound the retries.");
}
