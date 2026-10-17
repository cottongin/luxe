use super::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::Terminal;

fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::create_dir_all(dir.path().join(".git")).expect("git marker");
    dir
}

fn theme() -> ThemeStyles {
    let resolved = luxe_core::theme::ThemeRegistry::new().resolve("dark").expect("dark");
    ThemeStyles::from_resolved(&resolved, "dark")
}

/// One row of each kind, `skills` of them under SKILLS — enough to overflow any box.
fn many(skills: usize) -> ContextFilesState {
    let row = |kind, label: String| ContextFileRow {
        kind,
        label,
        scope: "this project".into(),
        path: PathBuf::from(format!("/repo/{kind:?}")),
        creates: false,
        reach: None,
        duplicate: false,
    };
    let mut rows = vec![row(ContextKind::Instructions, "AGENTS.md".into())];
    rows.extend((0..skills).map(|index| row(ContextKind::Skill, format!("skill-{index:02}"))));
    rows.push(row(ContextKind::McpServer, "postgres".into()));
    ContextFilesState::from_rows(rows)
}

/// Draws the surface into a buffer the size of a terminal.
fn draw(state: &ContextFilesState, width: u16, height: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    let hit = std::cell::RefCell::default();
    terminal.draw(|frame| render(frame, frame.area(), state, &theme(), &hit)).expect("draw");
    terminal.backend().buffer().clone()
}

fn screen(buffer: &Buffer) -> String {
    (0..buffer.area.height)
        .map(|y| (0..buffer.area.width).map(|x| buffer[(x, y)].symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

/// A project with more context than the terminal has rows still has every row reachable.
///
/// The box used to grow to the content and clip whatever the viewport could not hold, so a
/// project carrying forty skills had a list whose last rows could be selected, opened by
/// Enter, and never seen.
#[test]
fn a_list_longer_than_the_box_scrolls_to_the_row_the_cursor_is_on() {
    let mut state = many(40);
    let last = state.rows().len() - 1;
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);

    // At rest the window is at the top, and the far end of the list is not on it.
    let top = screen(&draw(&state, 100, 24));
    assert!(top.contains("skill-00"), "{top}");
    assert!(!top.contains("postgres"), "a 42-row list does not fit 24 rows: {top}");

    // End walks to the last row, and the draw brings it into view under its own heading.
    state.on_key(&key(KeyCode::End));
    assert_eq!(state.selected(), last);
    let bottom = screen(&draw(&state, 100, 24));
    assert!(bottom.contains("postgres"), "{bottom}");
    assert!(bottom.contains("MCP SERVERS"), "the row arrives with its heading: {bottom}");
    assert!(!bottom.contains("skill-00"), "and the top has scrolled away: {bottom}");

    // Home comes back, and Enter still opens the row the cursor names rather than the row the
    // window happens to be showing.
    state.on_key(&key(KeyCode::Home));
    assert_eq!(state.selected(), 0);
    assert!(screen(&draw(&state, 100, 24)).contains("AGENTS.md"));
    assert_eq!(
        state.on_key(&key(KeyCode::Enter)),
        ContextFilesOutcome::EditFile(PathBuf::from("/repo/Instructions"))
    );
}

/// The window follows the cursor by the least it can, and pages and the wheel clamp where the
/// arrows wrap.
#[test]
fn the_window_moves_only_when_the_cursor_leaves_it() {
    let mut state = many(40);
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    draw(&state, 100, 24);

    // Stepping down inside the window does not move it.
    for _ in 0..4 {
        state.on_key(&key(KeyCode::Down));
    }
    draw(&state, 100, 24);
    assert_eq!(state.scroll.get(), 0, "the cursor was still on screen");

    // Walking off the bottom moves it by exactly the overshoot.
    while state.selected() < 30 {
        state.on_key(&key(KeyCode::Down));
        draw(&state, 100, 24);
    }
    let deep = state.scroll.get();
    assert!(deep > 0, "the window followed the cursor down");
    state.on_key(&key(KeyCode::Up));
    draw(&state, 100, 24);
    assert_eq!(state.scroll.get(), deep, "and did not chase it back up from inside the window");

    // A page is a viewport's worth of rows, and neither it nor the wheel wraps.
    let before = state.selected();
    state.on_key(&key(KeyCode::PageUp));
    assert!(state.selected() < before);
    for _ in 0..200 {
        state.on_key(&key(KeyCode::PageUp));
        state.scroll_by(true);
    }
    assert_eq!(state.selected(), 0, "paging up stops at the first row");
    for _ in 0..200 {
        state.on_key(&key(KeyCode::PageDown));
        state.scroll_by(false);
    }
    assert_eq!(state.selected(), state.rows().len() - 1, "and down at the last");

    // The arrows still wrap, which is what every other list on this surface does.
    state.on_key(&key(KeyCode::Down));
    assert_eq!(state.selected(), 0);
}

/// A row half under the scrollbar is a row that opens a file when you reach for the track.
#[test]
fn a_scrolling_list_gives_the_bar_a_column_of_its_own() {
    let long = many(40);
    let short = many(1);
    let bar = |state: &ContextFilesState| {
        let hit = std::cell::RefCell::default();
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).expect("terminal");
        terminal.draw(|frame| render(frame, frame.area(), state, &theme(), &hit)).expect("draw");
        let regions = hit.borrow();
        let row = (0..24u16)
            .flat_map(|y| (0..100u16).map(move |x| (x, y)))
            .find(|(x, y)| matches!(regions.at(*x, *y), Some(MouseTarget::ContextFileRow(_))))
            .map(|(_, y)| y)
            .expect("a row is clickable");
        let right = (0..100u16)
            .rfind(|x| matches!(regions.at(*x, row), Some(MouseTarget::ContextFileRow(_))))
            .expect("the row spans cells");
        (row, right)
    };
    use crate::mouse::MouseTarget;

    let (_, scrolling) = bar(&long);
    let (_, whole) = bar(&short);
    assert_eq!(scrolling + 1, whole, "the bar's column is not part of any row");
}

/// Points every home lookup at a fresh directory, on every platform, so the machine's own
/// `~/.agents` and `~/.config/luxe` stay out of a test about the project.
///
/// The home is a directory and an environment held until the test ends.
struct IsolatedHome {
    dir: tempfile::TempDir,
    _env: crate::test_env::EnvGuard,
}

impl std::ops::Deref for IsolatedHome {
    type Target = tempfile::TempDir;

    fn deref(&self) -> &Self::Target {
        &self.dir
    }
}

fn isolated_home() -> IsolatedHome {
    let dir = tempfile::tempdir().expect("temp home");
    let path = dir.path().to_str().expect("a temp dir is UTF-8").to_owned();
    let config = dir.path().join(".config");
    let config = config.to_str().expect("a temp dir is UTF-8");
    let env = crate::test_env::EnvGuard::set(&[
        ("HOME", Some(&path)),
        ("USERPROFILE", Some(&path)),
        ("XDG_CONFIG_HOME", Some(config)),
        ("LUXE_CONFIG_DIR", None),
    ]);
    IsolatedHome { dir, _env: env }
}

fn instruction_rows(rows: &[ContextFileRow]) -> Vec<&ContextFileRow> {
    rows.iter().filter(|row| row.kind == ContextKind::Instructions && !row.creates).collect()
}

/// The list is the walk prompt assembly does, so the count beside it cannot be a different
/// number than the rows under it.
#[test]
fn the_rows_are_the_files_the_prompt_will_read() {
    let _home = isolated_home();
    let dir = repo();
    let root = dir.path();
    std::fs::write(root.join("AGENTS.md"), "rules").expect("write");

    let state = ContextFilesState::open(root);
    let loaded = instruction_rows(state.rows());
    assert_eq!(loaded.len(), luxe_core::instructions::context_files(root).len());
    assert_eq!(loaded[0].label, "AGENTS.md", "{:?}", state.rows());
    assert_eq!(loaded[0].scope, "this project");
    assert_eq!(loaded[0].noun(), "project context");
}

/// The reported confusion: `~/.config/luxe/agents.md` and `AGENTS.md`, both "context file".
#[test]
fn global_and_project_files_say_which_they_are() {
    let home = isolated_home();
    let global = home.path().join(".config").join("luxe").join("agents.md");
    std::fs::create_dir_all(global.parent().expect("parent")).expect("config dir");
    std::fs::write(&global, "mine").expect("write");
    let dir = repo();
    std::fs::write(dir.path().join("AGENTS.md"), "the project's").expect("write");

    let rows = discover(dir.path(), dir.path());
    let nouns: Vec<_> = instruction_rows(&rows).iter().map(|row| row.noun()).collect();
    assert_eq!(nouns, ["global context", "project context"]);
}

/// Launched in a package, the list reads the package's file too — as the prompt always did,
/// while the dashboard walked from the root and came up one file short.
#[test]
fn the_walk_starts_where_luxe_was_launched() {
    let _home = isolated_home();
    let dir = repo();
    let root = dir.path();
    let package = root.join("packages").join("app");
    std::fs::create_dir_all(&package).expect("nested dirs");
    std::fs::write(root.join("AGENTS.md"), "shared").expect("write");
    std::fs::write(package.join("AGENTS.md"), "package").expect("write");

    let rows = discover(root, &package);
    let found = instruction_rows(&rows);
    assert_eq!(found.len(), 2, "{rows:?}");
    assert_eq!(found[1].label, "packages/app/AGENTS.md");
    assert_eq!(found[1].noun(), "folder context");
    assert_eq!(found[1].scope, "work under packages/app");
}

/// A copy is shown for what it is rather than hidden: hiding it would put the offer to create
/// a file that exists back on the list.
#[test]
fn a_repeated_file_is_listed_as_read_once() {
    let _home = isolated_home();
    let dir = repo();
    let root = dir.path();
    let package = root.join("app");
    std::fs::create_dir_all(&package).expect("nested dir");
    std::fs::write(root.join("AGENTS.md"), "same\n").expect("write");
    std::fs::write(package.join("AGENTS.md"), "same\n").expect("write");

    let rows = discover(root, &package);
    let copy = rows.iter().find(|row| row.path == package.join("AGENTS.md")).expect("listed");
    assert!(copy.duplicate);
    assert_eq!(copy.scope, "same as AGENTS.md \u{2014} read once");
}

/// With a shared global file in place, starting a second one is not on offer.
#[test]
fn a_shared_global_file_counts_as_having_one() {
    let home = isolated_home();
    let dir = repo();
    let offered = |rows: &[ContextFileRow]| {
        rows.iter().any(|row| row.creates && row.reach == Some(ContextScope::Global))
    };
    assert!(offered(&discover(dir.path(), dir.path())), "nothing global yet");

    let shared = home.path().join(".agents");
    std::fs::create_dir_all(&shared).expect("agents dir");
    std::fs::write(shared.join("AGENTS.md"), "every agent").expect("write");
    let rows = discover(dir.path(), dir.path());
    assert!(!offered(&rows));
    assert_eq!(instruction_rows(&rows)[0].scope, "every project, and every agent that reads it");
}

/// An offer to start a file that already exists is a way to lose the one that does.
#[test]
fn a_scope_that_already_has_a_file_is_not_offered_a_second_one() {
    let dir = repo();
    let root = dir.path();
    assert!(
        ContextFilesState::open(root)
            .rows()
            .iter()
            .any(|row| row.creates && row.path == root.join("AGENTS.md")),
        "with nothing there, starting one is on offer"
    );

    std::fs::write(root.join("AGENTS.md"), "rules").expect("write");
    assert!(
        !ContextFilesState::open(root)
            .rows()
            .iter()
            .any(|row| row.creates && row.path == root.join("AGENTS.md")),
        "and once it exists, it is not"
    );
}

/// Enter hands back the path the row named, and Esc leaves without one.
#[test]
fn enter_opens_the_selected_row_and_esc_leaves() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let dir = repo();
    let root = dir.path();
    std::fs::write(root.join("AGENTS.md"), "rules").expect("write");
    let mut state = ContextFilesState::open(root);
    let first = state.rows()[0].path.clone();

    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    assert_eq!(state.on_key(&key(KeyCode::Enter)), ContextFilesOutcome::EditFile(first));
    assert_eq!(state.on_key(&key(KeyCode::Esc)), ContextFilesOutcome::Close);

    // The cursor wraps rather than sticking, like every other list on this surface.
    let last = state.rows().len() - 1;
    state.on_key(&key(KeyCode::Up));
    assert_eq!(state.selected(), last);
    state.on_key(&key(KeyCode::Down));
    assert_eq!(state.selected(), 0);
}

/// The global file is named the way its reader knows it, not by an absolute path.
#[test]
fn a_path_is_shown_the_way_its_reader_knows_it() {
    let dir = repo();
    let root = dir.path();
    assert_eq!(display_path(&root.join("AGENTS.md"), root), "AGENTS.md");
    if let Some(home) = luxe_core::user_dirs::home_dir() {
        assert_eq!(display_path(&home.join("a").join("b.md"), root), "~/a/b.md");
    }
}
