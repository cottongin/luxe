//! Palette — the command list, what it promises, and what refuses to run.

use super::*;

/// A place a palette opens, named, and how to build an app standing there.
pub(super) type Place = (&'static str, fn() -> App);

/// The places a palette opens, built the way a reader gets there.
pub(super) fn places() -> Vec<Place> {
    fn fresh() -> App {
        let (tx, _rx) = mpsc::channel(64);
        App::new(tx, KeyMap::defaults(), Config::default())
    }
    fn seeded(scenario: FixtureScenario) -> App {
        let mut app = seeded_app("nord", scenario).expect("fixture app");
        // Room for everything a command may send, so a full channel cannot pass for a refusal
        let (tx, rx) = mpsc::channel(64);
        std::mem::forget(rx);
        app.engine_tx = tx;
        app
    }
    vec![
        ("a fresh session", fresh),
        ("a conversation", || seeded(FixtureScenario::ChatIdle)),
        ("a conversation with an image", || seeded(FixtureScenario::ChatScreenshot)),
        ("messages waiting", || seeded(FixtureScenario::ChatPending)),
        ("the preview", || seeded(FixtureScenario::ChangesWide)),
        ("a review", || seeded(FixtureScenario::GuidedReviewReady)),
        ("a pull request's review", || seeded(FixtureScenario::GuidedReviewPullRequest)),
    ]
}

/// Every command a menu lists is one its dispatch handles, wherever it is listed.
///
/// The palette used to list 36 commands an idle session could not run, and picking one closed it
/// on nothing. A command cannot be added to the list now without being wired where it is listed:
/// each place builds the app fresh for each action, so one command's effect cannot mask another's.
#[test]
fn every_listed_command_runs_where_it_is_listed() {
    let mut listed_anywhere = std::collections::HashSet::new();
    for (place, build) in places() {
        let menu = command_menu(&build());
        assert!(!menu.entries.is_empty(), "{place} lists nothing");
        for entry in menu.entries {
            listed_anywhere.insert(entry.command.id.to_string());
            if let crate::commands::CommandKind::Action(action) = entry.command.kind {
                let mut app = build();
                assert!(
                    dispatch_action(&mut app, action),
                    "{place} lists {:?}, and running it does nothing",
                    entry.command.name
                );
                // What would leave Luxe is recorded, never launched: this loop runs every command
                // on every run of the suite, and it once opened a browser tab each time
                let crate::app::ExternalOpener::Recorded(opened) = &app.opener else {
                    panic!("{place}: a test app must not be able to launch anything");
                };
                if action == UiAction::ReviewOpenInBrowser {
                    assert_eq!(opened.len(), 1, "{place}: Open on GitHub asks to open one page");
                    assert!(opened[0].contains("/pull/"), "{place}: {opened:?}");
                }
            }
        }
    }
    for command in crate::commands::CommandRegistry::all().iter() {
        assert!(
            listed_anywhere.contains(command.id.as_ref()),
            "{:?} is listed nowhere a reader can reach",
            command.name
        );
    }
}

/// The places report themselves the way the list needs them to.
#[test]
fn each_place_reports_what_it_offers() {
    use crate::commands::Surface;
    let context_of = |scenario| command_context(&seeded_app("nord", scenario).expect("fixture"));
    let fresh = {
        let (tx, _rx) = mpsc::channel(1);
        command_context(&App::new(tx, KeyMap::defaults(), Config::default()))
    };
    assert_eq!(fresh, crate::commands::CommandContext::default(), "a fresh session offers nothing");
    assert!(context_of(FixtureScenario::ChatIdle).has_turn);
    assert!(context_of(FixtureScenario::ChatScreenshot).has_image);
    assert!(context_of(FixtureScenario::ChatPending).messages_waiting);
    assert!(context_of(FixtureScenario::ChangesWide).changes_pane);
    assert_eq!(context_of(FixtureScenario::GuidedReviewReady).surface, Surface::Review);
    assert!(!context_of(FixtureScenario::GuidedReviewReady).pull_request_review);
    assert!(context_of(FixtureScenario::GuidedReviewPullRequest).pull_request_review);
}

pub(super) fn command(slash_or_id: &str) -> crate::commands::Command {
    let registry = crate::commands::CommandRegistry::all();
    registry
        .parse_slash(slash_or_id)
        .or_else(|| registry.iter().find(|command| command.id == slash_or_id).cloned())
        .unwrap_or_else(|| panic!("{slash_or_id} is a command"))
}

pub(super) fn toast(app: &App) -> Option<&str> {
    app.status.toast.as_ref().map(|(said, _)| said.as_str())
}

/// A command that cannot run here says what it needs, runs nothing, and is not remembered as
/// used; one that ran leads the next menu.
#[test]
fn a_command_that_cannot_run_says_why_and_one_that_ran_is_remembered() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());

    dispatch(&mut app, command("/pr"));
    assert_eq!(toast(&app), Some("Pull request needs a git checkout"));
    assert!(rx.try_recv().is_err(), "nothing was asked of the engine");
    assert!(app.recent_commands.ids().is_empty(), "a refusal is not a use");

    dispatch(&mut app, command("tui.preview.toggle_word_wrap"));
    assert_eq!(toast(&app), Some("Wrap long lines needs the preview open"));

    dispatch(&mut app, command("/theme"));
    assert!(app.overlays.theme_picker.is_some());
    dispatch(&mut app, command("/cost"));
    assert_eq!(app.recent_commands.ids(), ["cmd.cost", "cmd.theme"]);
    let menu = command_menu(&app);
    assert_eq!(menu.recent, 2);
    assert_eq!(menu.entries[0].command.name, "Session cost");
}

/// The safety net under the list: an action that refuses when it is run says so, rather than
/// closing the palette on nothing.
#[test]
fn an_action_that_refuses_says_so() {
    let (tx, _rx) = mpsc::channel(8);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    let mut motion = command("tui.review.finish");
    motion.needs = std::borrow::Cow::Borrowed(&[]);
    dispatch(&mut app, motion);
    assert_eq!(toast(&app), Some("Finish review did nothing here"));
    assert!(app.recent_commands.ids().is_empty());
}

/// `/help` is the key card: the chat's in the conversation, the review's in a review. Esc or a
/// click puts the chat's away, it swallows what is typed or pasted while it is up, and a key
/// someone binds to help opens and closes it too.
#[test]
fn help_opens_the_key_card_for_what_is_on_screen() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let (tx, _rx) = mpsc::channel(8);
    let keymap = KeyMap::defaults().with_overrides(std::collections::HashMap::from([(
        "tui.app.toggle_help".to_string(),
        vec!["f1".to_string()],
    )]));
    let mut app = App::new(tx, keymap, Config::default());

    dispatch(&mut app, command("/help"));
    assert!(app.overlays.keys_card);
    app.update(crate::app::Msg::Key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE)));
    app.update(crate::app::Msg::PasteText("sk-not-a-real-key".into()));
    assert!(app.overlays.keys_card, "a stray key does not close it");
    assert_eq!(app.editor.current_input(), "", "and nothing reaches the composer behind it");
    app.update(crate::app::Msg::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
    assert!(!app.overlays.keys_card);

    app.update(crate::app::Msg::Key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE)));
    assert!(app.overlays.keys_card, "a bound help key opens it");
    app.update(crate::app::Msg::Key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE)));
    assert!(!app.overlays.keys_card, "and closes it");

    app.overlays.keys_card = true;
    let click = crossterm::event::MouseEvent {
        kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
        column: 0,
        row: 0,
        modifiers: KeyModifiers::NONE,
    };
    if let crate::main_loop::RouteAction::Msg(message) =
        crate::main_loop::route_mouse(&mut app, &click)
    {
        app.update(*message);
    }
    assert!(!app.overlays.keys_card, "a click puts it away");

    let mut review = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("fixture app");
    dispatch(&mut review, command("cmd.help"));
    assert!(!review.overlays.keys_card, "a review has its own card");
    assert!(review.guided_review_workspace.help_visible);
}

/// The ten commands the old `/help` text never mentioned are what motivated generating help from
/// the registry. It is a card of keys now, and the text that drifted is gone.
#[test]
fn help_is_no_longer_a_hand_written_list() {
    let (tx, _rx) = mpsc::channel(8);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    dispatch(&mut app, command("/help"));
    assert!(
        !app.convo.iter().any(|item| matches!(
            item.block(),
            ConvoBlock::System(text) if text.contains("Available commands")
        )),
        "no hand-written command list lands in the transcript"
    );
}

/// Bare `/diff` is the same verb as its key: it opens the review already pinned rather than
/// creating another, which is what `Ctrl+G` does and what one row for both has to mean.
#[test]
fn bare_diff_opens_the_pinned_review_like_its_key() {
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("fixture app");
    let (tx, mut rx) = mpsc::channel(8);
    app.engine_tx = tx;
    crate::app::navigation::exit_guided_review(&mut app);
    assert_eq!(app.current_view, crate::app::ViewMode::Chat);
    // Leaving records what was read; what matters below is what `/diff` sends
    while rx.try_recv().is_ok() {}

    dispatch(&mut app, command("/diff"));
    assert_eq!(app.current_view, crate::app::ViewMode::GuidedReview, "straight back to it");
    assert!(rx.try_recv().is_err(), "without taking another");
}

/// The palette opens from anywhere, a review included, and draws there.
///
/// The review claims every key it does not know so nothing reaches the composer hidden behind it,
/// and that swallowed `Ctrl+K`; the view also returned before drawing any overlay. So the review's
/// own commands were listed and could not be reached.
#[test]
fn the_palette_opens_and_draws_in_a_review() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::{backend::TestBackend, Terminal};
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("fixture app");
    app.update(crate::app::Msg::Key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL)));
    assert!(app.overlays.palette.is_some(), "ctrl+k opens it over the review");

    let mut terminal = Terminal::new(TestBackend::new(140, 40)).expect("terminal");
    terminal.draw(|frame| crate::view::render(frame, &mut app)).expect("draw");
    let buffer = terminal.backend().buffer().clone();
    let text: String = (0..40)
        .map(|y| (0..140).map(|x| buffer[(x, y)].symbol()).collect::<String>() + "\n")
        .collect();
    assert!(text.contains("Command palette"), "and draws there");
    assert!(text.contains("Finish review"), "listing the review's commands: {text}");

    for character in "finish".chars() {
        app.update(crate::app::Msg::Key(KeyEvent::new(
            KeyCode::Char(character),
            KeyModifiers::NONE,
        )));
    }
    assert_eq!(app.overlays.palette.as_ref().map(|palette| palette.query()), Some("finish"));
    app.update(crate::app::Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
    assert!(app.overlays.palette.is_none());
    assert!(
        app.guided_review_workspace.finish_review_choice.is_some()
            || app.guided_review_workspace.github_post.is_some(),
        "and runs what it picks"
    );
}

/// An app nobody installed the desktop opener into cannot start a browser or a viewer, whatever
/// asks it to: it records the request instead, which is also what lets a test check one was made.
#[test]
fn an_app_opens_nothing_outside_luxe_unless_a_launch_says_so() {
    let (tx, _rx) = mpsc::channel(1);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    assert_eq!(app.opener, crate::app::ExternalOpener::Recorded(Vec::new()));
    app.open_externally(std::ffi::OsStr::new("https://example.test/pull/1"), "the page");
    assert_eq!(app.opener, crate::app::ExternalOpener::Recorded(vec!["the page".into()]));
    assert!(app.convo.is_empty(), "nothing failed, so nothing is said");
}

/// The round trip in `/pr`'s Conversation: the tab asks for it, `r` writes a reply in the thread,
/// Enter sends it, and what lands closes the card and asks for the conversation again to show it.
#[test]
fn a_reply_written_in_pr_is_posted_and_the_conversation_read_again() {
    use luxe_core::github::overview::{ConversationEntry, Overview};
    use luxe_core::github::{ReviewThread, ThreadComment, ThreadSide};
    use luxe_core::workspace::PullRequestPresence;
    let registry = crate::commands::CommandRegistry::all();
    let mut app = seeded_app("nord", FixtureScenario::DockWide).expect("fixture app");
    let (engine_tx, mut engine_rx) = mpsc::channel(16);
    app.engine_tx = engine_tx;
    let status = luxe_core::github::PullRequestStatus {
        number: 146,
        title: "Round trip".into(),
        url: "https://github.com/acme/app/pull/146".into(),
        state: luxe_core::github::PullRequestState::Open,
        draft: false,
        review_decision: None,
        checks: luxe_core::github::CheckRollup::default(),
        unresolved_threads: 1,
        head_oid: "f6195af".into(),
        stack: None,
    };
    app.workspace.as_mut().expect("workspace").pull_request = PullRequestPresence::Found(status);
    run(&mut app, &registry.parse_slash("/pr").expect("/pr parses"));
    let overview = luxe_core::github::overview::PullRequestOverview {
        number: 146,
        ..crate::fixture::pull_request_overview()
    };
    app.update(crate::app::Msg::Engine(luxe_core::event::Event::GithubOverviewRead {
        repository: None,
        number: 146,
        overview: Some(Overview::PullRequest(Box::new(overview))),
        error: None,
        local: None,
    }));
    let drain = |rx: &mut mpsc::Receiver<Action>| {
        std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>()
    };
    drain(&mut engine_rx);

    app.update(key(crossterm::event::KeyCode::Tab));
    assert!(drain(&mut engine_rx).iter().any(|action| matches!(
        action,
        Action::ReadGithubConversation { repository: None, number: 146, pull_request: true }
    )));
    let thread = ConversationEntry::Thread(ReviewThread {
        id: "PRRT_9".into(),
        path: "src/lib.rs".into(),
        line: Some(3),
        start_line: None,
        side: ThreadSide::Right,
        outdated: false,
        resolved: false,
        file_level: false,
        comments: vec![ThreadComment {
            author: "jon".into(),
            body: "Why twice?".into(),
            created_at: String::new(),
            url: String::new(),
        }],
        total_comments: 1,
        anchor_text: None,
        can_reply: true,
        can_resolve: true,
    });
    app.update(crate::app::Msg::Engine(luxe_core::event::Event::GithubConversationRead {
        repository: None,
        number: 146,
        entries: Some(vec![thread]),
        error: None,
    }));

    app.update(key(crossterm::event::KeyCode::Char('r')));
    for character in "Once now.".chars() {
        app.update(key(crossterm::event::KeyCode::Char(character)));
    }
    app.update(crate::app::Msg::Key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Enter,
        crossterm::event::KeyModifiers::CONTROL,
    )));
    app.update(key(crossterm::event::KeyCode::Enter));
    let sent = drain(&mut engine_rx);
    let posted = sent.iter().find_map(|action| match action {
        Action::PostGithubComment(post) => Some(post.clone()),
        _ => None,
    });
    assert_eq!(
        posted,
        Some(luxe_core::engine::GithubPost {
            repository: None,
            number: 146,
            thread: Some("PRRT_9".into()),
            answering: Some("Why twice?".into()),
            body: "Once now.".into(),
        })
    );

    app.update(crate::app::Msg::Engine(luxe_core::event::Event::GithubCommentPosted {
        repository: None,
        number: 146,
        thread: Some("PRRT_9".into()),
        url: Some("https://github.com/acme/app/pull/146#discussion_r9".into()),
        error: None,
    }));
    let card = app.overlays.pull_request_card.as_ref().expect("the card stays");
    let crate::github_detail::CardState::Ready(detail) = &card.state else { panic!("ready") };
    assert!(detail.composing.is_none(), "the draft card closed");
    assert!(
        drain(&mut engine_rx)
            .iter()
            .any(|action| matches!(action, Action::ReadGithubConversation { number: 146, .. })),
        "read again to show what was posted"
    );
}

/// `/pr` with a card on the branch's own pull request, its overview `overview`.
pub(super) fn pr_card(
    overview: luxe_core::github::overview::PullRequestOverview,
) -> (App, mpsc::Receiver<Action>) {
    let mut app = seeded_app("nord", FixtureScenario::DockWide).expect("fixture app");
    let (engine_tx, engine_rx) = mpsc::channel(32);
    app.engine_tx = engine_tx;
    let mut card = crate::github_detail::PullRequestCard::reading(overview.number, None);
    card.land(Some(luxe_core::github::overview::Overview::PullRequest(Box::new(overview))), None);
    app.overlays.pull_request_card = Some(card);
    (app, engine_rx)
}

/// `f` on failing checks writes the request into the composer, naming them, and leaves the card:
/// what the agent is asked is still the reader's to send.
#[test]
fn fix_with_agent_writes_the_request_for_the_reader_to_send() {
    let (mut app, mut engine_rx) = pr_card(crate::fixture::pull_request_overview());
    app.update(key(crossterm::event::KeyCode::Char('f')));
    assert!(app.overlays.pull_request_card.is_none());
    assert!(app.editor.current_input().contains("failing checks on pull request #142"));
    assert!(
        !std::iter::from_fn(|| engine_rx.try_recv().ok())
            .any(|action| matches!(action, Action::SubmitUserMessage { .. })),
        "nothing is sent on the reader's behalf"
    );
}

/// Enter on Ready opens the confirmation; Enter there sends exactly the merge left — the method
/// first in GitHub's order, the head read, the branch to delete. A refusal shows in the confirmation;
/// a merge that lands closes it and reads the overview and the workspace again.
#[test]
fn a_merge_is_confirmed_sent_as_left_and_its_answer_lands() {
    use luxe_core::engine::{PullRequestChange, PullRequestWrite};
    use luxe_core::github::overview::{MergeMethod, MergeRequest, MergeStatus};
    use luxe_core::github::{CheckRollup, ReviewDecision};
    let ready = luxe_core::github::overview::PullRequestOverview {
        merge_status: MergeStatus::Clean,
        review_decision: Some(ReviewDecision::Approved),
        reviewers: Vec::new(),
        checks: CheckRollup { passed: 12, ..CheckRollup::default() },
        failing: Vec::new(),
        unresolved_threads: 0,
        merge_methods: vec![MergeMethod::Squash, MergeMethod::Merge],
        ..crate::fixture::pull_request_overview()
    };
    let (mut app, mut engine_rx) = pr_card(ready.clone());
    let drain = |rx: &mut mpsc::Receiver<Action>| {
        std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>()
    };
    app.update(key(crossterm::event::KeyCode::Enter));
    app.update(key(crossterm::event::KeyCode::Enter));
    let sent = drain(&mut engine_rx);
    let asked = sent.iter().find_map(|action| match action {
        Action::ChangePullRequest {
            number: 142, write: PullRequestWrite::Merge(request), ..
        } => Some(request.clone()),
        _ => None,
    });
    assert_eq!(
        asked,
        Some(MergeRequest {
            method: MergeMethod::Merge,
            head_oid: ready.head_oid.clone(),
            delete_branch: Some("digest-routing".into()),
        })
    );

    let refused =
        "GitHub refused the merge: Head branch was modified. Review and try the merge again.";
    app.update(crate::app::Msg::Engine(luxe_core::event::Event::PullRequestChanged {
        repository: None,
        number: 142,
        change: None,
        error: Some(refused.into()),
    }));
    let dialog = |app: &App| match app.overlays.pull_request_card.as_ref().map(|card| &card.state) {
        Some(crate::github_detail::CardState::Ready(detail)) => detail.merging.clone(),
        _ => None,
    };
    assert_eq!(dialog(&app).and_then(|d| d.error), Some(refused.to_string()));

    app.update(key(crossterm::event::KeyCode::Enter));
    drain(&mut engine_rx);
    app.update(crate::app::Msg::Engine(luxe_core::event::Event::PullRequestChanged {
        repository: None,
        number: 142,
        change: Some(PullRequestChange::Merged { sha: "abc".into(), branch_deleted: true }),
        error: None,
    }));
    assert!(dialog(&app).is_none(), "the confirmation closed");
    let after = drain(&mut engine_rx);
    assert!(after
        .iter()
        .any(|action| matches!(action, Action::ReadGithubOverview { number: 142, .. })));
    assert!(after.iter().any(|action| matches!(action, Action::RefreshWorkspace)));
}
