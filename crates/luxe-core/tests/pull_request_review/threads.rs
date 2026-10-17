//! Threads — earlier conversation is placed and kept out of the guide, and your own threads are answered.

use std::sync::Arc;

use luxe_core::event::Event;
use luxe_core::github::GitHub;
use luxe_core::guided_review::ReviewTarget;

use crate::support::*;

/// Threads land on the review — the current one on its line, the outdated one listed — and none of
/// their words reach the guide, which should not argue with a reviewer on the reader's behalf.
#[tokio::test]
async fn earlier_threads_are_placed_and_kept_out_of_the_guide() {
    use luxe_core::guided_review::{DiffSide, EarlierConversation};
    let remote = Remote::new();
    let mut h = harness(&remote.project, Some(scripted(&remote, None) as Arc<dyn GitHub>)).await;
    let review = h.load(ReviewTarget::PullRequest { number: 1, repository: None }).await.unwrap();

    let Some(EarlierConversation::Read { threads, more: 0, said }) = &review.conversation else {
        panic!("the conversation was read: {:?}", review.conversation);
    };
    assert_eq!(threads.len(), 2, "none dropped");
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(threads[0].sits_at(&review.files[0].id, DiffSide::New, 1), "{:?}", threads[0]);
    assert!(threads[1].anchor.is_none(), "an outdated thread is listed, not placed");

    let prompt = h.guide_prompt(0).await;
    assert!(
        !prompt.contains("keep_last_token run on empty input"),
        "threads stay out of the guide"
    );
    assert!(!prompt.contains("allocate twice"));
    assert!(!prompt.contains("changelog say so"), "nor does the conversation");
}

/// A conversation that could not be read is said to be unreadable; the review still loads.
#[tokio::test]
async fn unreadable_threads_do_not_cost_the_review() {
    use luxe_core::guided_review::EarlierConversation;
    let remote = Remote::new();
    let github = Arc::new(ScriptedGitHub {
        threads_fail: true,
        ..Arc::into_inner(scripted(&remote, None)).unwrap()
    });
    let mut h = harness(&remote.project, Some(github as Arc<dyn GitHub>)).await;
    let review = h.load(ReviewTarget::PullRequest { number: 1, repository: None }).await.unwrap();
    assert_eq!(review.conversation, Some(EarlierConversation::Unreadable));
    assert!(!review.files.is_empty());
}

/// A review of anything but a pull request has no conversation to show.
#[tokio::test]
async fn only_a_pull_request_has_an_earlier_conversation() {
    let remote = Remote::new();
    let mut h = harness(&remote.project, Some(scripted(&remote, None) as Arc<dyn GitHub>)).await;
    let head = git(&remote.project, &["rev-parse", "HEAD"]);
    let review = h.load(ReviewTarget::Commit { oid: head }).await.unwrap();
    assert!(review.conversation.is_none());
}

/// Preparing says whether the fix is on GitHub: `HEAD` is the pull request's head, and nothing is
/// left uncommitted.
#[tokio::test]
async fn preparing_answers_says_whether_the_fix_is_on_github() {
    let remote = Remote::new();
    let (mut h, review) = own_review(&remote, scripted(&remote, None)).await;
    let prepared = |event: Event| match event {
        Event::ThreadAnswersPrepared { pushed, uncommitted, .. } => (pushed, uncommitted),
        other => panic!("{other:?}"),
    };
    assert_eq!(prepared(h.prepare_answers(&review).await), (true, false));

    std::fs::write(remote.project.join("parser.rs"), "fn parse() { fixed(); }\n").unwrap();
    assert_eq!(prepared(h.prepare_answers(&review).await), (true, true), "edited, not committed");

    git(&remote.project, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qam", "Fix"]);
    assert_eq!(prepared(h.prepare_answers(&review).await), (false, false), "committed, not pushed");
}

/// Each thread is answered as confirmed and reported on its own. A reply that fails leaves its
/// thread open — resolving it would close the thread with nothing said — and one that timed out
/// says to look before sending it again.
#[tokio::test]
async fn threads_are_answered_one_at_a_time_and_each_says_how_it_went() {
    use luxe_core::guided_review::ThreadReply;
    let remote = Remote::new();
    let github = scripted(&remote, None);
    let (mut h, review) = own_review(&remote, github.clone()).await;
    let threads = ["t1", "t2", "t3", "refused", "slow", "unresolvable", "IC_1", "IC_gone"];
    let ids = h.threads(&review, &threads).await;
    let answer = |index: usize, reply: Option<&str>, resolve: bool| ThreadReply {
        finding_id: ids[index].clone(),
        reply: reply.map(str::to_string),
        resolve,
    };
    // A comment that never was a thread is answered too: with why nothing went
    let stranger = luxe_core::guided_review::FindingId::new();

    let Event::ThreadsAnswered { results, .. } = h
        .answer(
            &review,
            vec![
                answer(0, Some("Fixed: it starts at 0 now."), true),
                answer(1, None, true),
                answer(2, Some("   "), false),
                answer(3, Some("Done."), true),
                answer(4, Some("Done."), true),
                answer(5, None, true),
                answer(6, Some("Added to the changelog."), true),
                answer(7, Some("Sure."), false),
                ThreadReply {
                    finding_id: stranger.clone(),
                    reply: Some("Hi.".into()),
                    resolve: true,
                },
            ],
        )
        .await
    else {
        panic!("answered");
    };

    assert_eq!(results.len(), 9);
    assert_eq!(results[8].finding_id, stranger);
    assert!(results[8]
        .error
        .as_deref()
        .is_some_and(|error| error.contains("not come from a thread")));
    assert_eq!(
        results[0].reply_url.as_deref(),
        Some("https://github.com/acme/app/pull/1#discussion_t1")
    );
    assert!(results[0].resolved && results[0].error.is_none());
    assert!(results[1].resolved && results[1].reply_url.is_none());
    assert!(!results[2].resolved && results[2].reply_url.is_none() && results[2].error.is_none());
    assert!(!results[3].resolved);
    assert!(results[3].error.as_deref().is_some_and(|error| error.contains("Thread is locked")));
    assert!(results[4].error.as_deref().is_some_and(|error| error.contains("may have landed")));
    assert!(results[5].error.as_deref().is_some_and(|error| error.contains("not accessible")));
    assert_eq!(
        *github.answered.lock().unwrap(),
        [
            "reply:t1:Fixed: it starts at 0 now.",
            "resolve:t1",
            "resolve:t2",
            "reply:refused:Done.",
            "reply:slow:Done.",
            "resolve:unresolvable",
            "comment:1:> Could the changelog say so?\n\nAdded to the changelog.",
            "comment:1:Sure.",
        ],
        "the reader's words as written, a blank reply not sent, a failed reply never resolved, \
         and the conversation answered in the conversation, quoting what it answers when it can"
    );
    assert_eq!(
        results[6].reply_url.as_deref(),
        Some("https://github.com/acme/app/pull/1#issuecomment-9")
    );
    assert!(!results[6].resolved, "the conversation has nothing to resolve");
}

/// Only your own pull request's threads are answered from here.
#[tokio::test]
async fn someone_elses_pull_request_is_not_answered_from_here() {
    let remote = Remote::new();
    let github = scripted(&remote, None);
    let mut h = harness(&remote.project, Some(github.clone() as Arc<dyn GitHub>)).await;
    let review = h.load(ReviewTarget::PullRequest { number: 1, repository: None }).await.unwrap();

    let Event::ThreadAnswersFailed { message, .. } = h.answer(&review, Vec::new()).await else {
        panic!("refused");
    };
    assert!(message.contains("Only your own pull request"), "{message}");
    assert!(github.answered.lock().unwrap().is_empty());
}
