//! Answering — your own pull request's threads, sent as shown, and the worktree check.

use super::*;

pub(super) fn key(code: crossterm::event::KeyCode) -> crate::app::Msg {
    crate::app::Msg::Key(crossterm::event::KeyEvent::new(
        code,
        crossterm::event::KeyModifiers::NONE,
    ))
}

/// Your own pull request, its threads adopted, the answers preview closed.
pub(super) fn own_pull_request() -> (App, mpsc::Receiver<Action>) {
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewOwnPullRequest).expect("fixture");
    app.guided_review_workspace.thread_answers = None;
    let (engine_tx, engine_rx) = mpsc::channel(8);
    app.engine_tx = engine_tx;
    (app, engine_rx)
}

/// On your own pull request the finish chooser keeps its local choices — committing the fixes is
/// one — and adds answering on GitHub, which asks whether the fix is there before anything else.
#[test]
fn finishing_your_own_pull_request_offers_answering_on_github() {
    let (mut app, mut engine_rx) = own_pull_request();
    assert!(dispatch_action(&mut app, UiAction::ReviewFinish));
    assert!(app.guided_review_workspace.finish_offers_answers);
    let mut seen = Vec::new();
    for _ in 0..6 {
        app.guided_review_workspace.move_finish_review_choice(true);
        seen.push(app.guided_review_workspace.finish_review_choice.expect("open"));
    }
    assert!(seen.contains(&crate::guided_review::FinishReviewChoice::AnswerOnGitHub), "{seen:?}");
    assert!(seen.contains(&crate::guided_review::FinishReviewChoice::AcceptAndCommit));
    while app.guided_review_workspace.finish_review_choice
        != Some(crate::guided_review::FinishReviewChoice::AnswerOnGitHub)
    {
        app.guided_review_workspace.move_finish_review_choice(true);
    }
    app.update(key(crossterm::event::KeyCode::Enter));

    assert!(app.guided_review_workspace.finish_review_choice.is_none());
    let answers = app.guided_review_workspace.thread_answers.as_ref().expect("preview");
    assert_eq!(answers.rows.len(), 3, "two threads and a conversation comment");
    assert!(answers.rows[0].resolve, "the verified thread resolves by default");
    assert!(!answers.rows[1].resolve, "the unfixed one does not");
    assert!(!answers.rows[2].resolve, "nor does the conversation, which cannot be resolved");
    assert!(matches!(engine_rx.try_recv(), Ok(Action::PrepareThreadAnswers { .. })));

    // Someone else's pull request, or a working tree, offers no such choice
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewFindings).expect("fixture");
    assert!(dispatch_action(&mut app, UiAction::ReviewFinish));
    assert!(!app.guided_review_workspace.finish_offers_answers);
}

/// What is sent is what the preview shows: the reader's reply in their words, the resolves they
/// chose, and nothing for a thread left alone.
#[test]
fn answers_are_sent_as_shown_with_the_readers_own_reply() {
    let (mut app, mut engine_rx) = own_pull_request();
    app.update(crate::app::Msg::GuidedReviewOpenThreadAnswers);
    let _ = engine_rx.try_recv();

    // A reply on the second thread, typed with real keys; Enter is a new line, not a send
    app.update(key(crossterm::event::KeyCode::Down));
    app.update(key(crossterm::event::KeyCode::Char('c')));
    for character in "Not fixed yet.".chars() {
        app.update(key(crossterm::event::KeyCode::Char(character)));
    }
    app.update(key(crossterm::event::KeyCode::Enter));
    for character in "Next push.".chars() {
        app.update(key(crossterm::event::KeyCode::Char(character)));
    }
    assert!(engine_rx.try_recv().is_err(), "typing sends nothing");
    app.update(crate::app::Msg::Key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Enter,
        crossterm::event::KeyModifiers::CONTROL,
    )));
    // Sending is refused while a reply is still being written
    app.update(key(crossterm::event::KeyCode::Char('c')));
    app.update(crate::app::Msg::GuidedReviewConfirmThreadAnswers);
    assert!(engine_rx.try_recv().is_err(), "not while replying");
    app.update(key(crossterm::event::KeyCode::Esc));
    // and resolving the first stops, so it is left alone entirely
    app.update(key(crossterm::event::KeyCode::Up));
    app.update(key(crossterm::event::KeyCode::Char(' ')));
    app.update(key(crossterm::event::KeyCode::Enter));

    let Ok(Action::AnswerThreads { answers, .. }) = engine_rx.try_recv() else {
        panic!("expected answers");
    };
    assert_eq!(answers.len(), 1, "a thread neither resolved nor replied to is not sent");
    assert_eq!(answers[0].reply.as_deref(), Some("Not fixed yet.\nNext push."), "the Esc kept it");
    assert!(!answers[0].resolve);
    // Named by comment: which thread that is, the engine reads from the session
    let session = app.review.session().expect("session");
    let finding = session.findings.iter().find(|f| f.id == answers[0].finding_id).unwrap();
    assert!(finding.github_thread.is_some(), "a comment that came from a thread");

    // In flight, the preview cannot be closed
    app.update(key(crossterm::event::KeyCode::Esc));
    assert!(app.guided_review_workspace.thread_answers.as_ref().is_some_and(|a| a.sending));
}

/// Results mark what landed in one save; a thread that failed stays on screen saying why, and a
/// preview with nothing left closes.
#[test]
fn answered_threads_are_marked_and_failed_ones_stay_with_why() {
    let (mut app, mut engine_rx) = own_pull_request();
    app.update(crate::app::Msg::GuidedReviewOpenThreadAnswers);
    let _ = engine_rx.try_recv();
    let rows = app.guided_review_workspace.thread_answers.as_ref().expect("preview").rows.clone();
    app.update(crate::app::Msg::GuidedReviewToggleThreadResolve);
    app.update(crate::app::Msg::GuidedReviewToggleThreadResolve);
    app.update(crate::app::Msg::GuidedReviewMoveThreadAnswers(true));
    app.update(crate::app::Msg::GuidedReviewToggleThreadResolve);
    app.update(crate::app::Msg::GuidedReviewConfirmThreadAnswers);
    let Ok(Action::AnswerThreads { request_id, .. }) = engine_rx.try_recv() else {
        panic!("expected answers");
    };

    app.update(crate::app::Msg::Engine(luxe_core::event::Event::ThreadsAnswered {
        request_id: request_id.clone(),
        results: vec![
            luxe_core::guided_review::ThreadAnswerResult {
                finding_id: rows[0].finding_id.clone(),
                reply_url: None,
                resolved: true,
                error: None,
            },
            luxe_core::guided_review::ThreadAnswerResult {
                finding_id: rows[1].finding_id.clone(),
                reply_url: None,
                resolved: false,
                error: Some("GitHub refused it: Resource not accessible by integration.".into()),
            },
        ],
    }));

    let answers = app.guided_review_workspace.thread_answers.as_ref().expect("still open");
    // The conversation comment was not answered, so it stays too
    assert_eq!(answers.rows.len(), 2);
    assert_eq!(answers.rows[0].finding_id, rows[1].finding_id, "the one that failed stays");
    assert_eq!(answers.rows[1].finding_id, rows[2].finding_id);
    assert!(answers.rows[0].error.as_deref().is_some_and(|error| error.contains("not accessible")));
    let mut refreshed = false;
    while let Ok(action) = engine_rx.try_recv() {
        match action {
            Action::RefreshWorkspace => refreshed = true,
            // The engine records what landed on the session as it answers
            Action::EditReview { .. } => panic!("the TUI does not mark answered threads itself"),
            _ => {}
        }
    }
    assert!(refreshed, "the bar's thread count changed on GitHub, so it is asked again at once");
    assert!(matches!(
        app.convo.last().map(|item| item.block()),
        Some(ConvoBlock::System(text))
            if text == "[review] Answered 1 thread on GitHub; 1 thread did not go, and say why."
    ));

    // A stale answer changes nothing
    app.update(crate::app::Msg::Engine(luxe_core::event::Event::ThreadsAnswered {
        request_id: luxe_core::guided_review::ThreadAnswersId::new(),
        results: Vec::new(),
    }));
    assert_eq!(app.guided_review_workspace.thread_answers.as_ref().expect("open").rows.len(), 2);
}

/// A comment in the conversation has no resolve to toggle, and says so, pointing at the reply.
#[test]
fn a_conversation_comment_has_nothing_to_resolve() {
    let (mut app, mut engine_rx) = own_pull_request();
    app.update(crate::app::Msg::GuidedReviewOpenThreadAnswers);
    let _ = engine_rx.try_recv();
    app.update(crate::app::Msg::GuidedReviewMoveThreadAnswers(true));
    app.update(crate::app::Msg::GuidedReviewMoveThreadAnswers(true));
    app.update(crate::app::Msg::GuidedReviewToggleThreadResolve);
    let answers = app.guided_review_workspace.thread_answers.as_ref().expect("preview");
    assert!(!answers.rows[2].resolve);
    assert_eq!(
        app.status.toast(),
        Some("A comment in the conversation has nothing to resolve: c replies")
    );
}

/// A thread GitHub will not let this account resolve or reply to says so instead of toggling,
/// while its neighbor that allows it toggles as usual.
#[test]
fn a_thread_you_may_not_resolve_says_so() {
    let (mut app, mut engine_rx) = own_pull_request();
    let second = app
        .review
        .session()
        .expect("session")
        .findings
        .iter()
        .filter(|finding| finding.github_thread.is_some())
        .nth(1)
        .map(|finding| finding.id.clone())
        .expect("two threads");
    for finding in &mut app.review.session_mut().expect("session").findings {
        if finding.id == second {
            let thread = finding.github_thread.as_mut().expect("thread");
            thread.can_resolve = false;
            thread.can_reply = false;
        }
    }
    app.update(crate::app::Msg::GuidedReviewOpenThreadAnswers);
    let _ = engine_rx.try_recv();
    let resolve = |app: &App, row: usize| {
        app.guided_review_workspace.thread_answers.as_ref().unwrap().rows[row].resolve
    };
    assert!(resolve(&app, 0));
    app.update(crate::app::Msg::GuidedReviewToggleThreadResolve);
    assert!(!resolve(&app, 0), "the first may be resolved, so it toggles");
    app.update(crate::app::Msg::GuidedReviewMoveThreadAnswers(true));
    app.update(crate::app::Msg::GuidedReviewToggleThreadResolve);
    assert!(!resolve(&app, 1));
    assert!(app.status.toast().is_some_and(|toast| toast.contains("resolve")));
    app.update(crate::app::Msg::GuidedReviewBeginThreadReply);
    assert!(app.guided_review_workspace.thread_answers.as_ref().unwrap().replying.is_none());
    app.update(crate::app::Msg::GuidedReviewConfirmThreadAnswers);
    assert!(engine_rx.try_recv().is_err(), "nothing to send");
}

/// Whether the fix is on GitHub reaches the preview, and a failure before any thread was touched
/// stops the spinner and says why.
#[test]
fn preparing_and_failing_reach_the_preview() {
    let (mut app, mut engine_rx) = own_pull_request();
    app.update(crate::app::Msg::GuidedReviewOpenThreadAnswers);
    let Ok(Action::PrepareThreadAnswers { request_id, .. }) = engine_rx.try_recv() else {
        panic!("expected a prepare");
    };
    app.update(crate::app::Msg::Engine(luxe_core::event::Event::ThreadAnswersPrepared {
        request_id: request_id.clone(),
        pushed: true,
        uncommitted: false,
    }));
    assert_eq!(
        app.guided_review_workspace.thread_answers.as_ref().unwrap().prepared,
        Some((true, false))
    );

    app.update(crate::app::Msg::GuidedReviewConfirmThreadAnswers);
    let Ok(Action::AnswerThreads { request_id, .. }) = engine_rx.try_recv() else {
        panic!("expected answers");
    };
    app.update(crate::app::Msg::Engine(luxe_core::event::Event::ThreadAnswersFailed {
        request_id,
        message: "The GitHub CLI is not signed in.".into(),
    }));
    let answers = app.guided_review_workspace.thread_answers.as_ref().unwrap();
    assert!(!answers.sending);
    assert_eq!(answers.error.as_deref(), Some("The GitHub CLI is not signed in."));
}

/// `/worktree` says where the session works. `remove` asks git, through the engine, what is
/// uncommitted right now — a snapshot can predate a file made outside Luxe — and is refused,
/// naming it, unless nothing is; then the session ends with its worktree to remove.
#[test]
fn worktree_says_where_and_removes_only_committed_work() {
    let registry = crate::commands::CommandRegistry::all();
    let last = |app: &App| match app.convo.last().map(|item| item.block()) {
        Some(ConvoBlock::System(text)) => text.clone(),
        other => panic!("{other:?}"),
    };
    let mut app = seeded_app("nord", FixtureScenario::DockWide).expect("fixture");
    let (engine_tx, mut engine_rx) = mpsc::channel(8);
    app.engine_tx = engine_tx;
    run(&mut app, &registry.parse_slash("/worktree").expect("parses"));
    assert!(last(&app).starts_with("This session works in your checkout."), "{}", last(&app));
    run(&mut app, &registry.parse_slash("/worktree remove").expect("parses"));
    assert!(last(&app).contains("no worktree of its own"), "{}", last(&app));
    assert!(engine_rx.try_recv().is_err());

    let workspace = app.workspace.as_mut().expect("workspace");
    workspace.worktree = Some(".luxe/worktrees/topic".into());
    workspace.project.git_branch = Some("topic".into());
    let root = workspace.project.root.clone();
    run(&mut app, &registry.parse_slash("/worktree").expect("parses"));
    assert!(
        last(&app).starts_with(
            "This session works in its own worktree, `.luxe/worktrees/topic`, on branch `topic`."
        ),
        "{}",
        last(&app)
    );

    // Git says something is uncommitted: refused, naming it
    run(&mut app, &registry.parse_slash("/worktree remove").expect("parses"));
    assert!(matches!(engine_rx.try_recv(), Ok(Action::CheckWorktreeRemoval)));
    let files: Vec<String> = (1..=7).map(|n| format!("stray-{n}.txt")).collect();
    app.update(crate::app::Msg::Engine(luxe_core::event::Event::WorktreeRemovalChecked {
        uncommitted: files,
    }));
    assert!(last(&app).starts_with("The worktree has work that is not committed: stray-1.txt"));
    assert!(last(&app).contains("and 2 more"), "{}", last(&app));
    assert_eq!(app.outbox.worktree_removal, None);

    // An answer nobody asked for changes nothing
    app.update(crate::app::Msg::Engine(luxe_core::event::Event::WorktreeRemovalChecked {
        uncommitted: Vec::new(),
    }));
    assert_eq!(app.outbox.worktree_removal, None);

    // Nothing uncommitted: the session ends to remove it
    run(&mut app, &registry.parse_slash("/worktree remove").expect("parses"));
    assert!(matches!(engine_rx.try_recv(), Ok(Action::CheckWorktreeRemoval)));
    app.update(crate::app::Msg::Engine(luxe_core::event::Event::WorktreeRemovalChecked {
        uncommitted: Vec::new(),
    }));
    assert_eq!(app.outbox.worktree_removal, Some(root));

    run(&mut app, &registry.parse_slash("/worktree prune").expect("parses"));
    assert!(last(&app).contains("is not something it does"), "{}", last(&app));
}
