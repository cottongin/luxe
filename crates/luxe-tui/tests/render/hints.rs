//! Hints — what an idle turn offers, the first-steps line and the review footer.

use super::*;

/// The turn-end invitation: quiet, honest, and only once a review has actually happened.
#[test]
fn an_idle_turn_invites_a_review_of_what_is_unreviewed() {
    let (width, height) = (120, 30);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.update(Msg::TerminalResized { width, height });
    let mut snapshot = empty_workspace("invitation");
    snapshot.changes = WorkspaceChangesSummary {
        files: vec![
            FileChangeSummary {
                path: "src/lib.rs".into(),
                status: FileChangeStatus::Modified,
                additions: Some(12),
                deletions: Some(3),
                binary: false,
            },
            FileChangeSummary {
                path: "src/new.rs".into(),
                status: FileChangeStatus::Added,
                additions: Some(40),
                deletions: Some(0),
                binary: false,
            },
        ],
        total_files: 2,
        additions: Some(52),
        deletions: Some(3),
        truncated: false,
    };
    app.workspace = Some(snapshot);

    // No prior review: "everything is unreviewed" is true and useless, so nothing is said.
    let first = buffer_to_string(&render_app(&mut app, &backend));
    assert!(!first.contains("since your last review"), "nothing to compare against: {first}");

    // After a review that covered one of the two files, the other is the increment.
    app.review.seen = Some(luxe_core::guided_review::SeenReview {
        revision_id: None,
        at_ms: 1,
        item_ids: vec!["hunk:seen".into()],
        files: [("src/lib.rs".to_string(), (12, 3))].into_iter().collect(),
    });
    let content = buffer_to_string(&render_app(&mut app, &backend));

    assert!(
        content.contains("1 file changed since your last review"),
        "the increment is named: {content}"
    );
    assert!(content.contains("ctrl+g to review"), "and it says how: {content}");
    assert!(
        !content.contains("for commands"),
        "the invitation is about something that happened, so it has the row: {content}"
    );
}

/// The row above the composer, as text, and where the composer's frame starts.
pub(super) fn row_above_composer(buffer: &Buffer, width: u16, height: u16) -> (String, u16) {
    let composer = (0..height)
        .rev()
        .find(|&y| {
            let row = row_text(buffer, y, width);
            row.contains('╭') && row.contains('╮')
        })
        .expect("the composer's frame");
    (row_text(buffer, composer - 1, width), composer)
}

/// A conversation that has not started says where the composer's doors are, in the row above it,
/// and says nothing once there is a message. Nothing else on screen names `/`, `Ctrl+K` or `@`.
#[test]
fn a_fresh_conversation_names_the_composers_doors_until_its_first_message() {
    let (width, height) = (190, 30);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.update(Msg::TerminalResized { width, height });

    let (row, _) = row_above_composer(&render_app(&mut app, &backend), width, height);
    assert!(
        row.contains("shift+tab for plan mode  ·  / or ctrl+k for commands  ·  @ to reference files, skills, and more  ·  /help for keys"),
        "{row:?}"
    );
    let buffer = render_app(&mut app, &backend);
    let (row, composer) = row_above_composer(&buffer, width, height);
    let styled = |needle: &str| {
        let x = row[..row.find(needle).expect(needle)].chars().count() as u16;
        buffer[(x, composer - 1)].fg
    };
    assert_eq!(Some(styled("ctrl+k")), app.theme_styles.accent.fg, "a key reads as a key");
    assert_eq!(Some(styled("shift+tab")), app.theme_styles.accent.fg, "the mode key too");
    assert_eq!(Some(styled("for commands")), app.theme_styles.muted.fg, "the words stay quiet");

    // A notice — the line `--issue` leaves — does not start a conversation
    app.push_convo(ConvoBlock::System("Issue #12 — will be sent with your first message.".into()));
    let (row, _) = row_above_composer(&render_app(&mut app, &backend), width, height);
    assert!(row.contains("for commands"), "a notice is not a conversation: {row:?}");

    app.push_convo(ConvoBlock::User {
        text: "hello".into(),
        attached: Vec::new(),
        chips: Vec::new(),
    });
    let (row, _) = row_above_composer(&render_app(&mut app, &backend), width, height);
    assert!(!row.contains("for commands"), "gone once the conversation starts: {row:?}");

    // Nor does a conversation with a reply in it count as fresh, whoever wrote the rest
    let mut replied = create_app();
    replied.update(Msg::TerminalResized { width, height });
    replied.push_convo(ConvoBlock::Assistant("Here is the plan.".into()));
    let (row, _) = row_above_composer(&render_app(&mut replied, &backend), width, height);
    assert!(!row.contains("for commands"), "{row:?}");
}

/// The mode hint names the mode the key would switch to, and the key that is bound to do it.
#[test]
fn the_first_steps_hint_names_the_mode_shift_tab_reaches() {
    let (width, height) = (190, 30);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.update(Msg::TerminalResized { width, height });
    app.status.mode = "plan".into();
    let (row, _) = row_above_composer(&render_app(&mut app, &backend), width, height);
    assert!(
        row.contains(" shift+tab for build mode  ·  / or"),
        "planning, it offers build: {row:?}"
    );

    let keymap = KeyMap::defaults().with_overrides(std::collections::HashMap::from([(
        "tui.app.cycle_mode".to_string(),
        vec!["not a chord".to_string()],
    )]));
    let (tx, _rx) = tokio::sync::mpsc::channel::<luxe_core::engine::Action>(8);
    let mut unbound = App::new(tx, keymap, Config::default());
    unbound.update(Msg::TerminalResized { width, height });
    let (row, _) = row_above_composer(&render_app(&mut unbound, &backend), width, height);
    assert!(!row.contains("mode") && row.contains("/ or ctrl+k"), "unbound, left out: {row:?}");
}

/// The palette's key is the one bound now; with none bound it is left out rather than invented.
#[test]
fn the_first_steps_hint_names_the_palette_key_you_bound() {
    let (width, height) = (140, 30);
    let backend = test_backend(width, height);
    let bound = |chords: &[&str]| {
        let keymap = KeyMap::defaults().with_overrides(std::collections::HashMap::from([(
            "tui.app.open_palette".to_string(),
            chords.iter().map(|chord| chord.to_string()).collect(),
        )]));
        let (tx, _rx) = tokio::sync::mpsc::channel::<luxe_core::engine::Action>(8);
        let mut app = App::new(tx, keymap, Config::default());
        app.update(Msg::TerminalResized { width, height });
        row_above_composer(&render_app(&mut app, &backend), width, height).0
    };
    assert!(bound(&["alt+p"]).contains("/ or alt+p for commands"));
    // Every chord unparseable leaves the action unbound
    let unbound = bound(&["not a chord"]);
    assert!(unbound.contains(" / for commands  ·  @"), "{unbound:?}");
}

/// On a narrow terminal the hint gives up its tail first, and says nothing rather than half a
/// phrase when even the commands will not fit. A running turn has the row, and so does a review
/// invitation, which is about something that happened.
#[test]
fn the_first_steps_hint_shortens_from_its_tail_and_gives_way() {
    let height = 30;
    let row_at = |width: u16, app: &mut App| {
        let backend = test_backend(width, height);
        app.update(Msg::TerminalResized { width, height });
        row_above_composer(&render_app(app, &backend), width, height).0
    };
    let mut app = create_app();
    let medium = row_at(80, &mut app);
    assert!(medium.contains("shift+tab for plan mode  ·  / or ctrl+k for commands"), "{medium:?}");
    assert!(!medium.contains('@'), "the tail goes first: {medium:?}");
    let narrow = row_at(40, &mut app);
    assert!(narrow.contains("shift+tab for plan mode") && !narrow.contains('/'), "{narrow:?}");
    let tiny = row_at(22, &mut app);
    assert!(!tiny.contains("for comm") && !tiny.contains("shift"), "no half phrases: {tiny:?}");

    app.status.flash_toast("Copied to clipboard");
    let toasted = row_at(140, &mut app);
    assert!(toasted.contains("Copied") && !toasted.contains("for commands"), "{toasted:?}");
    app.status.toast = None;

    app.status.state = luxe_tui::app::AppState::Streaming;
    let running = row_at(140, &mut app);
    assert!(!running.contains("for commands"), "a running turn owns the row: {running:?}");
}

/// The footer keeps to the keys a reader works with. The reading toggles — layout, guide,
/// churn — are listed by help, and a footer that also carried them stopped being read at all.
#[test]
fn the_footer_keeps_to_the_keys_that_work() {
    let (width, height) = (200, 30);
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewNoise).unwrap();
    app.update(Msg::TerminalResized { width, height });
    let buffer = render_app(&mut app, &test_backend(width, height));
    // The diff pane's share of the row: the rail's foot sits beside it, across the rule
    let row = row_text(&buffer, height - 1, width);
    let footer = row.rsplit('│').next().unwrap_or_default();
    assert_eq!(
        footer.trim(),
        "space next · c comment · a ask agent · alt+enter finish · ? help",
        "{footer}"
    );
}

/// Scrolling clamps at the file's last row, and the pane draws one file at a time — so without a
/// marker there, hitting the bottom is indistinguishable from a scroll that stopped working.
#[test]
fn the_bottom_of_a_file_says_so_when_the_chapter_carries_on() {
    let (width, height) = (140, 30);
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewLarge).unwrap();
    app.update(Msg::TerminalResized { width, height });

    // The first chapter owns several files, so the first file's end is not the chapter's end.
    for _ in 0..200 {
        app.update(Msg::GuidedReviewLineNext);
    }
    let text = buffer_to_string(&render_app(&mut app, &test_backend(width, height)));
    assert!(text.contains("end of file"), "the bottom of the file is named: {text}");
    assert!(text.contains("— end of file —"), "and nothing else, mid-chapter: {text}");
}

/// Stepping to a chapter's last stop is what records it reviewed; scrolling there is not. The
/// end-of-reading marker is the only place that distinction becomes visible.
#[test]
fn the_end_of_a_chapter_says_whether_it_was_reviewed() {
    let (width, height) = (120, 30);
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).unwrap();
    app.update(Msg::TerminalResized { width, height });

    for _ in 0..60 {
        app.update(Msg::GuidedReviewLineNext);
    }
    let scrolled = buffer_to_string(&render_app(&mut app, &test_backend(width, height)));
    assert!(
        scrolled.contains("end of the last chapter"),
        "scrolling to the end is not reviewing it: {scrolled}"
    );

    for _ in 0..6 {
        app.update(Msg::GuidedReviewHunkNext);
    }
    let stepped = buffer_to_string(&render_app(&mut app, &test_backend(width, height)));
    assert!(stepped.contains("last chapter reviewed"), "stepping through it records it: {stepped}");
}

/// The fold controls describe churn. A review that has none is the common case for a
/// feature-sized change, and offering `f`/`d` there taught the reviewer that pressing a key
/// named on screen can do nothing at all — which costs every other key its credibility.
#[test]
fn the_fold_controls_appear_only_where_there_is_churn_to_fold() {
    let (width, height) = (120, 30);

    let mut clean = seeded_app("nord", FixtureScenario::GuidedReviewReady).unwrap();
    clean.update(Msg::TerminalResized { width, height });
    let text = buffer_to_string(&render_app(&mut clean, &test_backend(width, height)));
    assert!(!text.contains("f opens them"), "nothing mechanical, so no fold control: {text}");
    assert!(!text.contains("d why"), "and nothing for it to explain: {text}");
    assert!(text.contains("space next"), "the working controls stay: {text}");

    let mut noisy = seeded_app("nord", FixtureScenario::GuidedReviewNoise).unwrap();
    noisy.update(Msg::TerminalResized { width, height });
    let text = buffer_to_string(&render_app(&mut noisy, &test_backend(width, height)));
    assert!(text.contains("f opens them"), "a formatting-only hunk earns the control: {text}");
}

/// Prints the FILES section so its shape can be reviewed as text, without a screenshot.
#[test]
fn dock_changes_shape_snapshot() {
    let (width, height) = (160, 44);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.update(Msg::TerminalResized { width, height });
    let mut snapshot = empty_workspace("luxe");
    let file = |path: &str, status: FileChangeStatus, a: Option<u64>, d: Option<u64>| {
        FileChangeSummary { path: path.into(), status, additions: a, deletions: d, binary: false }
    };
    snapshot.changes = WorkspaceChangesSummary {
        files: vec![
            file("crates/luxe-core/src/usage.rs", FileChangeStatus::Added, Some(214), Some(0)),
            file(
                "crates/luxe-core/src/engine/usage.rs",
                FileChangeStatus::Added,
                Some(160),
                Some(0),
            ),
            file(
                "crates/luxe-core/src/engine/turn.rs",
                FileChangeStatus::Modified,
                Some(18),
                Some(24),
            ),
            file(
                "crates/luxe-core/src/engine/compact.rs",
                FileChangeStatus::Modified,
                Some(14),
                Some(2),
            ),
            file(
                "crates/luxe-tui/src/view/dock.rs",
                FileChangeStatus::Modified,
                Some(96),
                Some(31),
            ),
            file(
                "crates/luxe-tui/src/app/status.rs",
                FileChangeStatus::Modified,
                Some(41),
                Some(48),
            ),
            file("crates/luxe-tui/src/cost_report.rs", FileChangeStatus::Added, Some(131), Some(0)),
            file(
                "docs/design/a-very-long-design-document-name.md",
                FileChangeStatus::Untracked,
                Some(402),
                Some(0),
            ),
            file("assets/logo.png", FileChangeStatus::Added, None, None),
        ],
        total_files: 9,
        additions: Some(1076),
        deletions: Some(105),
        truncated: false,
    };
    app.workspace = Some(snapshot);

    let buffer = render_app(&mut app, &backend);
    println!("\n=== mixed roots ===\n{}", changes_block(&buffer, height));

    // Everything under one root: the prefix is said once and the headings shorten.
    let mut app = create_app();
    app.update(Msg::TerminalResized { width, height });
    let mut snapshot = empty_workspace("luxe");
    snapshot.changes = WorkspaceChangesSummary {
        files: vec![
            file("crates/luxe-core/src/usage.rs", FileChangeStatus::Added, Some(214), Some(0)),
            file(
                "crates/luxe-core/src/engine/turn.rs",
                FileChangeStatus::Modified,
                Some(18),
                Some(24),
            ),
            file(
                "crates/luxe-tui/src/view/dock.rs",
                FileChangeStatus::Modified,
                Some(96),
                Some(31),
            ),
        ],
        total_files: 3,
        additions: Some(328),
        deletions: Some(55),
        truncated: false,
    };
    app.workspace = Some(snapshot);
    let buffer = render_app(&mut app, &backend);
    let shared = changes_block(&buffer, height);
    println!("\n=== shared root ===\n{shared}");
    assert!(shared.contains("crates"), "one folder heads the tree: {shared}");
    assert!(shared.contains("luxe-core/src"), "a lone chain is one row: {shared}");
    assert!(shared.contains("usage.rs"), "files are named: {shared}");
}

/// The FILES block as plain text, for reviewing its shape without a screenshot.
pub(super) fn changes_block(buffer: &Buffer, height: u16) -> String {
    (0..height)
        .map(|y| row_text(buffer, y, 42).trim_end().to_string())
        .skip_while(|line| !line.contains("FILES"))
        .take_while(|line| !line.contains("MODEL"))
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Prints the guide pane so its shape can be reviewed as text, without a screenshot.
#[test]
fn guide_pane_shape_snapshot() {
    let (width, height) = (160, 44);
    let backend = test_backend(width, height);
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewBrief).unwrap();
    app.update(Msg::TerminalResized { width, height });
    let buffer = render_app(&mut app, &backend);
    let guide_width = luxe_tui::workspace_layout::guide_pane_width(width);
    let guide: Vec<String> =
        (0..height).map(|y| row_text(&buffer, y, guide_width).trim_end().to_string()).collect();
    println!("\n{}", guide.join("\n").trim_end());
    assert!(guide.iter().any(|line| line.contains("REVIEW")), "the pane names itself");
    assert!(guide.iter().any(|line| line.contains("Step ")), "and says where you are");
}

/// An untracked *directory* is one dock row and several review files, so comparing counts made
/// "Workspace changed" permanently true over a review that matched the working tree exactly.
#[test]
fn a_pinned_review_that_matches_the_working_tree_is_not_called_stale() {
    let (width, height) = (170, 40);
    let backend = test_backend(width, height);
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).unwrap();
    app.update(Msg::TerminalResized { width, height });
    let mut snapshot = empty_workspace("staleness");
    // Git collapses the untracked directory; the review lists the one file inside it.
    snapshot.changes = WorkspaceChangesSummary {
        files: vec![FileChangeSummary {
            path: "src/".into(),
            status: FileChangeStatus::Untracked,
            additions: None,
            deletions: None,
            binary: false,
        }],
        total_files: 1,
        additions: None,
        deletions: None,
        truncated: false,
    };
    app.workspace = Some(snapshot.clone());

    let matching = buffer_to_string(&render_app(&mut app, &backend));
    assert!(
        !matching.contains("Workspace changed"),
        "the review covers exactly what the dock reports: {matching}"
    );

    // A file the review has never heard of *is* a real divergence.
    snapshot.changes.files.push(FileChangeSummary {
        path: "src/elsewhere.rs".into(),
        status: FileChangeStatus::Modified,
        additions: Some(1),
        deletions: Some(0),
        binary: false,
    });
    snapshot.changes.total_files = 2;
    app.workspace = Some(snapshot);

    let moved = buffer_to_string(&render_app(&mut app, &backend));
    assert!(moved.contains("Workspace changed"), "real drift still says so: {moved}");
}
