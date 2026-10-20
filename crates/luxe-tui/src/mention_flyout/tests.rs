use super::*;
use crossterm::event::KeyModifiers;

#[test]
fn mention_flyout_shows_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::File::create(dir.path().join("main.rs")).unwrap();
    std::fs::File::create(dir.path().join("lib.rs")).unwrap();

    let provider = FileRefProvider::new(dir.path());
    let state = MentionFlyoutState::new(provider, dir.path().to_path_buf(), &[], Vec::new(), &[]);

    // Should show both files.
    assert_eq!(state.filtered_items().len(), 2);
}

#[test]
fn typing_filters_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::File::create(dir.path().join("main.rs")).unwrap();
    std::fs::File::create(dir.path().join("lib.rs")).unwrap();
    std::fs::File::create(dir.path().join("test.rs")).unwrap();

    let provider = FileRefProvider::new(dir.path());
    let mut state =
        MentionFlyoutState::new(provider, dir.path().to_path_buf(), &[], Vec::new(), &[]);

    let initial_count = state.filtered_items().len();

    state.set_query("m");
    assert!(
        state.filtered_items().len() <= initial_count,
        "filter should narrow or maintain count"
    );
}

#[test]
fn enter_inserts_selected() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::File::create(dir.path().join("main.rs")).unwrap();

    let provider = FileRefProvider::new(dir.path());
    let mut state =
        MentionFlyoutState::new(provider, dir.path().to_path_buf(), &[], Vec::new(), &[]);

    // Select the first item.
    let outcome = state.on_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(matches!(outcome, MentionFlyoutOutcome::Insert(_)));
}

#[test]
fn esc_closes() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::File::create(dir.path().join("main.rs")).unwrap();

    let provider = FileRefProvider::new(dir.path());
    let mut state =
        MentionFlyoutState::new(provider, dir.path().to_path_buf(), &[], Vec::new(), &[]);

    let outcome = state.on_key(&KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(matches!(outcome, MentionFlyoutOutcome::Close));
}

#[test]
fn text_keys_are_delegated_to_the_composer() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::File::create(dir.path().join("main.rs")).unwrap();

    let provider = FileRefProvider::new(dir.path());
    let mut state =
        MentionFlyoutState::new(provider, dir.path().to_path_buf(), &[], Vec::new(), &[]);

    assert!(matches!(
        state.on_key(&KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE)),
        MentionFlyoutOutcome::Edit
    ));
    assert!(matches!(
        state.on_key(&KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)),
        MentionFlyoutOutcome::Edit
    ));
}

#[test]
fn arrows_move_one_row_in_each_direction() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::File::create(dir.path().join("a.rs")).unwrap();
    std::fs::File::create(dir.path().join("b.rs")).unwrap();

    let provider = FileRefProvider::new(dir.path());
    let mut state =
        MentionFlyoutState::new(provider, dir.path().to_path_buf(), &[], Vec::new(), &[]);
    state.on_key(&KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(state.nav.selected(), 1);
    state.on_key(&KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(state.nav.selected(), 0);
}

/// The GitHub tab asks for its rows the first time it opens and keeps them after; a listing
/// that failed is asked for again on the next visit. Skills, on this machine, never ask.
#[test]
fn github_rows_are_asked_for_once_and_again_after_a_failure() {
    let dir = tempfile::tempdir().unwrap();
    let provider = FileRefProvider::new(dir.path());
    let mut state =
        MentionFlyoutState::new(provider, dir.path().to_path_buf(), &[], Vec::new(), &[]);
    let press = |state: &mut MentionFlyoutState, code| {
        state.on_key(&KeyEvent::new(code, KeyModifiers::NONE))
    };
    assert_eq!(press(&mut state, KeyCode::Tab), MentionFlyoutOutcome::Consumed, "skills");
    assert_eq!(*state.tab(), MentionTab::Skills);
    assert!(!state.fetching());
    assert_eq!(press(&mut state, KeyCode::Tab), MentionFlyoutOutcome::Fetch(MentionTab::GitHub));
    state.set_github(Vec::new(), Some("Not signed in.".into()));
    assert!(!state.fetching());
    assert_eq!(press(&mut state, KeyCode::Tab), MentionFlyoutOutcome::Consumed, "back on files");
    assert_eq!(*state.tab(), MentionTab::Files, "wrapping round");
    assert_eq!(
        press(&mut state, KeyCode::BackTab),
        MentionFlyoutOutcome::Fetch(MentionTab::GitHub),
        "asked again"
    );
    state.set_github(Vec::new(), None);
    assert_eq!(press(&mut state, KeyCode::Tab), MentionFlyoutOutcome::Consumed);
    assert_eq!(press(&mut state, KeyCode::BackTab), MentionFlyoutOutcome::Consumed, "kept");
}

/// The skills installed are listed at once, filter by name and description, and insert by
/// name.
#[test]
fn skills_are_listed_at_once_and_insert_by_name() {
    let dir = tempfile::tempdir().unwrap();
    let provider = FileRefProvider::new(dir.path());
    let skill = |name: &str, description: &str| luxe_core::skills::SkillDoc {
        name: name.into(),
        description: description.into(),
        path: dir.path().join(name).join("SKILL.md"),
    };
    let skills =
        vec![skill("gh-stack", "Stacked pull requests"), skill("tidy", "Keep diffs small")];
    let mut state = MentionFlyoutState::new(provider, dir.path().to_path_buf(), &[], skills, &[]);
    state.on_key(&KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(state.filtered_items().len(), 2);
    state.set_query("small");
    assert_eq!(state.selected_candidate().unwrap().label, "tidy");
    assert_eq!(
        state.on_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        MentionFlyoutOutcome::InsertSkill("tidy".into())
    );
}

/// The skills tab says where skills live when there are none, says nothing matches when a
/// query leaves none, and lists a skill with no description by name alone.
#[test]
fn the_skills_tab_says_what_its_empty_means() {
    use ratatui::{backend::TestBackend, Terminal};
    let resolved = luxe_core::theme::ThemeRegistry::new().resolve("nord").expect("nord");
    let theme = crate::theme::styles::ThemeStyles::from_resolved(&resolved, "nord");
    let drawn = |state: &MentionFlyoutState| {
        let mut terminal = Terminal::new(TestBackend::new(140, 12)).expect("terminal");
        let hit = std::cell::RefCell::new(crate::mouse::HitRegions::default());
        terminal
            .draw(|f| {
                render_mention_flyout(
                    f,
                    f.area(),
                    crate::picker::Edge::Above(12),
                    state,
                    &theme,
                    &hit,
                    0,
                );
            })
            .expect("draw");
        let buffer = terminal.backend().buffer().clone();
        (0..12)
            .map(|y| (0..140).map(|x| buffer[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    };
    let dir = tempfile::tempdir().unwrap();
    let open = |skills| {
        let provider = FileRefProvider::new(dir.path());
        let mut state =
            MentionFlyoutState::new(provider, dir.path().to_path_buf(), &[], skills, &[]);
        state.on_key(&KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        state
    };
    let none = drawn(&open(Vec::new()));
    assert!(none.contains("No skills found · add <name>/SKILL.md"), "{none}");
    let bare = luxe_core::skills::SkillDoc {
        name: "tidy".into(),
        description: String::new(),
        path: dir.path().join("SKILL.md"),
    };
    let mut state = open(vec![bare]);
    assert_eq!(state.selected_candidate().unwrap().detail, None, "no description, no detail");
    state.set_query("zzzq");
    let unmatched = drawn(&state);
    assert!(unmatched.contains("No matching skills") && !unmatched.contains("No skills found"));
}

fn server(id: &str, tools: Option<&[&str]>, signed_in: Option<bool>) -> luxe_core::McpServerStatus {
    luxe_core::McpServerStatus {
        id: id.into(),
        transport: "http".into(),
        auth: if signed_in.is_some() { "oauth" } else { "none" }.into(),
        tool_count: tools.map(<[&str]>::len),
        enabled: true,
        tools: tools.unwrap_or_default().iter().map(|name| (*name).to_string()).collect(),
        signed_in,
    }
}

/// The tab exists when the session has a server and not otherwise, sits after GitHub and
/// before the reader's sources, and never asks for its rows — they came with the snapshot.
#[test]
fn the_mcp_tab_is_there_only_with_a_server_and_never_fetches() {
    let dir = tempfile::tempdir().unwrap();
    let sources = [MentionSource {
        name: "Tickets".into(),
        kind: None,
        command: "tickets".into(),
        args: Vec::new(),
        icon: None,
    }];
    let tabs = |servers: &[luxe_core::McpServerStatus]| {
        let provider = FileRefProvider::new(dir.path());
        MentionFlyoutState::new(provider, dir.path().to_path_buf(), &sources, Vec::new(), servers)
            .tabs
            .iter()
            .map(|tab| tab.name().to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(tabs(&[]), ["Files", "Skills", "GitHub", "Tickets"], "no server, no tab");
    assert_eq!(
        tabs(&[server("asana", Some(&["get_tasks"]), Some(true))]),
        ["Files", "Skills", "GitHub", "MCP", "Tickets"]
    );

    let mut disabled = server("off", Some(&[]), None);
    disabled.enabled = false;
    let unsafe_id = server("two words", Some(&[]), None);
    assert!(!tabs(&[disabled, unsafe_id]).contains(&"MCP".to_string()), "nothing listable");

    let provider = FileRefProvider::new(dir.path());
    let mut state = MentionFlyoutState::new(
        provider,
        dir.path().to_path_buf(),
        &[],
        Vec::new(),
        &[server("asana", Some(&[]), None)],
    );
    state.on_key(&KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE));
    assert_eq!(*state.tab(), MentionTab::Mcp);
    assert!(!state.fetching(), "listed at once, from the snapshot");
}

/// A row is the server's id; its detail follows the sign-in first, then discovery; a query
/// matches a tool's name; Enter inserts the id; an unsafe id is not a row.
#[test]
fn a_server_row_says_what_stands_between_the_reader_and_its_tools() {
    let dir = tempfile::tempdir().unwrap();
    let servers = [
        server("asana", Some(&["get_tasks", "create_task"]), Some(true)),
        server("linear", Some(&["list_issues"]), Some(false)),
        server("docs", None, None),
        server("one", Some(&["only"]), None),
        server("evil\u{1b}[2J", Some(&[]), None),
    ];
    let provider = FileRefProvider::new(dir.path());
    let mut state =
        MentionFlyoutState::new(provider, dir.path().to_path_buf(), &[], Vec::new(), &servers);
    state.on_key(&KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE));
    let rows: Vec<(String, Option<String>)> = state
        .filtered_items()
        .iter()
        .map(|&index| (state.items[index].label.clone(), state.items[index].detail.clone()))
        .collect();
    let row = |label: &str, detail: &str| (label.to_string(), Some(detail.to_string()));
    assert_eq!(
        rows,
        vec![
            row("asana", "2 tools · signed in"),
            row("linear", "not signed in — luxe mcp login --id linear"),
            row("docs", "tools not discovered yet"),
            row("one", "1 tool"),
        ]
    );
    state.set_query("issues");
    assert_eq!(state.selected_candidate().unwrap().label, "linear", "found by a tool's name");
    assert_eq!(
        state.on_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        MentionFlyoutOutcome::InsertMcp("linear".into())
    );
}

/// Draws `state` into a terminal of `width`×24 with the composer at the bottom, as text rows.
fn drawn_rows(state: &MentionFlyoutState, width: u16) -> Vec<String> {
    use ratatui::{backend::TestBackend, Terminal};
    let resolved = luxe_core::theme::ThemeRegistry::new().resolve("nord").expect("nord");
    let theme = crate::theme::styles::ThemeStyles::from_resolved(&resolved, "nord");
    let mut terminal = Terminal::new(TestBackend::new(width, 24)).expect("terminal");
    let hit = std::cell::RefCell::new(crate::mouse::HitRegions::default());
    terminal
        .draw(|f| {
            render_mention_flyout(
                f,
                f.area(),
                crate::picker::Edge::Above(24),
                state,
                &theme,
                &hit,
                0,
            )
        })
        .expect("draw");
    let buffer = terminal.backend().buffer().clone();
    (0..24).map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect()).collect()
}

/// The frame's top edge, which is the height, since it grows up from the composer.
fn top_edge(rows: &[String]) -> usize {
    rows.iter().position(|row| row.contains("Mention")).expect("the frame is drawn")
}

/// Every tab draws the same height, whether it lists one row, several or none, so moving
/// between them never moves the frame.
#[test]
fn every_tab_keeps_the_same_height() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::File::create(dir.path().join("only.rs")).unwrap();
    let servers = [
        server("asana", Some(&["get_tasks"]), Some(true)),
        server("linear", Some(&[]), None),
        server("docs", None, None),
    ];
    let provider = FileRefProvider::new(dir.path());
    let mut state =
        MentionFlyoutState::new(provider, dir.path().to_path_buf(), &[], Vec::new(), &servers);
    let files = top_edge(&drawn_rows(&state, 100));
    let mut heights = vec![files];
    for _ in 0..3 {
        state.on_key(&KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        heights.push(top_edge(&drawn_rows(&state, 100)));
    }
    assert!(heights.iter().all(|&h| h == files), "skills, GitHub, MCP: {heights:?}");
    // Nine rows, the frame, the tab header and its rule, the footer and its rule
    assert_eq!(
        24 - files,
        crate::picker::TABBED_ROWS + 2 + 2 + usize::from(crate::widgets::FOOTER_ROWS),
        "{heights:?}"
    );
}

/// With nothing to list, what the tab says sits on the middle row of the list and in the
/// middle of it across; a message too wide for one row wraps, still centered.
#[test]
fn an_empty_tab_says_so_in_the_middle() {
    let dir = tempfile::tempdir().unwrap();
    let provider = FileRefProvider::new(dir.path());
    let mut state =
        MentionFlyoutState::new(provider, dir.path().to_path_buf(), &[], Vec::new(), &[]);
    // GitHub, before its rows arrive
    state.on_key(&KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    state.on_key(&KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    let rows = drawn_rows(&state, 100);
    let first_list_row = top_edge(&rows) + 3;
    let middle = first_list_row + crate::picker::TABBED_ROWS / 2;
    let at = rows.iter().position(|row| row.contains("Fetching data from GitHub")).unwrap();
    assert_eq!(at, middle, "{rows:#?}");
    let line: Vec<char> = rows[at].chars().collect();
    let text = |c: &char| !matches!(c, ' ' | '│');
    let left = line.iter().position(text).unwrap();
    let right = line.len() - 1 - line.iter().rposition(text).unwrap();
    assert!(left.abs_diff(right) <= 1, "centered across: {left} vs {right}\n{}", rows[at]);

    // Skills, empty: the long hint on where skills live, wrapped around the middle
    state.on_key(&KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE));
    let rows = drawn_rows(&state, 60);
    let first_list_row = top_edge(&rows) + 3;
    let said: Vec<usize> = (first_list_row..first_list_row + crate::picker::TABBED_ROWS)
        .filter(|&y| rows[y].chars().any(|c| c.is_alphanumeric()))
        .collect();
    assert!(said.len() > 1, "wrapped: {rows:#?}");
    let (above, below) = (
        said[0] - first_list_row,
        first_list_row + crate::picker::TABBED_ROWS - 1 - said[said.len() - 1],
    );
    assert!(above.abs_diff(below) <= 1, "centered down: {above} above, {below} below");
    assert!(rows.iter().any(|row| row.contains("No skills found")), "{rows:#?}");
}

/// A match lights where it landed: a skill's name or its description, a GitHub row's title or
/// the repository its detail ends with, and a server's id — or, when one of its tools matched,
/// that tool, named in the detail, since the id it lists by is not what matched.
#[test]
fn every_tab_lights_what_its_query_matched() {
    let lit = |candidate: &Candidate| {
        let pick = |text: &str, at: &[usize]| {
            text.chars()
                .enumerate()
                .filter(|(i, _)| at.contains(i))
                .map(|(_, c)| c)
                .collect::<String>()
        };
        (
            pick(&candidate.label, &candidate.label_matched),
            pick(candidate.detail.as_deref().unwrap_or(""), &candidate.detail_matched),
        )
    };
    let first = |entry: Entry, query: &str| {
        let words = entry_words(&entry);
        let found = crate::fuzzy::rank(query, &[words.as_str()]);
        entry_candidate(&entry, &found[0].positions)
    };

    let skill = Entry::Skill(luxe_core::skills::SkillDoc {
        name: "tidy".into(),
        description: "Keep diffs small".into(),
        path: "SKILL.md".into(),
    });
    assert_eq!(lit(&first(skill.clone(), "tidy")), ("tidy".into(), String::new()));
    assert_eq!(lit(&first(skill.clone(), "small")), (String::new(), "small".into()));
    let across = first(skill.clone(), "tidy keep");
    assert_eq!(across.label_matched, [0, 1, 2, 3], "the space after a label is not the label's");
    assert_eq!(first(skill, "tidy").insert, "tidy");

    let item = luxe_core::github::ListItem {
        pull_request: true,
        number: 177,
        title: "Route by category".into(),
        url: "https://github.com/acme/app/pull/177".into(),
        repository: "acme/app".into(),
        author: "mara".into(),
        draft: false,
        updated_at: String::new(),
        state: luxe_core::github::PullRequestState::Open,
        facts: None,
    };
    let github = first(Entry::GitHub(item.clone()), "acme/app");
    assert_eq!(lit(&github), (String::new(), "acme/app".into()), "{:?}", github.detail);
    let by_title = first(Entry::GitHub(item), "route");
    assert_eq!(lit(&by_title), ("Route".into(), String::new()));
    assert_eq!(by_title.insert, "@[github:acme/app/pull/177]");

    let row = Entry::Row(MentionRow {
        id: "PROJ-12".into(),
        label: "PROJ-12 Login fails".into(),
        detail: Some("In progress".into()),
        text: "Jira issue PROJ-12".into(),
    });
    let by_detail = first(row.clone(), "progress");
    assert_eq!(lit(&by_detail), (String::new(), "progress".into()));
    assert_eq!(by_detail.detail.as_deref(), Some("In progress"));
    assert_eq!(lit(&first(row.clone(), "login")), ("Login".into(), String::new()));
    assert_eq!(first(row, "login").insert, "PROJ-12");

    let asana = server("asana", Some(&["get_tasks", "create_task"]), Some(true));
    let by_tool = first(Entry::Mcp(asana.clone()), "create");
    assert_eq!(by_tool.detail.as_deref(), Some("create_task · 2 tools · signed in"));
    assert_eq!(lit(&by_tool), (String::new(), "create".into()));
    let by_first_tool = first(Entry::Mcp(asana.clone()), "get_t");
    assert_eq!(by_first_tool.detail.as_deref(), Some("get_tasks · 2 tools · signed in"));
    assert_eq!(lit(&by_first_tool), (String::new(), "get_t".into()));
    // A query across two tools names the first and lights only its letters
    let spanning = first(Entry::Mcp(asana.clone()), "tasks create");
    assert_eq!(spanning.detail.as_deref(), Some("get_tasks · 2 tools · signed in"));
    assert_eq!(spanning.detail_matched, [4, 5, 6, 7, 8]);
    let by_id = first(Entry::Mcp(asana), "asana");
    assert_eq!(by_id.detail.as_deref(), Some("2 tools · signed in"), "nothing to name");
    assert_eq!(lit(&by_id), ("asana".into(), String::new()));
    assert_eq!(by_id.insert, "asana");
}
