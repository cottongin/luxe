//! GitHub — `/github`, `/stack` and the pull request a push links to.

use super::*;

/// With `gh` signed out, the GitHub tab says so and how to fix it, rather than an empty list that
/// reads as "nothing matches"; nothing is kept, so the next look asks again, and once signed in
/// the rows come.
#[test]
fn a_signed_out_github_tab_says_why_and_asks_again() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut app = App::new(tx, luxe_core::keymap::KeyMap::defaults(), Default::default());
    let key = |code| Msg::Key(KeyEvent::new(code, KeyModifiers::NONE));
    let signed_out =
        "The GitHub CLI is not signed in. Run `gh auth login` in a terminal, then try again.";
    let listed = |error: Option<&str>, items: Vec<luxe_core::github::ListItem>| {
        Msg::Engine(luxe_core::event::Event::GithubListed {
            repository: None,
            sections: vec![luxe_core::github::ListedSection {
                name: "Mine".into(),
                items,
                error: error.map(str::to_string),
                total: None,
                next: None,
            }],
        })
    };
    let drawn = |app: &mut App| {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(110, 30)).unwrap();
        terminal.draw(|frame| crate::view::render(frame, app)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..30)
            .map(|y| (0..110).map(|x| buffer[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    };
    app.update(key(KeyCode::Char('@')));
    app.update(key(KeyCode::Tab));
    app.update(key(KeyCode::Tab));
    assert!(matches!(rx.try_recv(), Ok(Action::ListGithub { .. })));
    app.update(listed(Some(signed_out), Vec::new()));
    let screen = drawn(&mut app);
    assert!(screen.contains("The GitHub CLI is not signed in. Run gh auth login"), "{screen}");
    assert!(!screen.contains("No matching") && !screen.contains("enter insert"), "{screen}");
    assert!(app.github_listing.is_none(), "a failure is not kept");

    app.update(key(KeyCode::Tab));
    app.update(key(KeyCode::Tab));
    app.update(key(KeyCode::Tab));
    assert!(matches!(rx.try_recv(), Ok(Action::ListGithub { .. })), "asked again");
    let row = luxe_core::github::ListItem {
        pull_request: true,
        number: 7,
        title: "Signed in now".into(),
        url: "https://github.com/acme/app/pull/7".into(),
        repository: "acme/app".into(),
        author: String::new(),
        draft: false,
        updated_at: String::new(),
        state: luxe_core::github::PullRequestState::Open,
        facts: None,
    };
    // One section failing beside one that lists is a partial answer, and kept
    app.update(listed(Some("GitHub refused that search."), vec![row]));
    let screen = drawn(&mut app);
    assert!(screen.contains("#7 Signed in now") && !screen.contains("refused"), "{screen}");
    assert!(app.github_listing.is_some());
}

/// The session is linked to the branch's pull request once a push the agent made has finished and
/// the workspace names one — not when the call was written, and not for a call that pushed nothing.
#[test]
fn a_finished_push_links_the_session_to_its_pull_request() {
    use luxe_core::message::{ContentBlock, Message, Role};
    let mut app = crate::fixture::seeded_app("nord", crate::fixture::FixtureScenario::DockWide)
        .expect("fixture app");
    let (engine_tx, mut engine_rx) = mpsc::channel(8);
    app.engine_tx = engine_tx;
    app.session_id = Some("session-1".into());
    let call = |id: &str, command: &str| {
        Msg::Engine(Event::MessageCommitted {
            session_id: "session-1".into(),
            entry_id: luxe_core::session::EntryId(format!("entry-{id}")),
            message: Message {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolCall {
                    id: id.into(),
                    name: "bash".into(),
                    arguments: serde_json::json!({ "command": command }),
                }],
            },
        })
    };
    let mut snapshot = app.workspace.clone().expect("workspace");
    snapshot.pull_request =
        luxe_core::workspace::PullRequestPresence::Found(luxe_core::github::PullRequestStatus {
            number: 146,
            title: "Round trip".into(),
            url: "https://github.com/acme/app/pull/146".into(),
            state: luxe_core::github::PullRequestState::Open,
            draft: false,
            review_decision: None,
            checks: luxe_core::github::CheckRollup::default(),
            unresolved_threads: 0,
            head_oid: "abc".into(),
            stack: None,
        });
    let links = |rx: &mut mpsc::Receiver<Action>| {
        let mut links = Vec::new();
        while let Ok(action) = rx.try_recv() {
            if let Action::LinkPullRequest { number, .. } = action {
                links.push(number);
            }
        }
        links
    };

    app.update(call("status", "git status"));
    app.update(Msg::Engine(Event::ToolEnded { call_id: "status".into(), is_error: false }));
    snapshot.revision += 1;
    app.update(Msg::Engine(Event::WorkspaceUpdated(Box::new(snapshot.clone()))));
    assert!(links(&mut engine_rx).is_empty(), "nothing was pushed");

    app.update(call("push", "cargo fmt && git push"));
    snapshot.revision += 1;
    app.update(Msg::Engine(Event::WorkspaceUpdated(Box::new(snapshot.clone()))));
    assert!(links(&mut engine_rx).is_empty(), "not while the push is running");
    app.update(Msg::Engine(Event::ToolEnded { call_id: "push".into(), is_error: false }));
    snapshot.revision += 1;
    app.update(Msg::Engine(Event::WorkspaceUpdated(Box::new(snapshot.clone()))));
    assert_eq!(links(&mut engine_rx), [146]);
    snapshot.revision += 1;
    app.update(Msg::Engine(Event::WorkspaceUpdated(Box::new(snapshot))));
    assert!(links(&mut engine_rx).is_empty(), "once");
}

/// `/stack` asks for a plan and shows it; while it is open nothing typed or pasted reaches the
/// composer, Enter pushes exactly that plan, and the result is said once it lands.
#[tokio::test]
async fn slash_stack_shows_the_plan_and_pushes_exactly_it() {
    let (tx, mut rx) = mpsc::channel(16);
    let mut app =
        App::new(tx, luxe_core::keymap::KeyMap::defaults(), luxe_core::config::Config::default());
    // Outside a checkout there is no stack, and it says so rather than asking the engine
    app.editor.insert_text("/stack");
    app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
    assert!(rx.try_recv().is_err());
    assert_eq!(
        app.status.toast.as_ref().map(|(said, _)| said.as_str()),
        Some("Restack the stack needs a git checkout")
    );
    app.status.git_branch = Some("feat/threads".into());
    app.editor.insert_text("/stack");
    app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
    let Ok(luxe_core::Action::PlanRestack { request_id: planned }) = rx.try_recv() else {
        panic!("expected a plan to be asked for");
    };
    assert_eq!(app.overlays.restack, Some(crate::restack_panel::RestackPanel::Planning));

    let plan = luxe_core::restack::RestackPlan {
        remote: "origin".into(),
        steps: vec![luxe_core::restack::RestackStep {
            number: 140,
            head_ref: "feat/threads".into(),
            old: "a".into(),
            new: "b".into(),
            commits: 2,
            onto: "feat/review".into(),
            retarget: None,
            checked_out: None,
            local: true,
        }],
    };
    app.update(Msg::Engine(luxe_core::event::Event::RestackPlanned {
        request_id: planned.clone(),
        plan: Some(plan),
        merged: None,
        refusal: None,
    }));
    app.update(Msg::Key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE)));
    assert!(super::input::modal_took_paste(&mut app, "pasted"));
    assert_eq!(app.editor.current_input(), "", "the panel took both");
    app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
    let Ok(luxe_core::Action::Restack { request_id: pushed }) = rx.try_recv() else {
        panic!("expected the push");
    };
    assert_eq!(pushed, planned, "the push names the plan on screen");
    assert_eq!(app.overlays.restack, Some(crate::restack_panel::RestackPanel::Pushing));
    app.update(Msg::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
    assert!(app.overlays.restack.is_some(), "not closed mid-push");

    app.update(Msg::Engine(luxe_core::event::Event::Restacked {
        request_id: planned,
        moved: vec![140, 143],
        retargeted: vec![140],
        retarget_failed: None,
    }));
    assert!(app.overlays.restack.is_none());
    assert!(app.convo.iter().any(|item| matches!(item.block(), ConvoBlock::System(text)
        if text == "[stack] Restacked #140, #143; moved the base of #140.")));

    // A refusal is said in the panel, and Esc closes it
    app.overlays.restack = Some(crate::restack_panel::RestackPanel::Planning);
    app.update(Msg::Engine(luxe_core::event::Event::RestackPlanned {
        request_id: luxe_core::restack::RestackId::new(),
        plan: None,
        merged: None,
        refusal: Some("#9 is not in a stack.".into()),
    }));
    assert_eq!(
        app.overlays.restack,
        Some(crate::restack_panel::RestackPanel::Said("#9 is not in a stack.".into()))
    );
    app.update(Msg::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
    assert!(app.overlays.restack.is_none());
}

/// `/github` asks for the configured sections and lists what comes back; Enter on a pull request
/// opens its review, on an issue leaves for a session on it, and `c` drops a reference into the
/// composer. While filtering, a paste lands in the filter, not the composer.
#[tokio::test]
async fn slash_github_lists_and_each_row_leads_somewhere() {
    let (tx, mut rx) = mpsc::channel(16);
    let mut app =
        App::new(tx, luxe_core::keymap::KeyMap::defaults(), luxe_core::config::Config::default());
    let enter = |app: &mut App, text: &str| {
        app.editor.insert_text(text);
        app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
    };
    let listed = || {
        let item = |pull_request: bool, number: u64| luxe_core::github::ListItem {
            pull_request,
            number,
            title: format!("Item {number}"),
            url: format!("https://github.com/acme/app/issues/{number}"),
            repository: "acme/app".into(),
            author: "mara".into(),
            draft: false,
            updated_at: String::new(),
            state: luxe_core::github::PullRequestState::Open,
            facts: None,
        };
        Msg::Engine(luxe_core::event::Event::GithubListed {
            repository: Some("acme/app".into()),
            sections: vec![
                luxe_core::github::ListedSection {
                    name: "Mine".into(),
                    items: vec![item(true, 12)],
                    error: None,
                    total: None,
                    next: None,
                },
                luxe_core::github::ListedSection {
                    name: "Assigned issues".into(),
                    items: vec![item(false, 7)],
                    error: None,
                    total: None,
                    next: None,
                },
            ],
        })
    };

    enter(&mut app, "/github");
    let Ok(luxe_core::Action::ListGithub { sections }) = rx.try_recv() else {
        panic!("expected the list to be asked for");
    };
    assert_eq!(sections.len(), 5, "the defaults");
    // An empty first section is passed over for the first with rows in it
    app.update(Msg::Engine(luxe_core::event::Event::GithubListed {
        repository: None,
        sections: vec![
            luxe_core::github::ListedSection {
                name: "Review requested".into(),
                items: Vec::new(),
                error: None,
                total: None,
                next: None,
            },
            luxe_core::github::ListedSection {
                name: "Mine".into(),
                items: vec![luxe_core::github::ListItem {
                    pull_request: true,
                    number: 3,
                    title: "Three".into(),
                    url: String::new(),
                    repository: "acme/app".into(),
                    author: String::new(),
                    draft: false,
                    updated_at: String::new(),
                    state: luxe_core::github::PullRequestState::Open,
                    facts: None,
                }],
                error: None,
                total: None,
                next: None,
            },
        ],
    }));
    assert_eq!(app.overlays.github.as_ref().unwrap().section, 1);
    // A refresh from there keeps its place, even with that section emptied
    app.overlays.github.as_mut().unwrap().sections = None;
    app.update(Msg::Engine(luxe_core::event::Event::GithubListed {
        repository: None,
        sections: vec![
            luxe_core::github::ListedSection {
                name: "Review requested".into(),
                items: Vec::new(),
                error: None,
                total: None,
                next: None,
            },
            luxe_core::github::ListedSection {
                name: "Mine".into(),
                items: Vec::new(),
                error: None,
                total: None,
                next: None,
            },
        ],
    }));
    assert_eq!(app.overlays.github.as_ref().unwrap().section, 1);
    app.overlays.github.as_mut().unwrap().section = 0;
    app.update(listed());
    app.update(Msg::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE)));
    assert!(app.overlays.github.is_none());
    // The same reference the `@` picker makes, one chip, and what it stands for is kept
    assert_eq!(app.editor.current_input(), "@[github:acme/app/pull/12] ");
    assert!(app.github_rows.contains_key("acme/app/pull/12"));
    app.editor.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
    app.editor.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
    assert_eq!(app.editor.current_input(), "", "deleted whole");

    app.editor.clear_selection();
    app.editor = crate::editor::Editor::new();
    enter(&mut app, "/github");
    let _ = rx.try_recv();
    app.update(listed());
    app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
    let asked = std::iter::from_fn(|| rx.try_recv().ok()).find_map(|action| match action {
        luxe_core::Action::LoadGuidedReview { target, .. } => Some(target),
        _ => None,
    });
    assert_eq!(
        asked,
        Some(luxe_core::guided_review::ReviewTarget::PullRequest { number: 12, repository: None })
    );
    assert!(app.review.open_when_ready, "it opens once whole");

    enter(&mut app, "/github");
    let _ = rx.try_recv();
    app.update(listed());
    app.update(Msg::Key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE)));
    assert!(super::input::modal_took_paste(&mut app, "Item\n"));
    let typed =
        app.overlays.github.as_ref().and_then(|p| p.editing.as_ref()).unwrap().current_input();
    assert_eq!(typed, "is:open is:pr review-requested:@me Item", "one line, after the search");
    app.update(Msg::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
    // Tab moves between sections; `→` belongs to the detail beside the list
    app.update(Msg::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)));
    app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
    assert_eq!(app.outbox.issue, Some(7));
    let mut exit = crate::main_loop::TuiExit::Quit;
    crate::main_loop::collect_exit_for_tests(&mut app, &mut exit);
    assert_eq!(exit, crate::main_loop::TuiExit::TakeUpIssue(7));
}

/// A tab's edited search runs on its own and lands in that tab, and it is the tab's for the rest
/// of the session: `/github` opened again asks for it, not the configured one, until `x` puts the
/// configured one back.
#[tokio::test]
async fn an_edited_search_lands_in_its_tab_and_lasts_the_session() {
    let (tx, mut rx) = mpsc::channel(16);
    let mut app =
        App::new(tx, luxe_core::keymap::KeyMap::defaults(), luxe_core::config::Config::default());
    let key = |app: &mut App, code: KeyCode| {
        app.update(Msg::Key(KeyEvent::new(code, KeyModifiers::NONE)));
    };
    let open = |app: &mut App, rx: &mut mpsc::Receiver<Action>| {
        app.editor.insert_text("/github");
        app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
        let Ok(Action::ListGithub { sections }) = rx.try_recv() else {
            panic!("expected the list to be asked for");
        };
        let listed = sections
            .iter()
            .map(|section| luxe_core::github::ListedSection {
                name: section.name.clone(),
                items: Vec::new(),
                error: None,
                total: Some(0),
                next: None,
            })
            .collect();
        app.update(Msg::Engine(luxe_core::event::Event::GithubListed {
            repository: Some("acme/app".into()),
            sections: listed,
        }));
        sections
    };
    open(&mut app, &mut rx);
    app.overlays.github.as_mut().unwrap().section = 3;
    key(&mut app, KeyCode::Char('s'));
    let Ok(Action::SearchGithub { request, section, after: None }) = rx.try_recv() else {
        panic!("expected the tab to be searched again");
    };
    assert_eq!(section.query, "is:closed is:unmerged is:pr sort:updated-desc repo:{repo}");
    assert!(app.panel_waiting(), "a spinner while it is out");
    let found = luxe_core::github::ListedSection {
        name: section.name.clone(),
        items: vec![luxe_core::github::ListItem {
            pull_request: true,
            number: 5,
            title: "Closed".into(),
            url: "https://github.com/acme/app/pull/5".into(),
            repository: "acme/app".into(),
            author: "mara".into(),
            draft: false,
            updated_at: String::new(),
            state: luxe_core::github::PullRequestState::Closed,
            facts: None,
        }],
        error: None,
        total: Some(40),
        next: Some("c".into()),
    };
    app.update(Msg::Engine(luxe_core::event::Event::GithubSearched {
        request: request + 100,
        repository: None,
        section: found.clone(),
    }));
    assert!(app.overlays.github.as_ref().unwrap().rows().is_empty(), "not this search's answer");
    app.update(Msg::Engine(luxe_core::event::Event::GithubSearched {
        request,
        repository: None,
        section: found,
    }));
    let panel = app.overlays.github.as_ref().unwrap();
    assert_eq!(panel.rows().iter().map(|row| row.number).collect::<Vec<_>>(), [5]);
    assert!(!panel.searching(), "answered; what waits now is the row's own overview");

    key(&mut app, KeyCode::Esc);
    let sections = open(&mut app, &mut rx);
    assert!(sections[3].query.starts_with("is:closed"), "reopened on the reader's search");
    assert!(sections[4].query.starts_with("is:open"), "the others as configured");
    let panel = app.overlays.github.as_mut().unwrap();
    panel.section = 3;
    assert!(panel.hint().contains("x reset"));
    key(&mut app, KeyCode::Char('x'));
    let Ok(Action::SearchGithub { section, .. }) = rx.try_recv() else {
        panic!("expected the configured search back");
    };
    assert_eq!(section.query, "is:open is:pr sort:updated-desc repo:{repo}");
    assert!(app.github_searches.is_empty(), "nothing left to remember");
}
