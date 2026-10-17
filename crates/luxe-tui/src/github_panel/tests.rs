//! `/github`'s keys and layout.

use super::*;
use crate::github_detail::Subject;
use crate::theme::styles::ThemeStyles;
use crossterm::event::KeyModifiers;
use ratatui::layout::Rect;

mod collision;

fn item(pull_request: bool, number: u64, title: &str, repository: &str) -> ListItem {
    ListItem {
        pull_request,
        number,
        title: title.into(),
        url: format!("https://github.com/{repository}/pull/{number}"),
        repository: repository.into(),
        author: "mara".into(),
        draft: number == 12,
        updated_at: String::new(),
        state: luxe_core::github::PullRequestState::Open,
        facts: None,
    }
}

/// Two tabs kept to the checkout's repository, and a configured one searching everywhere.
fn searches() -> Vec<GithubSection> {
    let section =
        |name: &str, query: &str| GithubSection { name: name.into(), query: query.into() };
    vec![
        section("Mine", "is:open is:pr author:@me repo:{repo}"),
        section("Assigned issues", "is:open is:issue assignee:@me repo:{repo}"),
        section("Review requested", "is:open is:pr review-requested:@me"),
    ]
}

fn panel() -> GithubPanel {
    let searches = searches();
    GithubPanel {
        repository: Some("acme/app".into()),
        sections: Some(vec![
            ListedSection {
                name: "Mine".into(),
                items: vec![
                    item(true, 12, "Parse categories", "acme/app"),
                    item(true, 13, "Route by category", "upstream/app"),
                ],
                error: None,
                total: None,
                next: None,
            },
            ListedSection {
                name: "Assigned issues".into(),
                items: vec![
                    item(false, 7, "Total skips a number", "acme/app"),
                    item(false, 8, "Elsewhere", "other/lib"),
                ],
                error: None,
                total: None,
                next: None,
            },
            ListedSection {
                name: "Review requested".into(),
                items: Vec::new(),
                error: Some("GitHub refused the search.".into()),
                total: None,
                next: None,
            },
        ]),
        clock: Some(0),
        ..GithubPanel::with_searches(&searches, searches.clone())
    }
}

fn typed(panel: &mut GithubPanel, text: &str) {
    for character in text.chars() {
        assert_eq!(panel.on_key(&key(KeyCode::Char(character))), GithubKey::Held);
    }
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn styles() -> ThemeStyles {
    let registry = luxe_core::theme::ThemeRegistry::new();
    let resolved = registry.resolve("dark").expect("dark theme resolves");
    ThemeStyles::from_resolved(&resolved, "dark")
}

/// Enter reviews a pull request — naming its repository when it is not the checkout's — or
/// starts a session on an issue here, and refuses one elsewhere; `c` refers, `o` opens.
#[test]
fn each_row_leads_to_its_own_action() {
    let mut panel = panel();
    assert_eq!(
        panel.on_key(&key(KeyCode::Enter)),
        GithubKey::Review { number: 12, repository: None }
    );
    let refer = |key: GithubKey| match key {
        GithubKey::Refer(item) => crate::mentions::github_token(&item),
        other => panic!("{other:?}"),
    };
    assert_eq!(refer(panel.on_key(&key(KeyCode::Char('c')))), "@[github:acme/app/pull/12]");
    panel.on_key(&key(KeyCode::Down));
    assert_eq!(
        panel.on_key(&key(KeyCode::Enter)),
        GithubKey::Review { number: 13, repository: Some("upstream/app".into()) }
    );
    assert_eq!(refer(panel.on_key(&key(KeyCode::Char('c')))), "@[github:upstream/app/pull/13]");
    assert_eq!(
        panel.on_key(&key(KeyCode::Char('o'))),
        GithubKey::Open("https://github.com/upstream/app/pull/13".into())
    );
    panel.on_key(&key(KeyCode::Down));
    assert_eq!(panel.selected, 1, "the last row stays the last");

    panel.on_key(&key(KeyCode::Tab));
    assert_eq!((panel.section, panel.selected), (1, 0), "a new section starts at its top");
    assert_eq!(panel.on_key(&key(KeyCode::Enter)), GithubKey::TakeUpIssue(7));
    panel.on_key(&key(KeyCode::Down));
    assert!(
        matches!(panel.on_key(&key(KeyCode::Enter)), GithubKey::Refused(said) if said.contains("other/lib"))
    );
    panel.on_key(&key(KeyCode::Up));
    panel.on_key(&key(KeyCode::Up));
    assert_eq!(panel.selected, 0);
    panel.on_key(&key(KeyCode::Tab));
    panel.on_key(&key(KeyCode::Tab));
    assert_eq!(panel.section, 0, "round to the first");
    panel.on_key(&key(KeyCode::BackTab));
    assert_eq!(panel.section, 2, "and back to the last");
    assert_eq!(panel.on_key(&key(KeyCode::Enter)), GithubKey::Held, "no rows, nothing to do");
    assert_eq!(panel.on_key(&key(KeyCode::Char('r'))), GithubKey::Refresh);
    assert_eq!(panel.on_key(&key(KeyCode::Esc)), GithubKey::Close);
}

/// `/` opens the tab's search to edit, without the scope it never says. Plain words typed after it
/// narrow the loaded rows by title, author or number at once; Esc puts it all back.
#[test]
fn a_search_being_typed_previews_over_the_loaded_rows() {
    let mut panel = panel();
    panel.on_key(&key(KeyCode::Char('/')));
    let editor = panel.editing.as_ref().expect("editing");
    assert_eq!(editor.current_input(), "is:open is:pr author:@me ");
    typed(&mut panel, "route");
    assert_eq!(panel.rows().iter().map(|row| row.number).collect::<Vec<_>>(), [13]);
    typed(&mut panel, " label:bug");
    assert_eq!(panel.rows().len(), 1, "a qualifier waits for GitHub");
    for code in [KeyCode::Up, KeyCode::Down, KeyCode::Tab, KeyCode::BackTab] {
        assert_eq!(panel.on_key(&key(code)), GithubKey::Held, "{code:?}: a fresh line ignores it");
    }
    assert_eq!(panel.section, 0, "Tab is the search's while it is typed, not the tabs'");
    typed(&mut panel, "!");
    assert!(panel.editing.as_ref().unwrap().current_input().ends_with("label:bug!"), "caret kept");
    panel.on_key(&key(KeyCode::Backspace));
    for _ in 0.."route label:bug".len() {
        panel.on_key(&key(KeyCode::Backspace));
    }
    typed(&mut panel, "#12");
    assert_eq!(panel.rows().len(), 1, "by number");
    panel.on_key(&key(KeyCode::Backspace));
    panel.on_key(&key(KeyCode::Backspace));
    panel.on_key(&key(KeyCode::Backspace));
    typed(&mut panel, "MARA");
    assert_eq!(panel.rows().len(), 2, "by author, any case");
    assert_eq!(panel.hint(), "enter search GitHub · esc cancel");
    panel.on_key(&key(KeyCode::Esc));
    assert!(panel.editing.is_none());
    assert_eq!(panel.rows().len(), 2);
    assert!(panel.pending.is_empty(), "Esc asks nothing of GitHub");
}

/// Enter runs the tab again with what was typed, kept to the checkout's repository however hard
/// the reader tries to leave it; the rows go until the answer arrives, and the tab is marked as
/// edited. Enter on what it already searched asks nothing, and an emptied search is the way back.
#[test]
fn enter_runs_the_tab_again_with_what_was_typed() {
    let mut panel = panel();
    panel.on_key(&key(KeyCode::Char('/')));
    typed(&mut panel, "label:bug repo:other/lib");
    let GithubKey::Search { request, section, after } = panel.on_key(&key(KeyCode::Enter)) else {
        panic!("expected a search");
    };
    assert_eq!(section.query, "is:open is:pr author:@me label:bug repo:{repo}");
    assert_eq!((section.name.as_str(), after), ("Mine", None));
    assert!(panel.editing.is_none() && panel.researching(0) && panel.rows().is_empty());
    assert!(panel.edited(0) && !panel.edited(1));
    assert!(panel.hint().contains("x reset"), "{}", panel.hint());

    panel.on_key(&key(KeyCode::Char('/')));
    assert_eq!(panel.on_key(&key(KeyCode::Enter)), GithubKey::Held, "the same search again");
    assert!(panel.pending.contains_key(&request), "and the one out is still awaited");

    panel.on_key(&key(KeyCode::Char('/')));
    for _ in 0..80 {
        panel.on_key(&key(KeyCode::Backspace));
    }
    let GithubKey::Search { request: again, section, .. } = panel.on_key(&key(KeyCode::Enter))
    else {
        panic!("expected the configured search back");
    };
    assert_eq!(section.query, "is:open is:pr author:@me repo:{repo}");
    assert!(!panel.edited(0));
    assert!(!panel.pending.contains_key(&request), "the replaced search is forgotten");
    assert!(panel.pending.contains_key(&again));
}

/// A configured tab that searches everywhere is run as typed: its scope is the reader's.
#[test]
fn a_tab_searching_everywhere_runs_as_typed() {
    let mut panel = panel();
    panel.section = 2;
    panel.on_key(&key(KeyCode::Char('/')));
    typed(&mut panel, "repo:cli/cli");
    let GithubKey::Search { section, .. } = panel.on_key(&key(KeyCode::Enter)) else {
        panic!("expected a search");
    };
    assert_eq!(section.query, "is:open is:pr review-requested:@me repo:cli/cli");
}

/// `s` steps the tab through its states, an issues tab skipping merged, and `x` puts the configured
/// search back; neither does anything with no sections to act on.
#[test]
fn s_steps_the_state_and_x_resets_it() {
    let mut panel = panel();
    assert_eq!(panel.on_key(&key(KeyCode::Char('x'))), GithubKey::Held, "nothing to reset");
    let query = |key: GithubKey| match key {
        GithubKey::Search { section, .. } => section.query,
        other => panic!("{other:?}"),
    };
    panel.section = 1;
    assert_eq!(
        query(panel.on_key(&key(KeyCode::Char('s')))),
        "is:closed is:issue assignee:@me repo:{repo}"
    );
    assert_eq!(query(panel.on_key(&key(KeyCode::Char('s')))), "is:issue assignee:@me repo:{repo}");
    assert_eq!(
        query(panel.on_key(&key(KeyCode::Char('x')))),
        "is:open is:issue assignee:@me repo:{repo}"
    );
    assert_eq!(panel.pending.len(), 1, "one search out for the tab, the latest");

    let mut loading = GithubPanel::new(&searches());
    assert_eq!(loading.on_key(&key(KeyCode::Char('s'))), GithubKey::Held);
    assert_eq!(loading.on_key(&key(KeyCode::Char('/'))), GithubKey::Held);
    assert!(loading.editing.is_none());
}

/// The cursor reaching the last row asks for the next page, once; the page lands after the rows it
/// follows without repeating one, and the last page leaves nothing more to ask for. A page that
/// fails keeps the rows and says why.
#[test]
fn the_last_row_brings_the_next_page() {
    let mut panel = panel();
    let mine = &mut panel.sections.as_mut().unwrap()[0];
    mine.total = Some(3);
    mine.next = Some("c1".into());
    let GithubKey::Search { request, after, .. } = panel.on_key(&key(KeyCode::End)) else {
        panic!("expected the next page");
    };
    assert_eq!(after.as_deref(), Some("c1"));
    assert!(panel.paging(0) && !panel.researching(0));
    assert_eq!(panel.rows().len(), 2, "the rows stay while the page is out");
    assert_eq!(panel.on_key(&key(KeyCode::Down)), GithubKey::Held, "asked once");
    let page = ListedSection {
        name: "ignored".into(),
        items: vec![
            item(true, 13, "Route by category", "upstream/app"),
            item(true, 14, "New", "acme/app"),
        ],
        error: None,
        total: Some(3),
        next: None,
    };
    assert_eq!(panel.land_search(request, None, page.clone()), None);
    let numbers: Vec<u64> = panel.rows().iter().map(|row| row.number).collect();
    assert_eq!(numbers, [12, 13, 14]);
    assert_eq!(panel.sections.as_ref().unwrap()[0].name, "Mine");
    assert_eq!(panel.on_key(&key(KeyCode::End)), GithubKey::Held, "nothing more to ask for");
    assert_eq!(panel.land_search(request, None, page), None, "an answer lands once");

    // Only the last row asks, and a page out for another tab is not this one's
    let mut longer = self::panel();
    let assigned = &mut longer.sections.as_mut().unwrap()[1];
    assigned.next = Some("other".into());
    longer.section = 1;
    let GithubKey::Search { .. } = longer.on_key(&key(KeyCode::End)) else {
        panic!("expected the other tab's page");
    };
    longer.section = 0;
    longer.selected = 0;
    let mine = &mut longer.sections.as_mut().unwrap()[0];
    mine.items.push(item(true, 15, "Third", "acme/app"));
    mine.next = Some("c1".into());
    assert_eq!(longer.on_key(&key(KeyCode::Down)), GithubKey::Held, "not the last row yet");
    assert!(!longer.paging(0));
    assert!(matches!(longer.on_key(&key(KeyCode::Down)), GithubKey::Search { .. }));

    let mut failing = self::panel();
    failing.sections.as_mut().unwrap()[0].next = Some("c1".into());
    let GithubKey::Search { request, .. } = failing.on_key(&key(KeyCode::End)) else {
        panic!("expected the next page");
    };
    let refused = ListedSection {
        name: "Mine".into(),
        items: Vec::new(),
        error: Some("GitHub is rate limiting.".into()),
        total: None,
        next: None,
    };
    assert_eq!(
        failing.land_search(request, None, refused).as_deref(),
        Some("GitHub is rate limiting.")
    );
    assert_eq!(failing.rows().len(), 2);
    assert_eq!(failing.on_key(&key(KeyCode::Down)), GithubKey::Held, "and it stops asking");
}

/// A new search's rows replace the tab's, and the cursor goes back to the top of them.
#[test]
fn a_new_search_replaces_the_rows() {
    let mut panel = panel();
    panel.on_key(&key(KeyCode::Down));
    let GithubKey::Search { request, .. } = panel.on_key(&key(KeyCode::Char('s'))) else {
        panic!("expected a search");
    };
    let found = ListedSection {
        name: "whatever the answer echoes".into(),
        items: vec![item(true, 40, "Closed one", "acme/app")],
        error: None,
        total: Some(1),
        next: None,
    };
    assert_eq!(panel.land_search(request, Some("acme/app".into()), found), None);
    assert_eq!((panel.selected, panel.rows().len()), (0, 1));
    assert_eq!(panel.sections.as_ref().unwrap()[0].name, "Mine", "the tab keeps its name");
    assert!(!panel.searching());
}

/// A search the engine never got is taken back, so its tab is not left waiting on it.
#[test]
fn a_search_never_sent_leaves_nothing_waiting() {
    let mut panel = panel();
    let GithubKey::Search { request, .. } = panel.on_key(&key(KeyCode::Char('s'))) else {
        panic!("expected a search");
    };
    assert!(panel.researching(0));
    panel.unasked(request);
    assert!(!panel.researching(0) && !panel.searching());
}

/// Nothing to move among is no reason to fall over: arrows on a loading panel, or an empty
/// section, do nothing; and a control key typed into the search is not a letter.
#[test]
fn keys_with_nothing_to_move_among_do_nothing() {
    let mut loading = GithubPanel::default();
    for code in [
        KeyCode::Right,
        KeyCode::Left,
        KeyCode::Tab,
        KeyCode::Down,
        KeyCode::Up,
        KeyCode::End,
        KeyCode::PageDown,
        KeyCode::Char('s'),
        KeyCode::Char('/'),
    ] {
        assert_eq!(loading.on_key(&key(code)), GithubKey::Held, "{code:?}");
    }
    assert_eq!((loading.section, loading.selected, loading.focus), (0, 0, Focus::List));
    let mut empty = panel();
    empty.section = 2;
    empty.on_key(&key(KeyCode::Down));
    assert_eq!(empty.selected, 0);

    let mut searching = panel();
    searching.on_key(&key(KeyCode::Char('/')));
    for modifiers in [KeyModifiers::CONTROL, KeyModifiers::ALT] {
        searching.on_key(&KeyEvent::new(KeyCode::Char('q'), modifiers));
    }
    searching.on_key(&KeyEvent::new(KeyCode::Char('R'), KeyModifiers::SHIFT));
    let typed = searching.editing.as_ref().unwrap().current_input();
    assert!(typed.ends_with("@me R"), "a capital is a letter; a chord is not: {typed}");
}

/// PageDown and PageUp move ten rows at a time, Home and End to either end.
#[test]
fn page_keys_move_by_ten() {
    let mut panel = panel();
    let mine = &mut panel.sections.as_mut().unwrap()[0];
    mine.items = (1..=25).map(|number| item(true, number, "Row", "acme/app")).collect();
    panel.on_key(&key(KeyCode::PageDown));
    assert_eq!(panel.selected, 10);
    panel.on_key(&key(KeyCode::PageUp));
    assert_eq!(panel.selected, 0);
    panel.on_key(&key(KeyCode::End));
    assert_eq!(panel.selected, 24);
    panel.on_key(&key(KeyCode::PageDown));
    assert_eq!(panel.selected, 24, "the last row stays the last");
    panel.on_key(&key(KeyCode::Home));
    assert_eq!(panel.selected, 0);
}

/// `→` hands the keys to the detail of the row under the cursor, once it is read; `←` hands
/// them back, and the hint follows the keys.
#[test]
fn the_keys_move_between_the_list_and_the_detail() {
    let mut panel = panel();
    panel.on_key(&key(KeyCode::Right));
    assert_eq!(panel.focus, Focus::List, "nothing read yet, nothing to move to");
    assert!(!panel.hint().contains("→ details"));

    let overview = luxe_core::github::overview::IssueOverview {
        number: 12,
        title: "Parse categories".into(),
        body: String::new(),
        url: "https://github.com/acme/app/pull/12".into(),
        repository: "acme/app".into(),
        author: luxe_core::github::Author { login: "mara".into(), bot: false },
        state: luxe_core::github::PullRequestState::Open,
        updated_at: 0,
        labels: Vec::new(),
        assignees: Vec::new(),
        milestone: None,
        linked: Vec::new(),
        conversation_count: 0,
    };
    panel
        .details
        .insert(overview.url.clone(), GithubDetail::new(Subject::Issue(Box::new(overview))));
    assert!(panel.hint().starts_with("enter review · → details"));
    panel.on_key(&key(KeyCode::Right));
    assert_eq!(panel.focus, Focus::Detail);
    assert!(panel.hint().contains("tab conversation"), "{}", panel.hint());
    panel.on_key(&key(KeyCode::Tab));
    assert_eq!(
        panel.detail().map(|detail| detail.tab),
        Some(crate::github_detail::DetailTab::Conversation),
        "Tab is the detail's now, not the sections'"
    );
    assert_eq!(panel.section, 0);
    panel.on_key(&key(KeyCode::Esc));
    assert_eq!(panel.focus, Focus::List, "Esc leaves the detail before it closes anything");
    assert_eq!(panel.on_key(&key(KeyCode::Esc)), GithubKey::Close);
}

fn drawn(panel: &GithubPanel, width: u16, height: u16) -> Vec<String> {
    let backend = ratatui::backend::TestBackend::new(width, height);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal.draw(|f| render(f, Rect::new(0, 0, width, height), panel, &styles(), 0)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    (0..height).map(|y| (0..width).map(|x| buffer[(x, y)].symbol().to_string()).collect()).collect()
}

/// Each row takes two lines — the title, then who opened it — and the tabs count each section.
/// Too narrow for the counts, the tabs drop them; too narrow for the names, they show a run around
/// the open one and say more are aside.
#[test]
fn the_list_puts_each_row_on_two_lines() {
    let narrow = drawn(&panel(), 64, 24);
    let tabs = narrow.iter().position(|row| row.contains("Mine")).expect("tabs");
    assert!(
        narrow[tabs].contains("Review requested") && !narrow[tabs].contains("Mine 2"),
        "names before counts: {:?}",
        narrow[tabs]
    );
    let mut aside = panel();
    aside.section = 2;
    let tight = drawn(&aside, 44, 24);
    let tabs = tight.iter().position(|row| row.contains("Review requested")).expect("tabs");
    assert!(tight[tabs].contains('‹') && !tight[tabs].contains("Mine"), "{:?}", tight[tabs]);

    let rows = drawn(&panel(), 120, 24);
    let tabs = rows.iter().position(|row| row.contains("Mine 2")).expect("tabs");
    assert!(rows[tabs].contains("Assigned issues 2") && rows[tabs].contains("Review requested 0"));
    let title = rows.iter().position(|row| row.contains("#12 Parse categories")).expect("row");
    assert!(rows[title + 1].contains("mara"), "{rows:#?}");
    let elsewhere = rows.iter().position(|row| row.contains("#13 Route")).expect("second");
    assert!(rows[elsewhere + 1].contains("upstream/app"), "a row elsewhere names where");
    assert!(rows.iter().any(|row| row.contains("enter review")), "{rows:#?}");
    assert!(rows.iter().any(|row| row.contains("1/2")), "where the cursor is");
    assert!(
        rows.iter().any(|row| row.contains("GitHub · acme/app")),
        "the repository, in the title"
    );
}

/// The line over the rows says what the tab searched for, without its scope, and how many of how
/// many; an empty tab says nothing matched rather than that there are no items, and a search with
/// more says so under the last row.
#[test]
fn the_tab_says_what_it_searched_for() {
    let mut panel = panel();
    let mine = &mut panel.sections.as_mut().unwrap()[0];
    mine.total = Some(3_250);
    mine.next = Some("c1".into());
    let rows = drawn(&panel, 120, 30);
    let line = rows.iter().find(|row| row.contains("/ is:open")).expect("the search");
    assert!(line.contains("/ is:open is:pr author:@me") && !line.contains("repo:"), "{line}");
    assert!(line.contains("2 of 3,250"), "{line}");
    assert!(rows.iter().any(|row| row.contains("Mine 3,250")), "the tab counts them all");
    assert!(rows.iter().any(|row| row.contains("3,248 more below")), "{rows:#?}");

    panel.sections.as_mut().unwrap()[1].items.clear();
    panel.section = 1;
    let rows = drawn(&panel, 120, 30);
    assert!(rows.iter().any(|row| row.contains("Nothing matches this search")), "{rows:#?}");
    assert!(!rows.iter().any(|row| row.contains("No items")));

    panel.section = 0;
    let GithubKey::Search { .. } = panel.on_key(&key(KeyCode::Char('s'))) else {
        panic!("expected a search");
    };
    let rows = drawn(&panel, 120, 30);
    assert!(rows.iter().any(|row| row.contains("Searching GitHub")), "{rows:#?}");
    assert!(rows.iter().any(|row| row.contains("Mine* ")), "an edited tab is marked");
}

/// The row the cursor rests on is read once, a quarter second after it arrives: passing through a
/// row reads nothing, and a row elsewhere is asked for on its own repository.
#[test]
fn a_row_is_read_once_the_cursor_settles_on_it() {
    let mut panel = panel();
    let start = std::time::Instant::now();
    let later = |ms| start + std::time::Duration::from_millis(ms);
    assert_eq!(panel.due_read(start), None, "it has only just arrived");
    assert!(panel.reading());
    assert_eq!(panel.due_read(later(100)), None, "not settled yet");
    assert_eq!(
        panel.due_read(later(300)),
        Some(OverviewRead { repository: None, number: 12, pull_request: true })
    );
    assert_eq!(panel.due_read(later(900)), None, "asked once");

    panel.on_key(&key(KeyCode::Down));
    assert_eq!(panel.due_read(later(1_000)), None, "a new row starts the clock again");
    assert_eq!(
        panel.due_read(later(1_300)),
        Some(OverviewRead {
            repository: Some("upstream/app".into()),
            number: 13,
            pull_request: true
        })
    );
}

/// An answer lands in the detail of the row it answers: the branch's own pull request can have its
/// threads worked through here, another cannot, and a failure is kept to say why.
#[test]
fn an_answer_lands_in_its_own_row() {
    use luxe_core::github::overview::Overview;
    let mut panel = panel();
    let overview = |number| {
        Overview::PullRequest(Box::new(luxe_core::github::overview::PullRequestOverview {
            number,
            ..crate::fixture::pull_request_overview()
        }))
    };
    panel.land(None, 12, Some(overview(12)), None, None, Some(12));
    let mine = panel.details.get("https://github.com/acme/app/pull/12").expect("landed");
    assert!(mine.on_branch);
    assert!(!panel.reading());

    panel.land(Some("upstream/app"), 13, Some(overview(13)), None, None, Some(13));
    let theirs = panel.details.get("https://github.com/upstream/app/pull/13").expect("landed");
    assert!(!theirs.on_branch, "a fork's pull request is not the branch checked out here");

    panel.land(Some("nobody/else"), 99, Some(overview(99)), None, None, None);
    assert_eq!(panel.details.len(), 2, "an answer for no row is dropped");

    panel.section = 1;
    panel.land(None, 7, None, Some("GitHub has no such issue.".into()), None, None);
    assert!(!panel.reading(), "a failure is an answer");
    let drawn = drawn(&panel, 150, 30);
    assert!(drawn.iter().any(|row| row.contains("GitHub has no such issue.")), "{drawn:#?}");
}

/// Enter on an issue one of your sessions took up resumes that session rather than starting a
/// second; without one, it starts a session on the issue as before.
#[test]
fn enter_on_an_issue_you_took_up_resumes_your_session() {
    use luxe_core::github::overview::{IssueOverview, LocalSession, Overview};
    let mut panel = panel();
    panel.section = 1;
    let issue = IssueOverview {
        number: 7,
        title: "Total skips a number".into(),
        body: String::new(),
        url: "https://github.com/acme/app/pull/7".into(),
        repository: "acme/app".into(),
        author: luxe_core::github::Author { login: "mara".into(), bot: false },
        state: luxe_core::github::PullRequestState::Open,
        updated_at: 0,
        labels: Vec::new(),
        assignees: Vec::new(),
        milestone: None,
        linked: Vec::new(),
        conversation_count: 1,
    };
    let local =
        LocalSession { session: "s-7".into(), branch: Some("7-total".into()), last_used: 0 };
    panel.land(None, 7, Some(Overview::Issue(Box::new(issue))), None, Some(local), None);
    assert!(panel.hint().starts_with("enter resume"));
    assert_eq!(panel.on_key(&key(KeyCode::Enter)), GithubKey::Resume("s-7".into()));
    panel.details.clear();
    assert_eq!(panel.on_key(&key(KeyCode::Enter)), GithubKey::TakeUpIssue(7));
}

/// A row's diff counts are shown whole or not at all: cut to fit, `+96 −31` came out `+…`. The
/// status on the right, the part that says what to do, keeps its place either way.
#[test]
fn a_row_drops_its_counts_rather_than_cutting_them() {
    let rows_at = |width: u16| {
        let mut app = crate::fixture::seeded_app("nord", crate::fixture::FixtureScenario::ChatIdle)
            .expect("fixture app");
        crate::fixture::seed_github(&mut app, "github");
        let backend = ratatui::backend::TestBackend::new(width, 34);
        let mut terminal = ratatui::Terminal::new(backend).expect("terminal");
        app.update(crate::app::Msg::TerminalResized { width, height: 34 });
        terminal.draw(|frame| crate::view::render(frame, &mut app)).expect("draw");
        let buffer = terminal.backend().buffer().clone();
        (0..34)
            .map(|y| (0..width).map(|x| buffer[(x, y)].symbol().to_string()).collect::<String>())
            .collect::<Vec<_>>()
    };
    for width in [140, 220] {
        let rows = rows_at(width);
        let line = rows.iter().find(|row| row.contains("1 failing")).expect("the row's status");
        assert!(!line.contains("+…"), "{line}");
        assert!(line.contains("changes requested"), "{line}");
    }
    let wide = rows_at(220);
    assert!(wide.iter().any(|row| row.contains("+96 −31")), "with room, the counts are there");
}
