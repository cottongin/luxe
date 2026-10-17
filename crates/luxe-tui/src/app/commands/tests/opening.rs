//! Opening — a branch's pull request, a fix for someone else's, and a review opened as a stack.

use super::*;

/// A branch's review, with the status row saying what GitHub has for the branch.
pub(super) fn branch_review(
    presence: luxe_core::workspace::PullRequestPresence,
) -> (App, mpsc::Receiver<Action>) {
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewFindings).expect("fixture");
    let mut snapshot = app.workspace.clone().unwrap_or_else(|| {
        luxe_core::workspace::WorkspaceSnapshot::empty(
            luxe_core::workspace::WorkspaceScopeId::new("scope-a"),
            luxe_core::workspace::ProjectIdentity::non_git(
                "/project".into(),
                "project",
                "/project",
            ),
        )
    });
    snapshot.pull_request = presence;
    app.workspace = Some(snapshot);
    let (engine_tx, engine_rx) = mpsc::channel(8);
    app.engine_tx = engine_tx;
    (app, engine_rx)
}

pub(super) fn found(
    state: luxe_core::github::PullRequestState,
) -> luxe_core::workspace::PullRequestPresence {
    luxe_core::workspace::PullRequestPresence::Found(luxe_core::github::PullRequestStatus {
        number: 40,
        title: "Earlier".into(),
        url: String::new(),
        state,
        draft: false,
        review_decision: None,
        checks: Default::default(),
        unresolved_threads: 0,
        head_oid: String::new(),
        stack: None,
    })
}

/// Offered where the branch has no open pull request, as far as the status row knows, and
/// choosing it asks the engine for the draft.
#[test]
fn finishing_a_branch_without_a_pull_request_offers_opening_one() {
    use crate::guided_review::FinishReviewChoice;
    use luxe_core::github::PullRequestState;
    use luxe_core::workspace::PullRequestPresence;
    for (presence, offered) in [
        (PullRequestPresence::None, true),
        (found(PullRequestState::Closed), true),
        (found(PullRequestState::Open), false),
        (PullRequestPresence::Unknown, false),
        (PullRequestPresence::Unavailable { reason: "no gh".into() }, false),
    ] {
        let (mut app, _) = branch_review(presence.clone());
        assert!(dispatch_action(&mut app, UiAction::ReviewFinish));
        assert_eq!(app.guided_review_workspace.finish_offers_opening, offered, "{presence:?}");
        let mut seen = Vec::new();
        for _ in 0..7 {
            app.guided_review_workspace.move_finish_review_choice(true);
            seen.push(app.guided_review_workspace.finish_review_choice.expect("open"));
        }
        assert_eq!(seen.contains(&FinishReviewChoice::OpenPullRequest), offered, "{presence:?}");
    }

    let (mut app, mut engine_rx) = branch_review(PullRequestPresence::None);
    assert!(dispatch_action(&mut app, UiAction::ReviewFinish));
    while app.guided_review_workspace.finish_review_choice
        != Some(FinishReviewChoice::OpenPullRequest)
    {
        app.guided_review_workspace.move_finish_review_choice(true);
    }
    app.update(key(crossterm::event::KeyCode::Enter));
    assert!(app.guided_review_workspace.finish_review_choice.is_none());
    assert!(app.guided_review_workspace.pull_request_opening.is_some());
    assert!(matches!(engine_rx.try_recv(), Ok(Action::PreparePullRequest { .. })));

    // Your own pull request already exists, so it answers its threads instead
    let (mut app, _) = own_pull_request();
    let mut snapshot = app.workspace.clone().unwrap_or_else(|| {
        luxe_core::workspace::WorkspaceSnapshot::empty(
            luxe_core::workspace::WorkspaceScopeId::new("scope-a"),
            luxe_core::workspace::ProjectIdentity::non_git(
                "/project".into(),
                "project",
                "/project",
            ),
        )
    });
    snapshot.pull_request = PullRequestPresence::None;
    app.workspace = Some(snapshot);
    assert!(dispatch_action(&mut app, UiAction::ReviewFinish));
    assert!(!app.guided_review_workspace.finish_offers_opening);
}

pub(super) fn prepared_event(
    request_id: luxe_core::guided_review::PullRequestOpenId,
    commits: &[&str],
) -> crate::app::Msg {
    crate::app::Msg::Engine(luxe_core::event::Event::PullRequestPrepared {
        request_id,
        pull_request: luxe_core::github::NewPullRequest {
            head: "7-fix".into(),
            base: "main".into(),
            title: "Fix the total".into(),
            body: "### Sum every element\n\nWhy.\n\nCloses #7".into(),
            draft: false,
        },
        publication: luxe_core::github::Publication::Unpublished,
        uncommitted: 1,
        commits: commits.iter().map(|subject| (*subject).to_owned()).collect(),
    })
}

/// What opens is the draft with the reader's edits, typed with real keys, and the reviewed work is
/// committed first when the preview said so. Nothing is sent while typing.
#[test]
fn the_pull_request_opens_as_the_reader_edited_it() {
    let (mut app, mut engine_rx) = branch_review(luxe_core::workspace::PullRequestPresence::None);
    app.update(crate::app::Msg::GuidedReviewOpenPullRequest);
    let Ok(Action::PreparePullRequest { request_id, .. }) = engine_rx.try_recv() else {
        panic!("expected a prepare");
    };
    app.update(prepared_event(request_id, &["Sum every element"]));

    app.update(key(crossterm::event::KeyCode::Char('t')));
    for character in " at last".chars() {
        app.update(key(crossterm::event::KeyCode::Char(character)));
    }
    app.update(key(crossterm::event::KeyCode::Enter));
    app.update(key(crossterm::event::KeyCode::Char('e')));
    app.update(key(crossterm::event::KeyCode::Enter));
    for character in "Tested by hand.".chars() {
        app.update(key(crossterm::event::KeyCode::Char(character)));
    }
    assert!(engine_rx.try_recv().is_err(), "typing sends nothing");
    app.update(crate::app::Msg::Key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Enter,
        crossterm::event::KeyModifiers::CONTROL,
    )));
    // A field dropped with Esc keeps what was there before
    app.update(key(crossterm::event::KeyCode::Char('t')));
    app.update(key(crossterm::event::KeyCode::Char('!')));
    app.update(key(crossterm::event::KeyCode::Esc));
    app.update(key(crossterm::event::KeyCode::Char('d')));
    app.update(key(crossterm::event::KeyCode::Enter));

    // What the reader could change goes back; the engine kept the rest, commits included
    let Ok(Action::OpenPullRequest { request_id, title, body, draft }) = engine_rx.try_recv()
    else {
        panic!("expected an open");
    };
    assert_eq!(title, "Fix the total at last");
    assert_eq!(
        body, "### Sum every element\n\nWhy.\n\nCloses #7\nTested by hand.",
        "the reader's words, as typed"
    );
    assert!(draft, "d made it a draft");

    // In flight, the preview cannot be closed or edited
    app.update(key(crossterm::event::KeyCode::Esc));
    app.update(key(crossterm::event::KeyCode::Char('t')));
    let opening = app.guided_review_workspace.pull_request_opening.as_ref().expect("preview");
    assert!(opening.opening && opening.editing.is_none());

    app.update(crate::app::Msg::Engine(luxe_core::event::Event::PullRequestOpened {
        request_id,
        number: 151,
        url: "https://github.com/acme/app/pull/151".into(),
        committed: vec!["0123abcd".into()],
    }));
    assert!(app.guided_review_workspace.pull_request_opening.is_none());
    assert_eq!(
        app.guided_review_workspace.posted_review_url.as_deref(),
        Some("https://github.com/acme/app/pull/151"),
        "o opens it"
    );
    assert!(
        app.convo.iter().any(|item| matches!(item.block(), ConvoBlock::System(text)
            if text == "[review] Committed the reviewed work and opened #151: https://github.com/acme/app/pull/151")),
        "says what happened"
    );
}

/// `E` hands the description to the reader's editor, and what comes back replaces the draft. A
/// preview that started opening meanwhile keeps what it sent.
#[test]
fn the_description_goes_to_your_editor_and_comes_back() {
    let (mut app, mut engine_rx) = branch_review(luxe_core::workspace::PullRequestPresence::None);
    app.update(crate::app::Msg::GuidedReviewOpenPullRequest);
    let Ok(Action::PreparePullRequest { request_id, .. }) = engine_rx.try_recv() else {
        panic!("expected a prepare");
    };
    app.update(prepared_event(request_id, &[]));
    let shift = |character| {
        crate::app::Msg::Key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char(character),
            crossterm::event::KeyModifiers::SHIFT,
        ))
    };
    app.update(shift('E'));
    let edit = app.outbox.text_edit.take().expect("an edit for the main loop");
    assert_eq!(edit.text, "### Sum every element\n\nWhy.\n\nCloses #7");
    assert_eq!(edit.target, crate::launcher::EditTarget::PullRequestBody);

    app.update(crate::app::Msg::TextEdited {
        target: crate::launcher::EditTarget::PullRequestBody,
        text: "Rewritten at length.".into(),
    });
    let opening = app.guided_review_workspace.pull_request_opening.as_ref().expect("preview");
    assert_eq!(opening.prepared.as_ref().unwrap().pull_request.body, "Rewritten at length.");

    // While a field is open its keys are its own: E is a letter there
    app.update(key(crossterm::event::KeyCode::Char('t')));
    app.update(shift('E'));
    assert!(app.outbox.text_edit.is_none(), "typed, not handed off");
    app.update(key(crossterm::event::KeyCode::Esc));

    app.update(key(crossterm::event::KeyCode::Enter));
    assert!(matches!(engine_rx.try_recv(), Ok(Action::OpenPullRequest { .. })));
    app.update(crate::app::Msg::TextEdited {
        target: crate::launcher::EditTarget::PullRequestBody,
        text: "Too late.".into(),
    });
    let opening = app.guided_review_workspace.pull_request_opening.as_ref().expect("preview");
    assert_eq!(opening.prepared.as_ref().unwrap().pull_request.body, "Rewritten at length.");
}

/// With nothing to commit, nothing is; a failure keeps the preview open saying why, an answer for
/// an attempt no longer waiting is ignored, and an empty title is refused.
#[test]
fn a_failed_opening_says_why_and_a_stale_answer_is_ignored() {
    let (mut app, mut engine_rx) = branch_review(luxe_core::workspace::PullRequestPresence::None);
    app.update(crate::app::Msg::GuidedReviewOpenPullRequest);
    let Ok(Action::PreparePullRequest { request_id, .. }) = engine_rx.try_recv() else {
        panic!("expected a prepare");
    };
    app.update(prepared_event(luxe_core::guided_review::PullRequestOpenId::new(), &[]));
    assert!(
        app.guided_review_workspace.pull_request_opening.as_ref().unwrap().prepared.is_none(),
        "not the attempt it waits on"
    );
    app.update(prepared_event(request_id, &[]));
    app.update(crate::app::Msg::GuidedReviewConfirmPullRequest);
    let Ok(Action::OpenPullRequest { request_id, .. }) = engine_rx.try_recv() else {
        panic!("expected an open");
    };
    app.update(crate::app::Msg::Engine(luxe_core::event::Event::PullRequestOpenFailed {
        request_id,
        message: "GitHub refused it: No commits between main and 7-fix".into(),
    }));
    let opening = app.guided_review_workspace.pull_request_opening.as_ref().expect("still open");
    assert!(!opening.opening);
    assert_eq!(
        opening.error.as_deref(),
        Some("GitHub refused it: No commits between main and 7-fix")
    );

    app.update(key(crossterm::event::KeyCode::Char('t')));
    for _ in 0.."Fix the total".len() {
        app.update(key(crossterm::event::KeyCode::Backspace));
    }
    app.update(key(crossterm::event::KeyCode::Enter));
    app.update(crate::app::Msg::GuidedReviewConfirmPullRequest);
    assert!(engine_rx.try_recv().is_err(), "no title, nothing opens");
    app.update(key(crossterm::event::KeyCode::Esc));
    assert!(app.guided_review_workspace.pull_request_opening.is_none(), "closes when idle");
}

/// Someone else's pull request with three unsent comments, the post preview closed.
pub(super) fn their_pull_request() -> (App, mpsc::Receiver<Action>) {
    let mut app =
        seeded_app("nord", FixtureScenario::GuidedReviewPullRequestPost).expect("fixture");
    app.guided_review_workspace.github_post = None;
    let (engine_tx, engine_rx) = mpsc::channel(8);
    app.engine_tx = engine_tx;
    (app, engine_rx)
}

pub(super) fn fix_checked(
    number: u64,
    branch: Option<&str>,
    refusal: Option<&str>,
) -> crate::app::Msg {
    crate::app::Msg::Engine(luxe_core::event::Event::PullRequestFixChecked {
        number,
        branch: branch.map(str::to_string),
        refusal: refusal.map(str::to_string),
    })
}

/// Alt+U on someone else's pull request offers to fix it in a worktree instead of refusing, asks
/// GitHub first, and on Enter leaves with the unsent comments — not one already posted.
#[test]
fn sending_to_the_agent_on_their_pull_request_offers_a_worktree() {
    let (mut app, mut engine_rx) = their_pull_request();
    let posted = luxe_core::guided_review::PostedComment { review_id: 9, url: String::new() };
    let session = app.review.session_mut().expect("session");
    session.findings[0].posted = Some(posted);
    let unsent: Vec<_> = session.findings[1..].iter().map(|f| f.id.clone()).collect();

    app.update(crate::app::Msg::GuidedReviewOpenSendPreview);
    assert!(app.guided_review_workspace.send_preview.is_none(), "not the local send");
    let Ok(Action::CheckPullRequestFix { number, repository }) = engine_rx.try_recv() else {
        panic!("expected a check");
    };
    assert_eq!((number, repository), (42, None));
    let fix = app.guided_review_workspace.pull_request_fix.clone().expect("the offer");
    assert_eq!((fix.comments, fix.branch), (2, None));

    // Nothing leaves before GitHub has answered, nor for another pull request's answer
    app.update(key(crossterm::event::KeyCode::Enter));
    app.update(fix_checked(7, Some("other"), None));
    app.update(key(crossterm::event::KeyCode::Enter));
    assert!(app.outbox.pull_request_fix.is_none());

    app.update(fix_checked(42, Some("fix-redirects"), None));
    app.update(key(crossterm::event::KeyCode::Enter));
    let launch = app.outbox.pull_request_fix.clone().expect("leaving to fix it");
    assert_eq!(launch.number, 42);
    assert_eq!(launch.carried.iter().map(|f| f.id.clone()).collect::<Vec<_>>(), unsent);
    assert!(app.guided_review_workspace.pull_request_fix.is_none());

    let mut exit = crate::main_loop::TuiExit::Quit;
    crate::main_loop::collect_exit_for_tests(&mut app, &mut exit);
    assert_eq!(exit, crate::main_loop::TuiExit::FixPullRequest(launch));
}

/// From the post preview, `w` makes the same offer; a refusal says why and Enter does nothing.
#[test]
fn a_refused_fix_says_why_and_goes_nowhere() {
    let (mut app, mut engine_rx) = their_pull_request();
    // Read from another repository than the checkout's, which the engine has to be told
    app.guided_review.as_mut().expect("review").target =
        luxe_core::guided_review::ReviewTarget::PullRequest {
            number: 42,
            repository: Some("upstream/notify".into()),
        };
    app.update(crate::app::Msg::GuidedReviewOpenGithubPost);
    let _ = engine_rx.try_recv();
    app.update(key(crossterm::event::KeyCode::Char('w')));
    assert!(app.guided_review_workspace.github_post.is_none(), "the post preview gives way");
    let Ok(Action::CheckPullRequestFix { number: 42, repository }) = engine_rx.try_recv() else {
        panic!("expected a check");
    };
    assert_eq!(repository.as_deref(), Some("upstream/notify"));
    app.update(fix_checked(42, None, Some("The author did not allow edits by maintainers.")));
    app.update(key(crossterm::event::KeyCode::Enter));
    assert!(app.outbox.pull_request_fix.is_none());
    app.update(key(crossterm::event::KeyCode::Esc));
    assert!(app.guided_review_workspace.pull_request_fix.is_none());
}

/// With no engine to ask, `w` leaves the post preview where it was and says it could not ask,
/// rather than closing it on nothing.
#[test]
fn a_fix_that_cannot_be_asked_about_leaves_the_post_preview() {
    let (mut app, mut engine_rx) = their_pull_request();
    app.update(crate::app::Msg::GuidedReviewOpenGithubPost);
    let _ = engine_rx.try_recv();
    drop(engine_rx);
    app.update(key(crossterm::event::KeyCode::Char('w')));
    assert!(app.guided_review_workspace.github_post.is_some(), "the post preview stays");
    assert!(app.guided_review_workspace.pull_request_fix.is_none());
    assert_eq!(app.status.toast(), Some("Could not ask whether #42 can be fixed here"));
}

/// A new session on the checked-out pull request opens its review, and the comments carried to
/// it join that review once its session is here, and go to the agent in the same save.
#[test]
fn carried_comments_join_the_checked_out_review() {
    let (from, _) = their_pull_request();
    let carried = from.review.session().expect("session").findings.clone();
    let launch = crate::main_loop::FixLaunch { number: 42, carried: carried.clone() };

    let (tx, mut engine_rx) = mpsc::channel(16);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    app.open_pull_request_fix(launch);
    let request_id = loop {
        match engine_rx.try_recv() {
            Ok(Action::LoadGuidedReview { request_id, target, .. }) => {
                assert_eq!(
                    target,
                    luxe_core::guided_review::ReviewTarget::OwnPullRequest { number: 42 }
                );
                break request_id;
            }
            Ok(_) => {}
            Err(error) => panic!("no review asked for: {error:?}"),
        }
    };
    let mut review = crate::fixture::fixture_pull_request_review();
    review.target = luxe_core::guided_review::ReviewTarget::OwnPullRequest { number: 42 };
    // Checked out, a pull request is reviewed from where it branched, like any branch
    review.source.base = luxe_core::guided_review::ReviewBase::MergeBase {
        reference: "main".into(),
        oid: "4b825dc642cb6eb9a060e54bf8d69288fbee4904".into(),
    };
    app.update(crate::app::Msg::Engine(luxe_core::event::Event::GuidedReviewLoaded {
        request_id: request_id.clone(),
        review: Box::new(review),
    }));
    let bound = app.review.session.clone().expect("a session for the review");
    app.update(crate::app::Msg::Engine(luxe_core::event::Event::ReviewSessionRestored {
        session: Box::new(bound.clone()),
    }));

    let adopted =
        std::iter::from_fn(|| engine_rx.try_recv().ok()).find_map(|action| match action {
            Action::EditReview {
                request_id,
                edit: luxe_core::engine::ReviewEdit::AdoptCarried { findings, send },
                ..
            } => Some((request_id, findings, send)),
            _ => None,
        });
    let (save, findings, send) = adopted.expect("the carried comments are sent to the desk");
    assert_eq!(findings, carried);
    assert!(send, "choosing to fix them was the instruction");
    assert!(app.carried_findings.is_none(), "adopted once");

    // The desk adopts them and queues the unsent ones in the same save
    let mut stored = bound;
    stored.adopt_carried_findings(carried.clone(), |_| {
        luxe_core::guided_review::FindingAnchorRefresh::Exact
    });
    let unsent = stored.unsent_finding_ids();
    stored
        .queue_submission(
            luxe_core::guided_review::SubmissionId::new(),
            luxe_core::guided_review::ReviewFeedbackMode::RequestFixes,
            &unsent,
            &[],
            0,
        )
        .expect("queued");
    app.update(crate::app::Msg::Engine(luxe_core::event::Event::ReviewSessionSaved {
        request_id: save,
        session: Box::new(stored),
    }));

    // The agent is at work on them, so the transcript stays up and the review waits on Ctrl+G
    assert!(!app.review.open_when_ready);
    let review = app.guided_review.clone().expect("loaded");
    app.update(crate::app::Msg::Engine(luxe_core::event::Event::GuidedReviewGuideUpdated {
        request_id,
        review_id: review.id.clone(),
        revision_id: review.revision_id.clone(),
        chapters: review.chapters.clone(),
        provenance: luxe_core::guided_review::GuideProvenance::Model { batches: 1 },
        brief: None,
    }));
    assert_ne!(app.current_view, crate::app::ViewMode::GuidedReview);
    assert_eq!(
        app.status.toast(),
        Some("Your comments went to the agent · ctrl+g opens the review")
    );
}

/// Carried comments wait for their own pull request's review, not whichever loads first.
#[test]
fn carried_comments_wait_for_their_own_pull_request() {
    let (from, _) = their_pull_request();
    let carried = from.review.session().expect("session").findings.clone();
    let (tx, mut engine_rx) = mpsc::channel(16);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    let ids: Vec<_> = carried.iter().map(|finding| finding.id.clone()).collect();
    app.open_pull_request_fix(crate::main_loop::FixLaunch { number: 7, carried });
    let request_id = std::iter::from_fn(|| engine_rx.try_recv().ok())
        .find_map(|action| match action {
            Action::LoadGuidedReview { request_id, .. } => Some(request_id),
            _ => None,
        })
        .expect("a review asked for");
    let mut review = crate::fixture::fixture_pull_request_review();
    review.target = luxe_core::guided_review::ReviewTarget::OwnPullRequest { number: 42 };
    app.update(crate::app::Msg::Engine(luxe_core::event::Event::GuidedReviewLoaded {
        request_id,
        review: Box::new(review),
    }));
    let session = app.review.session.clone().expect("the review's own session");
    app.update(crate::app::Msg::Engine(luxe_core::event::Event::ReviewSessionRestored {
        session: Box::new(session.clone()),
    }));
    assert!(app.carried_findings.is_some(), "still waiting for #7");
    assert!(session.findings.iter().all(|finding| !ids.contains(&finding.id)), "none adopted");
    assert!(
        std::iter::from_fn(|| engine_rx.try_recv().ok())
            .all(|action| !matches!(action, Action::EditReview { .. })),
        "nothing is sent"
    );
}

/// Checked out with no comments to carry, nothing is sent to the agent.
#[test]
fn a_fix_with_no_comments_says_nothing_came() {
    let (tx, mut engine_rx) = mpsc::channel(16);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    app.open_pull_request_fix(crate::main_loop::FixLaunch { number: 42, carried: Vec::new() });
    let request_id = std::iter::from_fn(|| engine_rx.try_recv().ok())
        .find_map(|action| match action {
            Action::LoadGuidedReview { request_id, .. } => Some(request_id),
            _ => None,
        })
        .expect("a review asked for");
    let mut review = crate::fixture::fixture_pull_request_review();
    review.target = luxe_core::guided_review::ReviewTarget::OwnPullRequest { number: 42 };
    app.update(crate::app::Msg::Engine(luxe_core::event::Event::GuidedReviewLoaded {
        request_id,
        review: Box::new(review),
    }));
    let session = app.review.session.clone().expect("the review's own session");
    app.update(crate::app::Msg::Engine(luxe_core::event::Event::ReviewSessionRestored {
        session: Box::new(session),
    }));
    assert!(app.carried_findings.is_none());
    assert!(
        std::iter::from_fn(|| engine_rx.try_recv().ok())
            .all(|action| !matches!(action, Action::EditReview { .. })),
        "nothing to send"
    );
}

/// A branch's review with two chapters worth a pull request each.
pub(super) fn two_chapter_review() -> (App, mpsc::Receiver<Action>) {
    let (mut app, engine_rx) = branch_review(luxe_core::workspace::PullRequestPresence::None);
    let review = app.guided_review.as_mut().expect("review");
    let mut second = review.chapters[0].clone();
    second.title = "Cover the edge".into();
    second.kind = luxe_core::guided_review::ChapterKind::Test;
    review.chapters.push(second);
    (app, engine_rx)
}

pub(super) fn layer(head: &str, base: &str, title: &str) -> luxe_core::github::NewPullRequest {
    luxe_core::github::NewPullRequest {
        head: head.into(),
        base: base.into(),
        title: title.into(),
        body: String::new(),
        draft: false,
    }
}

/// Offered beside a single pull request when there are two chapters to split, not with one; the
/// preview opens exactly its layers, drafts when asked.
#[test]
fn a_review_with_two_chapters_opens_as_a_stack_as_shown() {
    use crate::guided_review::FinishReviewChoice;
    let (mut one, _) = branch_review(luxe_core::workspace::PullRequestPresence::None);
    assert!(dispatch_action(&mut one, UiAction::ReviewFinish));
    assert!(!one.guided_review_workspace.finish_offers_stack, "one chapter: one pull request");

    let (mut app, mut engine_rx) = two_chapter_review();
    assert!(dispatch_action(&mut app, UiAction::ReviewFinish));
    assert!(app.guided_review_workspace.finish_offers_stack);
    while app.guided_review_workspace.finish_review_choice != Some(FinishReviewChoice::OpenStack) {
        app.guided_review_workspace.move_finish_review_choice(true);
    }
    app.update(key(crossterm::event::KeyCode::Enter));
    let Ok(Action::PrepareStack { request_id, .. }) = engine_rx.try_recv() else {
        panic!("expected a prepare");
    };
    let layers = vec![layer("fix-part-1", "main", "Sum"), layer("fix", "fix-part-1", "Cover")];
    app.update(crate::app::Msg::Engine(luxe_core::event::Event::StackPrepared {
        request_id: luxe_core::guided_review::PullRequestOpenId::new(),
        layers: Some(Vec::new()),
        refusal: None,
    }));
    assert!(app.guided_review_workspace.stack_opening.as_ref().unwrap().layers.is_none(), "stale");
    app.update(crate::app::Msg::Engine(luxe_core::event::Event::StackPrepared {
        request_id,
        layers: Some(layers.clone()),
        refusal: None,
    }));
    app.update(key(crossterm::event::KeyCode::Char('d')));
    app.update(key(crossterm::event::KeyCode::Enter));
    // The engine kept the layers; whether they are drafts is the reader's one choice
    let Ok(Action::OpenStack { request_id, draft }) = engine_rx.try_recv() else {
        panic!("expected an open");
    };
    assert!(draft, "d made them drafts");
    app.update(key(crossterm::event::KeyCode::Esc));
    assert!(app.guided_review_workspace.stack_opening.is_some(), "not closed mid-open");

    app.update(crate::app::Msg::Engine(luxe_core::event::Event::StackOpened {
        request_id,
        numbers: vec![170, 171],
        urls: vec!["u170".into(), "u171".into()],
        failed: None,
    }));
    assert!(app.guided_review_workspace.stack_opening.is_none());
    assert_eq!(
        app.guided_review_workspace.posted_review_url.as_deref(),
        Some("u171"),
        "o: the top"
    );
    assert!(app.convo.iter().any(|item| matches!(item.block(), ConvoBlock::System(text)
        if text.ends_with("opened it as a stack of 2: #170, #171"))));
}

/// When the stack stops partway, the preview says which opened and offers nothing to open again.
#[test]
fn a_stack_that_stopped_partway_says_what_opened() {
    let (mut app, mut engine_rx) = two_chapter_review();
    app.update(crate::app::Msg::GuidedReviewOpenStack);
    let Ok(Action::PrepareStack { request_id, .. }) = engine_rx.try_recv() else {
        panic!("expected a prepare");
    };
    app.update(crate::app::Msg::Engine(luxe_core::event::Event::StackPrepared {
        request_id,
        layers: Some(vec![layer("a", "main", "A"), layer("b", "a", "B")]),
        refusal: None,
    }));
    app.update(crate::app::Msg::GuidedReviewConfirmStack);
    let Ok(Action::OpenStack { request_id, .. }) = engine_rx.try_recv() else {
        panic!("expected an open");
    };
    app.update(crate::app::Msg::Engine(luxe_core::event::Event::StackOpened {
        request_id,
        numbers: vec![170],
        urls: vec!["u170".into()],
        failed: Some(
            "`b` was pushed, but its pull request did not open: GitHub refused it.".into(),
        ),
    }));
    let stack = app.guided_review_workspace.stack_opening.as_ref().expect("still open");
    assert!(!stack.opening && stack.layers.is_none());
    assert!(stack.error.as_deref().unwrap().ends_with("Opened so far: #170."), "{:?}", stack.error);
    app.update(crate::app::Msg::GuidedReviewConfirmStack);
    assert!(engine_rx.try_recv().is_err(), "nothing opens twice");
    app.update(key(crossterm::event::KeyCode::Esc));
    assert!(app.guided_review_workspace.stack_opening.is_none(), "closes when idle");
}
