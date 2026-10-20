//! Mentions — the @ picker, its tabs and what a sent reference carries.

use super::*;

/// A mentioned file goes to the model as its path, for the agent to read when the work needs it.
#[tokio::test]
async fn a_mention_is_sent_as_a_reference_not_the_file() {
    let dir = tempfile::tempdir().expect("temp dir");
    let file_path = dir.path().join("test_file.txt");
    std::fs::write(&file_path, "This is test file content.\nLine 2.").expect("fixture");

    let (tx, mut rx) = mpsc::channel(8);
    let keymap = luxe_core::keymap::KeyMap::defaults();
    let config = luxe_core::config::Config::default();
    let mut app = App::new(tx, keymap, config);
    app.status.model = "test-model".into();
    let path = file_path.display().to_string();
    app.editor.insert_text("Look at ");
    app.editor.insert_reference(&format!("@[file:{path}]"));
    app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));

    match rx.try_recv() {
        Ok(Action::SubmitUserMessage { content, .. }) => {
            assert_eq!(content, format!("Look at `{path}`"), "the path, for the agent to read");
            assert!(!content.contains("This is test file content."), "nothing read in");
        }
        other => panic!("expected SubmitUserMessage, got {other:?}"),
    }
    // The transcript shows it as a chip, not as an attachment
    let shown = app.convo.iter().rev().find_map(|item| match item.block() {
        ConvoBlock::User { text, attached, .. } => Some((text.clone(), attached.clone())),
        _ => None,
    });
    let (text, attached) = shown.expect("the message");
    assert!(text.ends_with(&path) && !text.contains("@[file:"), "{text}");
    assert!(attached.is_empty(), "{attached:?}");
}

/// Every reference in a sent message is remembered as a chip, at the columns the chip landed on
/// once all of them were folded in and the message trimmed, so the card can draw each one.
#[tokio::test]
async fn a_sent_message_knows_where_its_chips_are() {
    let dir = tempfile::tempdir().expect("temp dir");
    let (short, long) = (dir.path().join("a.rs"), dir.path().join("a_much_longer_name.rs"));
    std::fs::write(&short, "").expect("fixture");
    std::fs::write(&long, "").expect("fixture");

    let (tx, _rx) = mpsc::channel(8);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    app.status.model = "test-model".into();
    app.editor.insert_text("  fix ");
    app.editor.insert_reference(&format!("@[file:{}]", long.display()));
    app.editor.insert_text("with ");
    app.editor.insert_reference(&format!("@[file:{}]", short.display()));
    app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));

    let (text, chips) = app
        .convo
        .iter()
        .rev()
        .find_map(|item| match item.block() {
            ConvoBlock::User { text, chips, .. } => Some((text.clone(), chips.clone())),
            _ => None,
        })
        .expect("the message");
    assert!(text.starts_with("fix "), "trimmed: {text:?}");
    assert_eq!(chips.len(), 2, "{chips:?} in {text:?}");
    assert!(text[chips[0].clone()].ends_with(&long.display().to_string()), "{text:?} {chips:?}");
    assert!(text[chips[1].clone()].ends_with(&short.display().to_string()), "{text:?} {chips:?}");
    assert!(text[chips[0].end..chips[1].start].contains("with"), "{text:?} {chips:?}");
}

/// The MCP tab lists the servers the snapshot names; picking one inserts a chip; the first send
/// carries the server's tool names and the second only points; compaction and a new session each
/// make the next send carry them again.
#[tokio::test]
async fn the_mention_picker_references_an_mcp_server_and_lists_it_once() {
    use luxe_core::workspace::{ProjectIdentity, WorkspaceScopeId};

    let (tx, mut rx) = mpsc::channel(8);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    app.status.model = "test-model".into();
    let mut snapshot = WorkspaceSnapshot::empty(
        WorkspaceScopeId::new("scope-a"),
        ProjectIdentity::non_git("/project".into(), "project", "/project"),
    );
    snapshot.tooling.mcp_servers = vec![luxe_core::McpServerStatus {
        id: "asana".into(),
        transport: "http".into(),
        auth: "oauth".into(),
        tool_count: Some(1),
        enabled: true,
        tools: vec!["get_tasks".into()],
        signed_in: Some(true),
    }];
    app.update(Msg::Engine(Event::WorkspaceUpdated(Box::new(snapshot))));

    let key = |code| Msg::Key(KeyEvent::new(code, KeyModifiers::NONE));
    app.update(key(KeyCode::Char('@')));
    app.update(Msg::Key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT)));
    assert_eq!(
        app.overlays.mention.as_ref().map(|state| state.tab().clone()),
        Some(crate::mention_flyout::MentionTab::Mcp)
    );
    app.update(key(KeyCode::Enter));
    assert_eq!(app.editor.current_input(), "@[mcp:asana] ");

    // Each send after the first types the reference itself; the first is the chip just picked
    let mut typed = false;
    let mut sent = |app: &mut App| {
        if std::mem::replace(&mut typed, true) {
            app.editor.insert_reference("@[mcp:asana]");
        }
        app.update(key(KeyCode::Enter));
        app.status.state = AppState::Idle;
        loop {
            match rx.try_recv() {
                // the FILES tree asks for the project root once a workspace is known
                Ok(Action::ListDirectory { .. }) => continue,
                Ok(Action::SubmitUserMessage { content, .. }) => break content,
                other => panic!("expected SubmitUserMessage, got {other:?}"),
            }
        }
    };
    assert!(sent(&mut app).contains(": get_tasks ("), "the first send lists the tools");
    assert!(!sent(&mut app).contains("get_tasks"), "the second only points");
    app.update(Msg::Engine(Event::CompactionDone { tokens_before: 90_000, tokens_after: 9_000 }));
    assert!(sent(&mut app).contains(": get_tasks ("), "after compaction, listed again");
    app.update(Msg::Engine(Event::SessionLoaded { session_id: "next".into(), messages: vec![] }));
    assert!(sent(&mut app).contains(": get_tasks ("), "in a new session, listed again");
}

/// @ at start of input opens mention picker.
#[test]
fn test_mention_picker_opens_at_start() {
    let (tx, _rx) = mpsc::channel(8);
    let keymap = luxe_core::keymap::KeyMap::defaults();
    let config = luxe_core::config::Config::default();
    let mut app = App::new(tx, keymap, config);

    // Type @ at start.
    app.update(Msg::Key(KeyEvent::new(KeyCode::Char('@'), KeyModifiers::NONE)));

    // Mention picker should be open.
    assert!(app.overlays.mention.is_some(), "mention picker should open at start");
}

/// The Files tab keeps up with the disk: opening the picker asks for a fresh read, the read lands
/// in the open picker with the cursor still on its file, and Ctrl+R asks again. A file made or
/// moved outside luxe used to be unreachable until the session was reloaded.
#[test]
fn the_files_tab_reads_the_disk_again_and_keeps_its_place() {
    let (tx, _rx) = mpsc::channel(8);
    let mut app = App::new_fixture(
        tx,
        luxe_core::keymap::KeyMap::defaults(),
        luxe_core::config::Config::default(),
    );
    app.file_provider.set_listing(vec!["alpha.rs".into(), "beta.rs".into()]);
    app.update(Msg::Key(KeyEvent::new(KeyCode::Char('@'), KeyModifiers::NONE)));
    assert!(app.file_listing.requested, "opening the picker asks for a fresh read");
    assert!(app.file_listing.start(std::time::Instant::now()), "which the loop starts");
    assert!(!app.file_listing.start(std::time::Instant::now()), "once");

    // The cursor is on beta.rs when the fresh read lands with a new file ahead of it
    app.update(Msg::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)));
    let selected = |app: &App| {
        let mention = app.overlays.mention.as_ref()?;
        mention.selected_candidate().map(|candidate| candidate.label.clone())
    };
    assert_eq!(selected(&app).as_deref(), Some("beta.rs"));
    let fresh = vec!["alpha.rs".into(), "aardvark.rs".into(), "beta.rs".into()];
    app.update(Msg::FilesListed(Some(fresh)));
    assert!(!app.file_listing.in_flight, "the read is over");
    let listed = app.overlays.mention.as_ref().map(|mention| mention.filtered_items().len());
    assert_eq!(listed, Some(3), "the new file is listed");
    assert_eq!(selected(&app).as_deref(), Some("beta.rs"), "and the cursor held its file");

    app.update(Msg::Key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL)));
    assert!(app.file_listing.requested, "ctrl+r asks again");
    assert!(app.overlays.mention.is_some(), "without closing the picker");
    assert_eq!(app.editor.current_input(), "@", "or typing into the composer");

    // A failed read changes nothing but lets the next one start
    assert!(app.file_listing.start(std::time::Instant::now()));
    app.update(Msg::FilesListed(None));
    assert!(!app.file_listing.in_flight);
    assert_eq!(app.overlays.mention.as_ref().map(|m| m.filtered_items().len()), Some(3));
}

/// Left open, the picker reads again every few seconds by itself, and never two reads at once.
#[test]
fn an_open_files_tab_is_due_a_read_every_few_seconds() {
    let mut listing = crate::app::FileListing::default();
    let start = std::time::Instant::now();
    assert!(listing.due(start), "never read: due");
    listing.request();
    assert!(!listing.due(start), "already asked");
    assert!(listing.start(start));
    assert!(!listing.due(start + crate::app::FileListing::EVERY), "one at a time");
    listing.in_flight = false;
    assert!(!listing.due(start + std::time::Duration::from_secs(1)), "too soon");
    assert!(listing.due(start + crate::app::FileListing::EVERY));
    // A request made while a read runs waits for it rather than being lost
    listing.in_flight = true;
    listing.request();
    assert!(!listing.start(start), "not while one runs");
    listing.in_flight = false;
    assert!(listing.start(start), "then it goes");
}

/// Tab in the `@` picker opens GitHub, asking for the rows the first time only; picking one
/// inserts a chip, and sending says what the row is. A row in two sections is listed once.
#[test]
fn the_mention_picker_references_github_rows() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut app = App::new(tx, luxe_core::keymap::KeyMap::defaults(), Default::default());
    app.status.model = "test-model".into();
    let key = |code| Msg::Key(KeyEvent::new(code, KeyModifiers::NONE));
    let row = |number: u64, title: &str| luxe_core::github::ListItem {
        pull_request: number != 9,
        number,
        title: title.into(),
        url: format!("https://github.com/acme/app/pull/{number}"),
        repository: "acme/app".into(),
        author: "mara".into(),
        draft: false,
        updated_at: String::new(),
        state: luxe_core::github::PullRequestState::Open,
        facts: None,
    };
    app.update(key(KeyCode::Char('@')));
    assert!(!app.overlays.mention.as_ref().unwrap().fetching(), "files need no fetching");
    assert!(!app.animating());
    app.update(key(KeyCode::Tab));
    app.update(key(KeyCode::Tab));
    assert!(matches!(rx.try_recv(), Ok(Action::ListGithub { .. })), "asked for the rows");
    assert!(app.overlays.mention.as_ref().unwrap().fetching());
    assert!(app.animating(), "its spinner turns");
    let drawn = |app: &mut App| {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| crate::view::render(frame, app)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..30)
            .map(|y| (0..100).map(|x| buffer[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    };
    let screen = drawn(&mut app);
    assert!(screen.contains("Fetching data from GitHub…") && screen.contains("tab switch"));
    assert!(screen.contains("Mention") && screen.contains("Files") && screen.contains("GitHub"));
    let section = |name: &str, items| luxe_core::github::ListedSection {
        name: name.into(),
        items,
        error: None,
        total: None,
        next: None,
    };
    app.update(Msg::Engine(luxe_core::event::Event::GithubListed {
        repository: Some("acme/app".into()),
        sections: vec![
            section("Mine", vec![row(12, "Parse categories"), row(13, "Route by category")]),
            section("Review requested", vec![row(12, "Parse categories")]),
            section("Assigned issues", vec![row(9, "Total skips a number")]),
        ],
    }));
    let mention = app.overlays.mention.as_ref().unwrap();
    assert_eq!(mention.filtered_items().len(), 3, "#12 once");
    assert!(!mention.fetching() && !app.animating(), "answered");
    for character in "zzzq".chars() {
        app.update(key(KeyCode::Char(character)));
    }
    let screen = drawn(&mut app);
    assert!(screen.contains("No matching pull requests or issues"), "{screen}");
    assert!(!screen.contains("Fetching"));
    for _ in 0..4 {
        app.update(key(KeyCode::Backspace));
    }
    for character in "Route".chars() {
        app.update(key(KeyCode::Char(character)));
    }
    let picked = app.overlays.mention.as_ref().unwrap().selected_candidate().unwrap().label.clone();
    assert_eq!(picked, "#13 Route by category");
    app.update(key(KeyCode::Enter));
    assert!(app.overlays.mention.is_none());
    assert_eq!(app.editor.current_input(), "@[github:acme/app/pull/13] ");

    // Picked before a space already there, it adds none of its own
    app.editor.handle_key(KeyEvent::new(KeyCode::Home, KeyModifiers::CONTROL));
    app.editor.insert_text("@Par ");
    app.editor.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    app.update(key(KeyCode::Char('s')));
    app.overlays.mention.as_mut().unwrap().set_github(vec![row(12, "Parse categories")], None);
    app.update(key(KeyCode::Tab));
    app.update(key(KeyCode::Tab));
    app.update(key(KeyCode::Enter));
    assert_eq!(
        app.editor.current_input(),
        "@[github:acme/app/pull/12] @[github:acme/app/pull/13] "
    );
    app.editor.cancel();
    app.editor.insert_reference("@[github:acme/app/pull/13]");
    app.editor.insert_text(" ");

    // Another `@` finds the rows kept, and asks GitHub nothing
    app.update(key(KeyCode::Char('@')));
    app.update(key(KeyCode::BackTab));
    assert!(rx.try_recv().is_err());
    assert_eq!(app.overlays.mention.as_ref().unwrap().filtered_items().len(), 3);
    app.update(key(KeyCode::Esc));
    app.editor.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));

    app.update(key(KeyCode::Enter));
    match rx.try_recv() {
        Ok(Action::SubmitUserMessage { content, .. }) => assert_eq!(
            content.trim(),
            "pull request acme/app#13 (\"Route by category\", open, \
             https://github.com/acme/app/pull/13; the git tool reads it: action \"pr\", args \
             [\"view\", \"13\", \"--repo\", \"acme/app\", \"--comments\"])"
        ),
        other => panic!("expected SubmitUserMessage, got {other:?}"),
    }
}

/// A source from `mentions` is a tab after GitHub. Shift+Tab reaches it from files; opening it
/// asks the main loop to run it, a failure says why and is asked again, rows insert a chip, and
/// sending says what the row is.
#[test]
fn a_configured_source_is_a_tab_of_its_own() {
    let (tx, mut rx) = mpsc::channel(8);
    let config = luxe_core::config::Config {
        mentions: vec![luxe_core::config::MentionSource {
            name: "Jira".into(),
            kind: None,
            command: "jira-mentions".into(),
            args: Vec::new(),
            icon: Some("\u{f0303}".into()),
        }],
        ..Default::default()
    };
    let mut app = App::new(tx, luxe_core::keymap::KeyMap::defaults(), config);
    app.status.model = "test-model".into();
    let key = |code| Msg::Key(KeyEvent::new(code, KeyModifiers::NONE));
    let row = |id: &str, label: &str| luxe_core::mention_source::MentionRow {
        id: id.into(),
        label: label.into(),
        detail: Some("In progress".into()),
        text: format!("Jira issue {id} \"{label}\""),
    };
    let listed = |answer| Msg::MentionsListed { kind: "jira".into(), answer };
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
    app.update(key(KeyCode::BackTab));
    let asked = app.outbox.mention_source.take().expect("the source is run");
    assert_eq!(asked.name, "Jira");
    assert!(rx.try_recv().is_err(), "GitHub is not asked for a tab it is not");
    let screen = drawn(&mut app);
    assert!(screen.contains("Fetching from Jira…"), "{screen}");
    let tab = crate::glyphs::icon_label(
        crate::glyphs::configured_glyph(Some("\u{f0303}"), crate::glyphs::Icon::MentionSource),
        "Jira",
    );
    assert!(screen.contains(&tab), "the tab wears the icon it was given: {screen}");

    app.update(listed(Err("Not signed in: run jira login.".into())));
    let screen = drawn(&mut app);
    assert!(screen.contains("Not signed in: run jira login."), "{screen}");
    assert!(app.mention_listings.is_empty(), "a failure is not kept");
    app.update(key(KeyCode::Tab));
    app.update(key(KeyCode::BackTab));
    assert!(app.outbox.mention_source.take().is_some(), "asked again");

    app.update(listed(Ok(vec![row("PROJ-12", "Login fails"), row("PROJ-9", "Export is slow")])));
    for character in "slow".chars() {
        app.update(key(KeyCode::Char(character)));
    }
    let picked = app.overlays.mention.as_ref().unwrap().selected_candidate().unwrap().label.clone();
    assert_eq!(picked, "Export is slow");
    app.update(key(KeyCode::Enter));
    assert_eq!(app.editor.current_input(), "@[jira:PROJ-9] ");

    // Kept: the next look at the tab runs nothing
    app.update(key(KeyCode::Char('@')));
    app.update(key(KeyCode::BackTab));
    assert!(app.outbox.mention_source.is_none());
    assert_eq!(app.overlays.mention.as_ref().unwrap().filtered_items().len(), 2);
    app.update(key(KeyCode::Esc));
    app.editor.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));

    app.update(key(KeyCode::Enter));
    match rx.try_recv() {
        Ok(Action::SubmitUserMessage { content, .. }) => {
            assert_eq!(content.trim(), "Jira issue PROJ-9 \"Export is slow\"");
        }
        other => panic!("expected SubmitUserMessage, got {other:?}"),
    }
    let shown = app.convo.iter().rev().find_map(|item| match item.block() {
        ConvoBlock::User { text, .. } => Some(text.clone()),
        _ => None,
    });
    let chip = crate::glyphs::icon_label(
        crate::glyphs::configured_glyph(Some("\u{f0303}"), crate::glyphs::Icon::MentionSource),
        "Jira PROJ-9",
    );
    assert!(shown.expect("the message").ends_with(&chip), "and so does its chip");
}

/// A skill picked from its tab goes into the composer as one chip.
#[test]
fn a_picked_skill_is_a_chip() {
    let (tx, _rx) = mpsc::channel(8);
    let mut app = App::new(tx, luxe_core::keymap::KeyMap::defaults(), Default::default());
    let key = |code| Msg::Key(KeyEvent::new(code, KeyModifiers::NONE));
    app.editor.insert_text("Split this with ");
    app.update(key(KeyCode::Char('@')));
    let dir = tempfile::tempdir().unwrap();
    let skill = luxe_core::skills::SkillDoc {
        name: "gh-stack".into(),
        description: "Stacked pull requests".into(),
        path: dir.path().join("SKILL.md"),
    };
    app.overlays.mention = Some(crate::mention_flyout::MentionFlyoutState::new(
        crate::ref_provider::FileRefProvider::new(dir.path()),
        dir.path().to_path_buf(),
        &[],
        vec![skill],
        &[],
    ));
    app.update(key(KeyCode::Tab));
    app.update(key(KeyCode::Enter));
    assert!(app.overlays.mention.is_none());
    assert_eq!(app.editor.current_input(), "Split this with @[skill:gh-stack] ");
    app.editor.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
    app.editor.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
    assert_eq!(app.editor.current_input(), "Split this with ", "one chip, deleted whole");
}

/// @ after whitespace opens mention picker.
#[test]
fn test_mention_picker_opens_after_whitespace() {
    let (tx, _rx) = mpsc::channel(8);
    let keymap = luxe_core::keymap::KeyMap::defaults();
    let config = luxe_core::config::Config::default();
    let mut app = App::new(tx, keymap, config);

    // Type "look at @".
    for c in "look at @".chars() {
        app.update(Msg::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)));
    }

    // Mention picker should be open.
    assert!(app.overlays.mention.is_some(), "mention picker should open after whitespace");
}

/// @ mid-word (e.g., email) does NOT open mention picker.
#[test]
fn test_mention_picker_does_not_open_mid_word() {
    let (tx, _rx) = mpsc::channel(8);
    let keymap = luxe_core::keymap::KeyMap::defaults();
    let config = luxe_core::config::Config::default();
    let mut app = App::new(tx, keymap, config);

    // Type "user@example.com" with cursor after @.
    for c in "user@example.com".chars() {
        app.update(Msg::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)));
    }

    // Mention picker should NOT be open.
    assert!(app.overlays.mention.is_none(), "mention picker should not open mid-word");
}

#[test]
fn mention_query_lives_in_the_editor_and_space_closes_the_picker() {
    let (tx, _rx) = mpsc::channel(8);
    let mut app =
        App::new(tx, luxe_core::keymap::KeyMap::defaults(), luxe_core::config::Config::default());

    for character in "look at @src ".chars() {
        app.update(Msg::Key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE)));
    }

    assert_eq!(app.editor.current_input(), "look at @src ");
    assert!(app.overlays.mention.is_none());
}

#[test]
fn accepting_a_mention_replaces_the_trigger_instead_of_duplicating_it() {
    let (tx, _rx) = mpsc::channel(8);
    let mut app =
        App::new(tx, luxe_core::keymap::KeyMap::defaults(), luxe_core::config::Config::default());

    for character in "look at @".chars() {
        app.update(Msg::Key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE)));
    }
    app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));

    assert!(app.editor.current_input().starts_with("look at @[file:"));
    assert!(!app.editor.current_input().contains("@@"));
    assert!(app.editor.current_input().ends_with(' '));
    assert!(app.overlays.mention.is_none());
}

/// Multi-mention submit: two @-file references in one message (regression for crash).
#[test]
fn test_multi_mention_submit_does_not_panic() {
    let (tx, mut rx) = mpsc::channel(8);
    let keymap = luxe_core::keymap::KeyMap::defaults();
    let config = luxe_core::config::Config::default();
    let mut app = App::new(tx, keymap, config);

    // Type two file references.
    app.status.model = "test-model".into();
    app.editor.insert_text("@[file:a.txt] and @[file:b.txt]");

    app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));

    // Should not panic and should dispatch SubmitUserMessage.
    match rx.try_recv() {
        Ok(Action::SubmitUserMessage { content, .. }) => {
            assert!(content.contains("a.txt"), "should contain first ref target");
            assert!(content.contains("b.txt"), "should contain second ref target");
        }
        other => panic!("expected SubmitUserMessage, got {other:?}"),
    }
}

/// A mention source whose kind is one of luxe's own tabs' — `mcp` above all, reserved after sources
/// existed — says why it has no tab, at startup, instead of quietly not having one.
#[test]
fn a_mention_source_on_a_reserved_kind_says_so_at_startup() {
    let source = |name: &str, kind: Option<&str>| luxe_core::config::MentionSource {
        name: name.into(),
        kind: kind.map(str::to_string),
        command: "list".into(),
        args: Vec::new(),
        icon: None,
    };
    let config = Config {
        mentions: vec![source("MCP", None), source("Jira", None), source("Mine", Some("github"))],
        ..Config::default()
    };
    let (tx, _rx) = mpsc::channel(8);
    let app = App::new(tx, KeyMap::defaults(), config);
    let notices: Vec<&String> =
        app.startup_notices.iter().filter(|notice| notice.contains("mention source")).collect();
    assert_eq!(notices.len(), 2, "{notices:?}");
    assert!(
        notices[0].starts_with("mention source `MCP` is not listed — `@[mcp:…]`"),
        "{notices:?}"
    );
    assert!(notices[0].contains("config.json"), "and where to fix it: {notices:?}");
    assert!(notices[1].contains("`Mine`") && notices[1].contains("`@[github:…]`"), "{notices:?}");
}
