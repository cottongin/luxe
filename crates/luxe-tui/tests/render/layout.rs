//! Layout — selection highlight, composer and statusline, and the dock beside or over the chat.

use super::*;

#[test]
fn selected_conversation_block_uses_visual_highlight_and_escape_clears_it() {
    let (w, h) = (80, 24);
    let backend = test_backend(w, h);
    let mut app = create_app();
    app.push_convo(ConvoBlock::User {
        text: "selected text".into(),
        attached: Vec::new(),
        chips: Vec::new(),
    });
    app.selected_block = Some(0);
    let selected_bg = app.theme_styles.selected_bg.bg.expect("selected background");

    let buffer = render_app(&mut app, &backend);
    let row = (0..h)
        .find(|&y| row_text(&buffer, y, w).contains("selected text"))
        .expect("selected message row");
    assert!((0..w).any(|x| buffer[(x, row)].bg == selected_bg));

    assert!(matches!(
        route_key(&mut app, &key(KeyCode::Esc)),
        RouteAction::Msg(message) if matches!(*message, Msg::Tick)
    ));
    assert!(app.selected_block.is_none());
}

#[test]
fn composer_and_statusline_are_adjacent_distinct_surfaces() {
    let (w, h) = (80, 24);
    let backend = test_backend(w, h);
    let mut app = create_app();
    app.status.model = "gpt-4o".into();
    app.status.git_branch = Some("main".into());
    app.status.context_pct = 42.0;
    app.status.context_window = 128_000;
    app.editor.insert_text("hello");
    let buffer = render_app(&mut app, &backend);
    let composer: String = (h - 5..h).map(|row| row_text(&buffer, row, w)).collect();

    assert!(!composer.contains("Status"));
    assert!(!composer.contains("Input"));
    assert!(composer.contains("│hello"), "input should start at the first inner cell");
    assert!(composer.contains("gpt-4o"));
    assert!(composer.contains("42.0%/128k"));
    // The bar is flat now: no editor mode label, no powerline arrow separators.
    assert!(!composer.contains("INSERT"), "mode label is gone: {composer:?}");
    assert!(!composer.contains('\u{e0b0}'), "no left powerline separator");
    assert!(!composer.contains('\u{e0b2}'), "no right powerline separator");
    assert!(row_text(&buffer, h - 2, w).trim_start().starts_with('╰'));
    let statusline = row_text(&buffer, h - 1, w);
    assert!(statusline.contains("gpt-4o"), "model anchors the statusline row: {statusline:?}");
    // Both draw flush to the terminal's edges, and line up with each other there
    let corner_left =
        (0..w).find(|&x| buffer[(x, h - 4)].symbol() == "╭").expect("composer top-left corner");
    let corner_right = (0..w)
        .rev()
        .find(|&x| buffer[(x, h - 4)].symbol() == "╮")
        .expect("composer top-right corner");
    // Painted, not inked: a band opens on a pad cell, which is blank but carries the slab's color.
    // The canvas is read off the empty transcript, since the bar now reaches column 0
    let canvas = buffer[(0, 0)].bg;
    let painted = |x: u16| buffer[(x, h - 1)].symbol() != " " || buffer[(x, h - 1)].bg != canvas;
    let statusline_left = (0..w).find(|&x| painted(x)).expect("statusline left content");
    let statusline_right = (0..w).rev().find(|&x| painted(x)).expect("statusline right content");
    assert_eq!(corner_left, statusline_left, "statusline aligns with the prompt box left edge");
    assert_eq!(corner_right, statusline_right, "statusline aligns with the prompt box right edge");
}

#[test]
fn standard_context_dock_overlays_the_configured_side_without_narrowing_the_footer() {
    let (width, height) = (120, 32);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.update(Msg::TerminalResized { width, height });
    app.dock.show();
    app.status.model = "gpt-4o".into();
    app.push_convo(ConvoBlock::Assistant("conversation remains primary".into()));

    let buffer = render_app(&mut app, &backend);
    let content = buffer_to_string(&buffer);
    assert!(content.contains("PROJECT"), "missing Context overlay: {content}");
    // Overlay is not side-by-side, so the statusline stays as the compact footer (auto). The
    // model name anchors the bar's bottom row (there is no editor-mode label anymore).
    assert!(
        row_text(&buffer, height - 1, width).contains("gpt-4o"),
        "footer should remain visible on the bottom row: {content}"
    );

    // Left is the default side, so the overlay hugs the left edge; the heading sits flush against
    // it, matching the full-height dock.
    let context_x =
        (0..width).find(|&x| buffer[(x, 0)].symbol() == "P").expect("dock heading should render");
    assert_eq!(
        context_x, DOCK_HEADING_LEAD,
        "overlay must occupy the leftmost dock cells by default"
    );
}

/// The overlay dock owns its cells. A `Block` only restyles what is already in the buffer, so the
/// conversation underneath used to show through wherever a dock row was shorter than the line it
/// covered — recolored as dock text, which reads as garbled dock content rather than as chat.
#[test]
fn the_overlay_dock_covers_the_conversation_underneath_it() {
    let (width, height) = (120, 32);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.update(Msg::TerminalResized { width, height });
    app.dock.show();
    // Long enough to run under the whole dock column on several rows.
    app.push_convo(ConvoBlock::Assistant("BLEED ".repeat(40)));
    app.workspace = Some(empty_workspace("dock-overlay-bleed"));

    let buffer = render_app(&mut app, &backend);
    let dock_width = luxe_tui::workspace_layout::CONTEXT_DOCK_WIDTH;
    for y in 0..height {
        let covered: String = (0..dock_width).map(|x| buffer[(x, y)].symbol()).collect();
        assert!(
            !covered.contains("BLEED"),
            "conversation text shows through the overlay on row {y}: {covered:?}"
        );
    }
}

#[test]
fn compact_context_dock_replaces_chat_and_hides_the_composer() {
    let (width, height) = (78, 30);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.update(Msg::TerminalResized { width, height });
    app.dock.show();
    app.status.model = "qwen3.8-27b".into();
    app.status.context_window = 128_000;
    app.status.context_pct = 42.0;
    app.editor.insert_text("composer must stay hidden");
    app.push_convo(ConvoBlock::Assistant("chat must stay hidden".into()));

    let buffer = render_app(&mut app, &backend);
    let content = buffer_to_string(&buffer);
    assert!(content.contains("PROJECT"), "missing replacement Context surface: {content}");
    assert!(content.contains("CONTEXT"), "Context footer should replace editor mode: {content}");
    assert!(
        content.contains("pgup/Dn scroll"),
        "footer should explain independent Context scrolling: {content}"
    );
    assert!(content.contains("esc"), "footer should explain how to return to chat: {content}");
    assert!(!content.contains("chat must stay hidden"), "chat leaked into replacement: {content}");
    assert!(
        !content.contains("composer must stay hidden"),
        "composer leaked into replacement: {content}"
    );

    let context_x =
        (0..width).find(|&x| buffer[(x, 0)].symbol() == "P").expect("dock heading should render");
    assert_eq!(
        context_x, DOCK_HEADING_LEAD,
        "the heading badge's leading edge sits on the terminal's edge"
    );
}

#[test]
fn wide_context_dock_is_a_full_height_left_panel_and_hides_the_statusline() {
    let (width, height) = (160, 40);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.update(Msg::TerminalResized { width, height });
    app.push_convo(ConvoBlock::Assistant("conversation remains primary".into()));

    let mut snapshot = WorkspaceSnapshot::empty(
        WorkspaceScopeId::new("dock-render"),
        ProjectIdentity {
            root: "/work/luxe".into(),
            display_name: "luxe".into(),
            display_path: "~/work/luxe".into(),
            git_branch: Some("feature/context-dock".into()),
            git_dirty: true,
            git_head: None,
        },
    );
    snapshot.session = Some(WorkspaceSessionSummary {
        id: "session-1".into(),
        title: "Render the Context Dock".into(),
        updated_at_ms: 1,
    });
    snapshot.todos = TodoSummary {
        current: Some(TodoSummaryItem {
            id: "2".into(),
            subject: "Render the Context Plan".into(),
            status: TodoSummaryStatus::InProgress,
            blocked_by: Vec::new(),
        }),
        items: vec![
            TodoSummaryItem {
                id: "2".into(),
                subject: "Render the Context Plan".into(),
                status: TodoSummaryStatus::InProgress,
                blocked_by: Vec::new(),
            },
            TodoSummaryItem {
                id: "3".into(),
                subject: "Capture responsive fixtures".into(),
                status: TodoSummaryStatus::Blocked,
                blocked_by: vec!["2".into()],
            },
            TodoSummaryItem {
                id: "4".into(),
                subject: "Write the dock tests".into(),
                status: TodoSummaryStatus::Pending,
                blocked_by: Vec::new(),
            },
            TodoSummaryItem {
                id: "1".into(),
                subject: "Model the sections".into(),
                status: TodoSummaryStatus::Completed,
                blocked_by: Vec::new(),
            },
        ],
        pending: 1,
        in_progress: 1,
        completed: 3,
        blocked: 1,
        total: 8,
        truncated: true,
    };
    snapshot.changes = WorkspaceChangesSummary {
        files: vec![
            FileChangeSummary {
                path: "crates/luxe-tui/src/view/dock.rs".into(),
                status: FileChangeStatus::Modified,
                additions: Some(12),
                deletions: Some(3),
                binary: false,
            },
            FileChangeSummary {
                path: "assets/context-preview.png".into(),
                status: FileChangeStatus::Added,
                additions: None,
                deletions: None,
                binary: true,
            },
        ],
        total_files: 4,
        additions: None,
        deletions: None,
        truncated: true,
    };
    snapshot.review = Some(ReviewSummary {
        id: "review-1".into(),
        title: "Fix the stale reference".into(),
        source: "last turn".into(),
        unresolved_findings: 1,
        completed_at_ms: 2,
    });
    snapshot.tooling.debug_sessions = vec![DebugSessionStatus {
        id: "dbg-1".into(),
        adapter: "codelldb".into(),
        program: "target/debug/app".into(),
        terminated: false,
        stopped: true,
        stop_reason: Some("breakpoint".into()),
    }];
    app.workspace = Some(snapshot);
    let buffer = render_app(&mut app, &backend);
    let content = buffer_to_string(&buffer);
    assert!(content.contains("conversation remains primary"), "missing transcript: {content}");
    assert!(content.contains("PROJECT"), "missing identity heading: {content}");
    // Identity is project + branch only: the auto-derived session title and the task slug were
    // dropped as chrome that restated what the user already knew.
    assert!(!content.contains("Session ·"), "session title row was dropped: {content}");
    assert!(!content.contains("Task ·"), "task slug row was dropped: {content}");
    // The badge holds the word and the ratio rides bare beside it, so the run of headings down the
    // rail stays one kind of object — hence the closing cap between the two.
    assert!(content.contains("PLAN"), "missing the Plan heading: {content}");
    assert!(content.contains("3/8"), "Plan heading carries the ratio: {content}");
    // Every todo is its own checklist row, in-progress first, with per-status glyphs.
    assert!(content.contains("◉ Render the Context Plan"), "in-progress row: {content}");
    // The blocked row is its mark and its subject: the ids it waits on are shown nowhere else
    assert!(content.contains("⊘ Capture responsive fixtu"), "blocked row: {content}");
    assert!(!content.contains("needs"), "no dependency note: {content}");
    assert!(content.contains("○ Write the dock tests"), "a pending row: {content}");
    assert!(content.contains("✓ Model the sections"), "a completed row: {content}");
    assert!(content.contains("├─") || content.contains("└─"), "tree connectors: {content}");
    // What a list is not showing is the bar's to say: the rail counts none of it in words.
    assert!(!content.contains("not listed"), "the rail counts nothing in words: {content}");
    assert!(!content.contains("more"), "and folds nothing into a tail: {content}");
    assert!(!content.contains("Open Plan"), "the dock must not advertise a phantom action");
    assert!(content.contains("FILES"), "missing the Files heading: {content}");
    // With no prior review the headline *is* the total, said once.
    assert!(content.contains("4 changed"), "missing the changes headline: {content}");
    assert!(!content.contains("in the worktree"), "the total is not said twice: {content}");
    assert!(
        content.contains("ctrl+g review"),
        "the Changes section names the way into review: {content}"
    );
    assert!(content.contains("ctrl+p preview"), "and the cheaper way in beside it: {content}");
    // The rail carries what the reader can act on. A review's open comments and a live debug
    // session are both true here and both stay off it: neither is reachable from the rail, and the
    // debugger is the agent's tool — `/debug` answers on demand, as `/lsp` does.
    assert!(!content.contains("REVIEW"), "no review section: {content}");
    assert!(!content.contains("open comment"), "and no comment count: {content}");
    assert!(!content.contains("DEBUG"), "no debug section: {content}");
    assert!(!content.contains("stopped: breakpoint"), "and no session state: {content}");
    assert!(
        !content.contains("LOOP"),
        "loop gate must not surface in the interactive dock: {content}"
    );
    assert!(!content.contains("Gate ·"), "gate verdict must not surface: {content}");
    // The dock has no selection highlight or collapse markers anymore.
    assert!(!content.contains("▶"), "informational dock shows no collapse markers: {content}");
    // The full-height sidebar carries context on wide terminals, so the statusline auto-hides and
    // the composer bottom-aligns to the final body row (no compact bar beneath it).
    let composer_x = luxe_tui::workspace_layout::CONTEXT_DOCK_WIDTH
        + luxe_tui::workspace_layout::CONTEXT_DOCK_SEPARATOR_WIDTH
        + 1;
    assert_eq!(
        buffer[(composer_x, height - 1)].symbol(),
        "╰",
        "statusline should be hidden beside the sidebar: {content}"
    );
    // The sidebar is a full-height LEFT column: its heading opens on the first row and the panel
    // still occupies the top and bottom rows of the viewport.
    let context_x =
        (0..width).find(|&x| buffer[(x, 0)].symbol() == "P").expect("dock heading should render");
    assert_eq!(
        context_x, DOCK_HEADING_LEAD,
        "left dock heading badge opens on the terminal's edge"
    );
    // The left rail is followed by its separator, and both span the whole viewport height:
    // the divider glyph is present on the very first and very last rows.
    let sep_x = usize::from(luxe_tui::workspace_layout::CONTEXT_DOCK_WIDTH);
    assert_eq!(buffer[(sep_x as u16, 0)].symbol(), "│", "separator missing on the top row");
    assert_eq!(
        buffer[(sep_x as u16, height - 1)].symbol(),
        "│",
        "separator should reach the bottom row (full-height sidebar)"
    );
}
