//! GitHub posts — a review posted to a pull request, and the conversation read back.

use super::*;

/// A post fixture with its engine channel in hand, and the post's request id.
pub(super) fn post_fixture(
) -> (App, mpsc::Receiver<Action>, luxe_core::guided_review::PullRequestPostId) {
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewPullRequestPost)
        .expect("pull request post fixture");
    let (engine_tx, engine_rx) = mpsc::channel(8);
    app.engine_tx = engine_tx;
    let request_id =
        app.guided_review_workspace.github_post.as_ref().expect("post preview").request_id.clone();
    (app, engine_rx, request_id)
}

/// A pull request's review finishes on GitHub: Alt+Enter asks GitHub what the preview needs rather
/// than opening the local finish choices, and Alt+U offers to check it out for the agent.
#[test]
fn finishing_a_pull_request_review_asks_github_and_alt_u_offers_a_worktree() {
    let mut app =
        seeded_app("nord", FixtureScenario::GuidedReviewPullRequest).expect("pull request fixture");
    let (engine_tx, mut engine_rx) = mpsc::channel(8);
    app.engine_tx = engine_tx;

    assert!(dispatch_action(&mut app, UiAction::ReviewFinish));

    assert!(app.guided_review_workspace.finish_review_choice.is_none());
    let post = app.guided_review_workspace.github_post.as_ref().expect("post preview");
    assert!(post.prepared.is_none(), "nothing to preview until GitHub answers");
    let Ok(Action::PreparePullRequestPost { request_id, .. }) = engine_rx.try_recv() else {
        panic!("expected the engine to be asked");
    };
    assert_eq!(request_id, post.request_id);

    // Alt+U cannot reach an agent that has not got the code: it offers to check it out instead
    app.guided_review_workspace.github_post = None;
    app.update(crate::app::Msg::GuidedReviewOpenSendPreview);
    assert!(app.guided_review_workspace.send_preview.is_none());
    assert!(app.guided_review_workspace.pull_request_fix.is_some());
}

/// Confirming names the preview it shows, with the verdict on screen and the session revision the
/// preview was drawn from, so the engine posts that and nothing else; it plans the post with the
/// same function over the same comments, and refuses if they moved. While it is in flight the
/// dialog cannot be closed, since closing it would drop the answer that says where it landed.
#[test]
fn confirming_posts_the_preview_it_shows_and_holds_the_dialog_open() {
    let (mut app, mut engine_rx, prepared) = post_fixture();
    assert!(crate::app::github_post_plan(&app).expect("prepared").is_ok(), "postable");
    let verdict = app.guided_review_workspace.github_post.as_ref().expect("preview").verdict;
    let revision = app.review.session().expect("session").revision;

    app.update(crate::app::Msg::GuidedReviewConfirmGithubPost);

    let Ok(Action::PostPullRequestReview { request_id, verdict: sent, session_revision }) =
        engine_rx.try_recv()
    else {
        panic!("expected a post");
    };
    assert_eq!(request_id, prepared, "the preview the engine read, not a new attempt");
    assert_eq!(sent, verdict);
    assert_eq!(session_revision, revision);

    app.update(crate::app::Msg::GuidedReviewCancelGithubPost);
    assert!(app.guided_review_workspace.github_post.as_ref().is_some_and(|post| post.posting));
    app.update(crate::app::Msg::GuidedReviewConfirmGithubPost);
    assert!(engine_rx.try_recv().is_err(), "a second press does not post twice");
}

/// GitHub refuses an author's verdict on their own pull request, so the preview offers a comment
/// and nothing else once it knows.
#[test]
fn an_authors_own_pull_request_offers_only_a_comment() {
    let (mut app, _engine_rx, _) = post_fixture();
    let post = app.guided_review_workspace.github_post.as_mut().expect("post preview");
    post.prepared = None;
    let request_id = post.request_id.clone();

    app.update(crate::app::Msg::Engine(luxe_core::event::Event::PullRequestPostPrepared {
        request_id,
        patches: Vec::new(),
        head_moved: false,
        own_pull_request: true,
    }));
    app.update(crate::app::Msg::GuidedReviewMoveGithubVerdict(true));

    let post = app.guided_review_workspace.github_post.as_ref().expect("post preview");
    assert_eq!(post.verdict, luxe_core::github::ReviewEvent::Comment);
    assert_eq!(post.verdicts(), &[luxe_core::github::ReviewEvent::Comment]);
}

/// A post that landed marks what it carried and records the verdict, in one save; an answer for an
/// attempt nobody is waiting on changes nothing.
#[test]
fn a_landed_post_marks_its_comments_and_a_stale_answer_is_ignored() {
    let (mut app, mut engine_rx, _) = post_fixture();
    let shown = crate::app::github_post_plan(&app).expect("prepared").expect("postable");
    let finding_ids: Vec<_> = shown.placed.iter().map(|placed| placed.finding_id.clone()).collect();
    app.update(crate::app::Msg::GuidedReviewConfirmGithubPost);
    let Ok(Action::PostPullRequestReview { request_id, .. }) = engine_rx.try_recv() else {
        panic!("expected a post");
    };

    app.update(crate::app::Msg::Engine(luxe_core::event::Event::PullRequestReviewPosted {
        request_id: luxe_core::guided_review::PullRequestPostId::new(),
        github_review_id: 1,
        url: "https://example.invalid/stale".into(),
        finding_ids: finding_ids.clone(),
    }));
    assert!(engine_rx.try_recv().is_err(), "a stale answer saves nothing");
    assert!(app.guided_review_workspace.github_post.is_some());

    let url = "https://github.com/acme/notify/pull/42#pullrequestreview-9".to_owned();
    app.update(crate::app::Msg::Engine(luxe_core::event::Event::PullRequestReviewPosted {
        request_id,
        github_review_id: 9,
        url: url.clone(),
        finding_ids: finding_ids.clone(),
    }));

    assert!(app.guided_review_workspace.github_post.is_none());
    assert_eq!(app.guided_review_workspace.posted_review_url.as_deref(), Some(url.as_str()));
    // The engine marked what went and recorded the verdict as it posted; the TUI sends nothing,
    // and takes the marks when the desk's save arrives
    assert!(engine_rx.try_recv().is_err());
    assert!(!finding_ids.is_empty());
}

/// A failure keeps the preview open with GitHub's reason, and posting again is allowed.
#[test]
fn a_failed_post_says_why_and_can_be_tried_again() {
    let (mut app, mut engine_rx, _) = post_fixture();
    app.update(crate::app::Msg::GuidedReviewConfirmGithubPost);
    let Ok(Action::PostPullRequestReview { request_id, .. }) = engine_rx.try_recv() else {
        panic!("expected a post");
    };

    app.update(crate::app::Msg::Engine(luxe_core::event::Event::PullRequestPostFailed {
        request_id,
        message: "GitHub refused the review: Line could not be resolved.".into(),
    }));

    let post = app.guided_review_workspace.github_post.as_ref().expect("still open");
    assert!(!post.posting);
    assert!(post.error.as_deref().is_some_and(|error| error.contains("Line could not")));
    app.update(crate::app::Msg::GuidedReviewConfirmGithubPost);
    assert!(matches!(engine_rx.try_recv(), Ok(Action::PostPullRequestReview { .. })));
}

/// Requesting changes with nothing to say is refused before anything is sent.
#[test]
fn requesting_changes_without_a_summary_sends_nothing() {
    let (mut app, mut engine_rx, _) = post_fixture();
    let session = app.review.session_mut().expect("session");
    session.findings.retain(|finding| {
        !matches!(finding.anchor, luxe_core::guided_review::FindingAnchor::Review)
            || finding.severity == luxe_core::guided_review::FindingSeverity::Question
    });

    app.update(crate::app::Msg::GuidedReviewConfirmGithubPost);

    assert!(engine_rx.try_recv().is_err());
    assert!(!app.guided_review_workspace.github_post.as_ref().expect("open").posting);
    assert!(
        app.status.toast().is_some_and(|toast| toast.starts_with("Requesting changes needs")),
        "{:?}",
        app.status.toast()
    );
}

/// The verdict steps through what is on offer both ways and wraps at either end.
#[test]
fn the_verdict_steps_both_ways_and_wraps() {
    use luxe_core::github::ReviewEvent::{Approve, Comment, RequestChanges};
    let (mut app, _engine_rx, _) = post_fixture();
    let post = app.guided_review_workspace.github_post.as_mut().expect("post preview");
    post.verdict = Comment;
    let mut forward = Vec::new();
    for _ in 0..3 {
        post.move_verdict(true);
        forward.push(post.verdict);
    }
    assert_eq!(forward, [Approve, RequestChanges, Comment]);
    let mut back = Vec::new();
    for _ in 0..3 {
        post.move_verdict(false);
        back.push(post.verdict);
    }
    assert_eq!(back, [RequestChanges, Approve, Comment]);
}

/// Request changes needs a summary, and the preview is where it is written: `c` there opens a
/// comment on the review as a whole, over the preview, and once it is saved the preview posts it.
#[test]
fn a_summary_is_written_from_the_preview_and_posts_with_it() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let (mut app, mut engine_rx, _) = post_fixture();
    let session = app.review.session_mut().expect("session");
    session.findings.retain(|finding| {
        !matches!(finding.anchor, luxe_core::guided_review::FindingAnchor::Review)
            || finding.severity == luxe_core::guided_review::FindingSeverity::Question
    });
    assert!(matches!(
        crate::app::github_post_plan(&app),
        Some(Err(luxe_core::guided_review::PostPlanError::NeedsBody))
    ));

    app.update(crate::app::Msg::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE)));
    let draft = app.guided_review_workspace.finding_draft.as_ref().expect("summary composer");
    assert_eq!(draft.anchor, luxe_core::guided_review::FindingAnchor::Review);
    for character in "Bound the redirects.".chars() {
        app.update(crate::app::Msg::Key(KeyEvent::new(
            KeyCode::Char(character),
            KeyModifiers::NONE,
        )));
    }
    assert!(app.guided_review_workspace.github_post.is_some(), "typing stays in the composer");
    app.update(crate::app::Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL)));
    let Ok(Action::EditReview {
        request_id,
        edit: luxe_core::engine::ReviewEdit::Comment { draft, .. },
        ..
    }) = engine_rx.try_recv()
    else {
        panic!("expected the summary to be sent to the review desk");
    };
    // What the desk stores for it, answering this edit
    let mut stored = app.review.session.clone().expect("session");
    stored
        .add_finding(luxe_core::guided_review::ReviewFinding {
            id: luxe_core::guided_review::FindingId::new(),
            label: String::new(),
            anchor: draft.anchor,
            severity: draft.severity,
            title: draft.title,
            body: draft.body,
            proposed_fix: draft.proposed_fix,
            state: luxe_core::guided_review::FindingState::Draft,
            thread: Vec::new(),
            posted: None,
            github_thread: None,
        })
        .expect("the summary is a valid comment");
    app.update(crate::app::Msg::Engine(luxe_core::event::Event::ReviewSessionSaved {
        request_id,
        session: Box::new(stored),
    }));

    let plan = crate::app::github_post_plan(&app).expect("prepared").expect("postable now");
    assert_eq!(plan.review.body, "Bound the redirects.");
    assert!(app.guided_review_workspace.finding_draft.is_none());
}

/// `/pr` opens the branch's pull request in its card and asks for the overview. Its threads are
/// work to do: once the overview lands, `t` opens them in a review against the working tree, as
/// `/pr` used to on its own. A merged one has nothing left to resolve, and a branch with no pull
/// request gets the sentence saying so.
#[test]
fn slash_pr_opens_the_card_and_its_threads_are_a_key_away() {
    use luxe_core::github::overview::{Overview, PullRequestOverview};
    use luxe_core::github::{CheckRollup, PullRequestState, PullRequestStatus, ReviewDecision};
    use luxe_core::workspace::PullRequestPresence;
    let registry = crate::commands::CommandRegistry::all();
    for (state, opens) in [(PullRequestState::Open, true), (PullRequestState::Merged, false)] {
        let mut app = seeded_app("nord", FixtureScenario::DockWide).expect("fixture app");
        let (engine_tx, mut engine_rx) = mpsc::channel(8);
        app.engine_tx = engine_tx;
        app.workspace.as_mut().expect("workspace").pull_request =
            PullRequestPresence::Found(PullRequestStatus {
                number: 146,
                title: "Round trip".into(),
                url: "https://github.com/acme/app/pull/146".into(),
                state,
                draft: false,
                review_decision: None,
                checks: CheckRollup::default(),
                unresolved_threads: 2,
                head_oid: "f6195af".into(),
                stack: None,
            });

        run(&mut app, &registry.parse_slash("/pr").expect("/pr parses"));
        assert_eq!(app.overlays.pull_request_card.as_ref().map(|card| card.number), Some(146));
        let asked = std::iter::from_fn(|| engine_rx.try_recv().ok()).any(|action| {
            matches!(
                action,
                Action::ReadGithubOverview { repository: None, number: 146, pull_request: true }
            )
        });
        assert!(asked, "the card asks for its overview");

        let overview = PullRequestOverview {
            number: 146,
            state,
            review_decision: Some(ReviewDecision::ChangesRequested),
            unresolved_threads: 2,
            viewer: luxe_core::github::overview::Viewer {
                author: true,
                can_push: true,
                can_merge: true,
            },
            ..crate::fixture::pull_request_overview()
        };
        app.update(crate::app::Msg::Engine(luxe_core::event::Event::GithubOverviewRead {
            repository: None,
            number: 146,
            overview: Some(Overview::PullRequest(Box::new(overview))),
            error: None,
            local: None,
        }));
        app.update(key(crossterm::event::KeyCode::Char('t')));
        let loaded =
            std::iter::from_fn(|| engine_rx.try_recv().ok()).find_map(|action| match action {
                Action::LoadGuidedReview { target, .. } => Some(target),
                _ => None,
            });
        assert_eq!(
            loaded,
            opens.then_some(luxe_core::guided_review::ReviewTarget::OwnPullRequest { number: 146 }),
            "{state:?}"
        );
        assert_eq!(app.overlays.pull_request_card.is_none(), opens, "acting leaves the card");
    }

    let mut app = seeded_app("nord", FixtureScenario::DockWide).expect("fixture app");
    app.workspace.as_mut().expect("workspace").pull_request = PullRequestPresence::None;
    run(&mut app, &registry.parse_slash("/pr").expect("/pr parses"));
    assert!(app.overlays.pull_request_card.is_none());
    assert!(matches!(
        app.convo.last().map(|item| item.block()),
        Some(ConvoBlock::System(text)) if text == "This branch has no pull request on GitHub."
    ));
}
