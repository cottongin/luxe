//! Dock — the rail's sections, its changes, counts and scrolling.

use super::*;

pub(super) fn empty_workspace(scope: &str) -> WorkspaceSnapshot {
    WorkspaceSnapshot::empty(
        WorkspaceScopeId::new(scope),
        ProjectIdentity::non_git("/work/luxe".into(), "luxe", "~/work/luxe"),
    )
}

/// The Changes dock section always surfaces the way into review when there are changes (not only
/// on overflow), and it says which key, so review stays discoverable without a keymap lookup.
#[test]
fn changes_section_surfaces_the_review_changes_affordance() {
    let (width, height) = (160, 40);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.update(Msg::TerminalResized { width, height });
    let mut snapshot = empty_workspace("changes-affordance");
    snapshot.changes = WorkspaceChangesSummary {
        files: vec![FileChangeSummary {
            path: "src/lib.rs".into(),
            status: FileChangeStatus::Modified,
            additions: Some(2),
            deletions: Some(1),
            binary: false,
        }],
        total_files: 1,
        additions: Some(2),
        deletions: Some(1),
        truncated: false,
    };
    app.workspace = Some(snapshot);

    let content = buffer_to_string(&render_app(&mut app, &backend));
    assert!(content.contains("FILES"), "Files section renders: {content}");
    assert!(
        content.contains("ctrl+p preview · ctrl+g review"),
        "the Changes section surfaces both ways in, even without overflow: {content}"
    );
}

/// A folder row is a name and nothing else. The ± numbers belong to the file rows under it;
/// repeating a total on every folder doubled the digits in a 42-cell rail for no new information.
/// A file at the project root sits at the tree's top level, under no heading at all.
#[test]
fn the_tree_counts_on_files_and_never_on_folders() {
    let (width, height) = (160, 40);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.update(Msg::TerminalResized { width, height });
    let changed = |path: &str, additions, deletions| FileChangeSummary {
        path: path.into(),
        status: FileChangeStatus::Modified,
        additions: Some(additions),
        deletions: Some(deletions),
        binary: false,
    };
    let mut snapshot = empty_workspace("changes-directories");
    snapshot.changes = WorkspaceChangesSummary {
        files: vec![
            changed("README.md", 7, 2),
            changed("src/lib.rs", 4, 1),
            changed("src/app.rs", 9, 3),
        ],
        total_files: 3,
        additions: Some(20),
        deletions: Some(6),
        truncated: false,
    };
    app.workspace = Some(snapshot);

    let buffer = render_app(&mut app, &backend);
    let row = |needle: &str| {
        (0..height)
            .map(|y| row_text(&buffer, y, width))
            .find(|text| text.contains(needle))
            .unwrap_or_else(|| panic!("no dock row containing {needle:?}"))
    };
    let unnumbered = |text: &str| !text.contains('+') && !text.contains('\u{2212}');

    let root = row("README.md");
    assert!(root.contains("+7") && root.contains('M'), "a root file is a row like any: {root}");
    let source = row(" src");
    assert!(unnumbered(&source), "a folder carries no counts: {source}");
    // The files still carry theirs — that is where the numbers were always readable.
    let file = row("app.rs");
    assert!(file.contains("+9") && file.contains('\u{2212}'), "file counts stay: {file}");
}

/// The hint names keys, and never a click — without mouse capture the terminal keeps the pointer,
/// and a row that offered a click would be advertising a gesture that does nothing.
#[test]
fn the_change_hints_name_keys_rather_than_promising_a_click() {
    let (width, height) = (160, 40);
    let backend = test_backend(width, height);
    let mut app = create_app_with_config(Config { mouse: Some(false), ..Config::default() });
    app.update(Msg::TerminalResized { width, height });
    let mut snapshot = empty_workspace("changes-hint-no-mouse");
    snapshot.changes = WorkspaceChangesSummary {
        files: vec![FileChangeSummary {
            path: "src/lib.rs".into(),
            status: FileChangeStatus::Modified,
            additions: Some(2),
            deletions: Some(1),
            binary: false,
        }],
        total_files: 1,
        additions: Some(2),
        deletions: Some(1),
        truncated: false,
    };
    app.workspace = Some(snapshot);

    let content = buffer_to_string(&render_app(&mut app, &backend));
    assert!(content.contains("ctrl+p preview · ctrl+g review"), "the keys are named: {content}");
    assert!(!content.contains("Click"), "and no click is offered: {content}");
}

/// The review's open comments stay on the wire and off the rail.
///
/// The count is `Draft + Open + NeedsVerification` — one state the reader has already acted on and
/// two that want them back — and the rail could reach none of them: it has no click target, and
/// `Ctrl+G` captures a *new* review rather than resuming the pinned one. A row that names
/// unfinished business with no route to it is worse than silence. `ReviewSummary` stays on the
/// snapshot, which is a published protocol.
#[test]
fn the_dock_draws_no_review_section_for_a_snapshot_that_carries_open_comments() {
    let (width, height) = (160, 40);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.update(Msg::TerminalResized { width, height });
    let mut snapshot = empty_workspace("dock-review-gate");
    snapshot.review = Some(ReviewSummary {
        id: "review-1".into(),
        title: "Fix the stale reference".into(),
        source: "working tree".into(),
        unresolved_findings: 2,
        completed_at_ms: 2,
    });
    assert_eq!(
        snapshot.review.as_ref().map(|review| review.unresolved_findings),
        Some(2),
        "the snapshot still carries them"
    );
    app.workspace = Some(snapshot);

    let content = buffer_to_string(&render_app(&mut app, &backend));
    assert!(content.contains("PROJECT"), "the rail still renders: {content}");
    assert!(!content.contains("REVIEW"), "no review heading: {content}");
    assert!(!content.contains("open comment"), "no comment count: {content}");
    assert!(!content.contains("Fix the stale reference"), "no review title: {content}");
}

/// Clicking the dock's "Review changes" affordance opens Guided Review (plan B3 mouse hook): the
/// render publishes a wrapping-aware region and the click dispatches `UiAction::ReviewOpen`.
#[test]
fn clicking_the_dock_review_changes_affordance_opens_review() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let (width, height) = (160, 40);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.update(Msg::TerminalResized { width, height });
    let mut snapshot = empty_workspace("dock-review-click");
    snapshot.changes = WorkspaceChangesSummary {
        files: vec![FileChangeSummary {
            path: "src/lib.rs".into(),
            status: FileChangeStatus::Modified,
            additions: Some(2),
            deletions: Some(1),
            binary: false,
        }],
        total_files: 1,
        additions: Some(2),
        deletions: Some(1),
        truncated: false,
    };
    app.workspace = Some(snapshot);

    let buffer = render_app(&mut app, &backend);
    let row = (0..height)
        .find(|&y| row_text(&buffer, y, width).contains("ctrl+g review"))
        .expect("the review affordance renders");
    let hints = row_text(&buffer, row, width).trim_end().to_string();
    assert!(hints.trim_start().starts_with("ctrl+p preview · ctrl+g review"), "{hints}");

    // The row carries two affordances, so each has to answer for its own words.
    let click_at = |app: &mut _, column| {
        route_mouse(
            app,
            &MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column,
                row,
                modifiers: KeyModifiers::NONE,
            },
        )
    };
    let review_column =
        u16::try_from(hints.find("ctrl+g").expect("the review hint is on the row") + 2)
            .expect("a narrow rail");
    match click_at(&mut app, review_column) {
        RouteAction::Msg(message) => assert!(
            matches!(*message, Msg::DispatchUiAction(luxe_core::keymap::UiAction::ReviewOpen)),
            "clicking the review words dispatches ReviewOpen"
        ),
        other => panic!("expected a ReviewOpen dispatch from the click, got {other:?}"),
    }
    let peek_column =
        u16::try_from(hints.find("ctrl+p").expect("the preview hint is on the row") + 2)
            .expect("a narrow rail");
    match click_at(&mut app, peek_column) {
        RouteAction::Msg(message) => assert!(
            matches!(*message, Msg::DispatchUiAction(luxe_core::keymap::UiAction::ToggleChanges)),
            "clicking the preview hint opens the preview, not the review"
        ),
        other => panic!("expected a ToggleChanges dispatch from the click, got {other:?}"),
    }
}

/// Clicking the dock's model row opens the model picker on the models: in the wide layout the
/// statusline hides and MODEL telemetry relocates into the dock footer as a clickable row.
#[test]
fn clicking_the_dock_model_row_opens_the_model_picker() {
    use luxe_tui::model_picker::PickerStart;
    let message = click_dock_row("test-model-xyz");
    let opened = matches!(message, Msg::OpenModelPicker(PickerStart::Models));
    assert!(opened, "the model row opens the picker on the models, got {message:?}");
}

/// The provider row is its own target, opening on the providers rather than on the models.
#[test]
fn clicking_the_dock_provider_row_opens_the_providers() {
    use luxe_tui::model_picker::PickerStart;
    let message = click_dock_row("test-provider-abc");
    let opened = matches!(message, Msg::OpenModelPicker(PickerStart::Providers));
    assert!(opened, "the provider row opens the picker on the providers, got {message:?}");
}

/// An app wide enough to show the dock, on `test-model-xyz` from `test-provider-abc`.
fn app_on_test_model(width: u16, height: u16) -> App {
    let mut app = create_app();
    app.update(Msg::TerminalResized { width, height });
    app.workspace = Some(empty_workspace("dock-model-click"));
    app.status.model = "test-model-xyz".into();
    app.status.provider = "test-provider-abc".into();
    app.status.context_window = 128_000;
    app.status.context_pct = 10.0;
    app
}

/// The message a click on the dock row showing `text` sends.
fn click_dock_row(text: &str) -> Msg {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let (width, height) = (160, 40);
    let backend = test_backend(width, height);
    let mut app = app_on_test_model(width, height);
    let buffer = render_app(&mut app, &backend);
    let row = (0..height)
        .find(|&y| row_text(&buffer, y, width).contains(text))
        .unwrap_or_else(|| panic!("{text} renders in the dock"));
    let click = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 5,
        row,
        modifiers: KeyModifiers::NONE,
    };
    match route_mouse(&mut app, &click) {
        RouteAction::Msg(message) => *message,
        other => panic!("expected a message, got {other:?}"),
    }
}

/// The mouse wheel scrolls the informational dock when the cursor is over the panel (there is no
/// keyboard focus). A notch down advances the offset; the top clamps at zero.
#[test]
fn wheel_over_the_dock_scrolls_its_content() {
    // A short viewport so the dock's (bounded) sections overflow the middle and a real max exists.
    let (width, height) = (160, 12);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.update(Msg::TerminalResized { width, height });
    let mut snapshot = empty_workspace("dock-wheel-scroll");
    // Plan is a fixed-size section (heading + counts + one row per todo), so eight of them reliably
    // overflow a 12-row dock regardless of the height-scaled Changes cap.
    let items: Vec<TodoSummaryItem> = (0..8)
        .map(|n| TodoSummaryItem {
            id: n.to_string(),
            subject: format!("Task {n}"),
            status: TodoSummaryStatus::Pending,
            blocked_by: Vec::new(),
        })
        .collect();
    snapshot.todos =
        TodoSummary { pending: items.len(), total: items.len(), items, ..TodoSummary::default() };
    app.workspace = Some(snapshot);
    // Render once so the dock publishes its scroll region + max offset.
    let _ = render_app(&mut app, &backend);
    assert_eq!(app.dock.scroll, 0);

    // A wheel over the dock rail (column 5) is inside the panel on a wide layout.
    use crossterm::event::MouseEventKind;
    let wheel = |kind| crossterm::event::MouseEvent {
        kind,
        column: 5,
        row: 10,
        modifiers: KeyModifiers::NONE,
    };
    match route_mouse(&mut app, &wheel(MouseEventKind::ScrollDown)) {
        RouteAction::Msg(message) => app.update(*message),
        other => {
            panic!("expected a dock-scroll message from a wheel over the panel, got {other:?}")
        }
    }
    assert!(app.dock.scroll > 0, "a wheel notch over the dock advances its scroll");

    for _ in 0..5 {
        if let RouteAction::Msg(message) = route_mouse(&mut app, &wheel(MouseEventKind::ScrollUp)) {
            app.update(*message);
        }
    }
    assert_eq!(app.dock.scroll, 0, "scrolling back up clamps at the top");
}

#[test]
fn dock_position_right_places_the_full_height_sidebar_on_the_right() {
    let (width, height) = (160, 40);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.dock.position = luxe_core::config::DockPosition::Right;
    app.update(Msg::TerminalResized { width, height });
    app.workspace = Some(empty_workspace("dock-right"));

    let buffer = render_app(&mut app, &backend);
    // The right rail hugs the right edge: panel starts at width - dock, its separator precedes it,
    // and the heading sits flush at the 1-col content margin into the panel.
    let dock_start = width - luxe_tui::workspace_layout::CONTEXT_DOCK_WIDTH;
    let separator_x = dock_start - 1;
    let context_x =
        (0..width).find(|&x| buffer[(x, 0)].symbol() == "P").expect("dock heading should render");
    assert_eq!(
        context_x,
        dock_start + 1 + DOCK_HEADING_LEAD,
        "right dock heading badge opens on the 1-col content margin into the panel"
    );
    assert_eq!(
        buffer[(separator_x, 0)].symbol(),
        "│",
        "separator on the top row precedes the right panel"
    );
    assert_eq!(
        buffer[(separator_x, height - 1)].symbol(),
        "│",
        "right sidebar separator should reach the bottom row (full height)"
    );
}

#[test]
fn hidden_statusline_relocates_model_telemetry_into_the_dock() {
    let (width, height) = (160, 40);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.update(Msg::TerminalResized { width, height });
    app.workspace = Some(empty_workspace("telemetry"));
    app.status.model = "gpt-4o".into();
    app.status.context_window = 128_000;
    app.status.context_pct = 42.0;
    app.status.prompt_tokens = 1_200;
    app.status.completion_tokens = 340;
    app.status.session = luxe_core::usage::SessionUsage::single(
        luxe_core::usage::UsageSource::Agent,
        luxe_core::usage::UsageTotals {
            calls: 3,
            prompt_tokens: 12_400,
            completion_tokens: 3_800,
            cost_usd: 0.42,
            ..Default::default()
        },
    );

    let buffer = render_app(&mut app, &backend);
    let content = buffer_to_string(&buffer);
    // Wide + auto statusline => statusline hidden (the composer bottom-aligns), so telemetry
    // migrates into the dock instead of riding the bar.
    let composer_x = luxe_tui::workspace_layout::CONTEXT_DOCK_WIDTH
        + luxe_tui::workspace_layout::CONTEXT_DOCK_SEPARATOR_WIDTH
        + 1;
    assert_eq!(
        buffer[(composer_x, height - 1)].symbol(),
        "╰",
        "statusline should be hidden on wide: {content}"
    );
    // Two sections, because they answer two questions: what is answering, and what this session
    // has consumed — how full its window is, how many tokens went through it, what that cost.
    assert!(content.contains("MODEL"), "missing the model section: {content}");
    assert!(content.contains("gpt-4o"), "missing relocated model: {content}");
    assert!(content.contains("thinking"), "the model's reasoning level rides with it: {content}");
    assert!(content.contains("SESSION"), "missing the session section: {content}");
    assert!(content.contains("42% of 128k"), "missing relocated context usage: {content}");
    assert!(content.contains("$0.42"), "missing relocated session cost: {content}");
    assert!(!content.contains("CONTEXT"), "the meter reads under SESSION now: {content}");
    assert!(!content.contains("lsp "), "the constant lsp row is gone: {content}");
}

#[test]
fn forced_statusline_beside_the_sidebar_keeps_the_dock_model_section() {
    let (width, height) = (160, 40);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.update(Msg::TerminalResized { width, height });
    app.workspace = Some(empty_workspace("beside"));
    app.status.model = "gpt-4o".into();
    app.statusline_state.show();

    let buffer = render_app(&mut app, &backend);
    let content = buffer_to_string(&buffer);
    // The sidebar's MODEL footer is decoupled from the statusline: a forced-on statusline sits
    // beside the full-height sidebar and BOTH carry telemetry (the sidebar keeps its own footer
    // rather than emptying it). `auto` still yields exactly one copy by hiding the bar.
    assert!(
        row_text(&buffer, height - 1, width).contains("gpt-4o"),
        "forced statusline should show the model on its bottom row: {content}"
    );
    assert!(content.contains("PROJECT"), "sidebar should still render: {content}");
    assert!(
        content.contains("MODEL"),
        "sidebar should keep its MODEL section even with the statusline shown: {content}"
    );
    // The sidebar remains full-height even with the statusline in the main column.
    let sep_x = luxe_tui::workspace_layout::CONTEXT_DOCK_WIDTH;
    assert_eq!(
        buffer[(sep_x, height - 1)].symbol(),
        "│",
        "sidebar should still reach the bottom row"
    );
}

/// Diagnostics stay on the wire and off the rail.
///
/// The snapshot still carries them — `Event::WorkspaceUpdated` is a published protocol and an RPC
/// front-end may want them — but the dock draws no section for them. The counts only cover files
/// the agent edited this session with a warm language server attached, so a rail read as fact
/// cannot show them: `✗ 0` there reads as "the project is clean" and means "nothing found in what I
/// touched". The agent gets every diagnostic injected after the edit that caused it, which is where
/// it can still be acted on.
#[test]
fn the_dock_draws_no_diagnostics_section_for_a_snapshot_that_carries_them() {
    let (width, height) = (160, 40);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.update(Msg::TerminalResized { width, height });
    let mut snapshot = empty_workspace("diagnostics");
    snapshot.diagnostics = DiagnosticsSummary {
        errors: 3,
        warnings: 2,
        infos: 0,
        hints: 0,
        files: vec![
            FileDiagnostics {
                path: "src/main.rs".into(),
                errors: 3,
                warnings: 1,
                infos: 0,
                hints: 0,
            },
            FileDiagnostics {
                path: "src/lib.rs".into(),
                errors: 0,
                warnings: 1,
                infos: 0,
                hints: 0,
            },
        ],
        total_files: 5,
        truncated: true,
    };
    assert!(!snapshot.diagnostics.is_empty(), "the snapshot still carries them");
    app.workspace = Some(snapshot);

    let content = buffer_to_string(&render_app(&mut app, &backend));
    assert!(content.contains("PROJECT"), "the rail still renders: {content}");
    assert!(!content.contains("DIAGNOSTICS"), "no diagnostics heading: {content}");
    assert!(!content.contains("src/main.rs"), "no per-file diagnostic row: {content}");
    assert!(!content.contains("3✗"), "no per-file counts: {content}");
}

#[test]
fn idle_dock_omits_the_now_section() {
    let (width, height) = (160, 40);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.update(Msg::TerminalResized { width, height });
    // Empty snapshot: no attention, no telemetry, idle turn.
    app.workspace = Some(empty_workspace("idle"));

    let content = buffer_to_string(&render_app(&mut app, &backend));
    assert!(content.contains("PROJECT"), "identity should still render: {content}");
    // NOW is attention-only now: an idle dock shows no permanent "● Idle" row.
    assert!(!content.contains("NOW"), "idle dock must omit the NOW heading: {content}");
    assert!(!content.contains("● Idle"), "idle dock must omit the '● Idle' row: {content}");
}

#[test]
fn dock_footer_bottom_anchors_model_telemetry() {
    let (width, height) = (160, 40);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.update(Msg::TerminalResized { width, height });
    app.workspace = Some(empty_workspace("footer-telemetry"));
    app.status.model = "gpt-4o".into();
    app.status.context_window = 128_000;
    app.status.context_pct = 42.0;
    app.status.cost_usd = Some(0.42);

    let buffer = render_app(&mut app, &backend);
    let content = buffer_to_string(&buffer);
    assert!(content.contains("MODEL"), "footer should carry telemetry: {content}");
    let project_row = (0..height)
        .find(|&row| row_text(&buffer, row, width).contains("PROJECT"))
        .expect("PROJECT heading row");
    let model_row = (0..height)
        .find(|&row| row_text(&buffer, row, width).contains("MODEL"))
        .expect("MODEL heading row");
    // Identity stays at the top; telemetry sinks to the bottom near the composer.
    assert!(model_row > project_row, "telemetry should sit below identity: {content}");
    // Heading, provider, model, thinking and mode, then SESSION's heading, context and cost
    assert!(
        model_row >= height - 8,
        "telemetry should be bottom-anchored (row {model_row} of {height}): {content}"
    );
}

/// The CHANGES section colors each file's counts like its summary line and the Guided Review file
/// rows: additions in the success color, deletions in the error color (not a flat muted run).
#[test]
fn dock_changes_group_rows_color_the_add_remove_counts() {
    let (width, height) = (160, 40);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.update(Msg::TerminalResized { width, height });
    let mut snapshot = empty_workspace("changes-color");
    snapshot.changes = WorkspaceChangesSummary {
        files: vec![FileChangeSummary {
            path: "src/lib.rs".into(),
            status: FileChangeStatus::Modified,
            additions: Some(12),
            deletions: Some(3),
            binary: false,
        }],
        total_files: 1,
        additions: Some(12),
        deletions: Some(3),
        truncated: false,
    };
    app.workspace = Some(snapshot);

    let buffer = render_app(&mut app, &backend);
    // The file row carries the counts; the heading over a single file leaves them to it.
    let row = (0..height)
        .find(|&y| {
            let text = row_text(&buffer, y, width);
            text.contains("lib.rs") && text.contains("+12")
        })
        .expect("the changed file row renders");
    let plus_col = (0..width).find(|&x| buffer[(x, row)].symbol() == "+").expect("additions count");
    let minus_col =
        (plus_col + 1..width).find(|&x| buffer[(x, row)].symbol() == "−").expect("deletions count");
    assert_eq!(
        buffer[(plus_col, row)].fg,
        app.theme_styles.success.fg.expect("success fg"),
        "additions should use the success color"
    );
    assert_eq!(
        buffer[(minus_col, row)].fg,
        app.theme_styles.error.fg.expect("error fg"),
        "deletions should use the error color"
    );
}

/// The dock's changed files are a list you pick from, so the one under the pointer is cut from the
/// slab that means *chosen* everywhere else — tapering into the rail's own fill, not the color
/// behind it, and without moving the name off the column it shares with every other row.
#[test]
fn a_hovered_dock_file_row_is_cut_into_a_slab_on_the_rails_own_fill() {
    luxe_tui::glyphs::set_tier(luxe_tui::glyphs::GlyphTier::Nerd);
    let (width, height) = (160, 40);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.update(Msg::TerminalResized { width, height });
    // Every bundled theme paints the rail the same color as the app background, which would let a
    // cap cut against the wrong canvas pass unnoticed. A theme is free to raise it, so this one
    // does.
    const RAIL: ratatui::style::Color = ratatui::style::Color::Rgb(0x21, 0x1e, 0x1a);
    app.theme_styles.tool_bg = ratatui::style::Style::new().bg(RAIL);
    let mut snapshot = empty_workspace("changes-hover");
    let file = |path: &str| FileChangeSummary {
        path: path.into(),
        status: FileChangeStatus::Modified,
        additions: Some(4),
        deletions: Some(1),
        binary: false,
    };
    snapshot.changes = WorkspaceChangesSummary {
        files: vec![file("src/lib.rs"), file("src/main.rs")],
        total_files: 2,
        additions: Some(8),
        deletions: Some(2),
        truncated: false,
    };
    app.workspace = Some(snapshot);

    let plain = render_app(&mut app, &backend);
    app.pointer.hover = Some(luxe_tui::mouse::MouseTarget::DockFile("src/lib.rs".into()));
    let hovered = render_app(&mut app, &backend);

    let column = |buffer: &ratatui::buffer::Buffer, needle: &str| -> (u16, u16) {
        (0..height)
            .find_map(|y| {
                let text = row_text(buffer, y, width);
                text.find(needle)
                    .map(|byte| (unicode_width::UnicodeWidthStr::width(&text[..byte]) as u16, y))
            })
            .unwrap_or_else(|| panic!("the {needle} row renders"))
    };
    let (name_x, row) = column(&hovered, "lib.rs");
    assert_eq!(
        (name_x, row),
        column(&plain, "lib.rs"),
        "the indent a file row already spent is the slab's own frame, so nothing moves"
    );

    let line = row_text(&hovered, row, width);
    let cap = luxe_tui::widgets::Cap::Open.glyph();
    let cut = line.find(cap).expect("the hovered row opens on a cut");
    let cut_x = unicode_width::UnicodeWidthStr::width(&line[..cut]) as u16;
    assert!(cut_x < name_x, "and the cut opens before the name: {line:?}");

    // A cap is the slab's color over the canvas *behind* it. On a raised rail that canvas is the
    // rail's own fill; against the app background it would leave a notch at the diagonal.
    assert_eq!(
        hovered[(cut_x, row)].bg,
        RAIL,
        "the cut tapers into the rail, not into the color behind it"
    );
    assert_eq!(
        hovered[(cut_x, row)].fg,
        app.theme_styles.selected_bg.bg.expect("the theme defines a selection"),
    );
    // The row that is not hovered keeps the rail's fill and no cut at all.
    let (_, other) = column(&hovered, "main.rs");
    assert!(!row_text(&hovered, other, width).contains(cap), "only one row is chosen");
}
