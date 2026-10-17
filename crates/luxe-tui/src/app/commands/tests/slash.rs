//! Slash — the commands that flip a switch, and the config they re-read.

use super::*;

#[test]
fn mouse_toggle_action_and_slash_command_flip_capture_state() {
    let mut app = seeded_app("nord", FixtureScenario::ChatIdle).expect("chat fixture");
    assert!(app.mouse_enabled, "mouse defaults on");

    assert!(dispatch_action(&mut app, UiAction::ToggleMouse));
    assert!(!app.mouse_enabled, "ToggleMouse turns it off");

    let registry = crate::commands::CommandRegistry::all();
    run(&mut app, &registry.parse_slash("/mouse on").expect("/mouse on parses"));
    assert!(app.mouse_enabled, "/mouse on enables");
    run(&mut app, &registry.parse_slash("/mouse off").expect("/mouse off parses"));
    assert!(!app.mouse_enabled, "/mouse off disables");
    run(&mut app, &registry.parse_slash("/mouse").expect("bare /mouse parses"));
    assert!(app.mouse_enabled, "bare /mouse toggles back on");
}

#[test]
fn permissions_slash_toggles_auto_approve() {
    let (mut app, _engine) = seeded_session("nord", FixtureScenario::ChatIdle).expect("fixture");
    assert!(!app.permission_auto_approve, "auto-approve defaults off in the TUI");

    let registry = crate::commands::CommandRegistry::all();
    run(&mut app, &registry.parse_slash("/permissions on").expect("parses"));
    assert!(app.permission_auto_approve, "/permissions on enables auto-approve");
    run(&mut app, &registry.parse_slash("/permissions off").expect("parses"));
    assert!(!app.permission_auto_approve, "/permissions off disables");
    run(&mut app, &registry.parse_slash("/permissions").expect("bare parses"));
    assert!(app.permission_auto_approve, "bare /permissions toggles back on");
}

#[test]
fn lsp_and_debug_slash_render_the_tooling_summary() {
    use luxe_core::{
        DebugAdapterStatus, LanguageServerStatus, ProjectIdentity, ToolingSummary,
        WorkspaceScopeId, WorkspaceSnapshot,
    };
    let (tx, _rx) = mpsc::channel(1);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    let mut snapshot = WorkspaceSnapshot::empty(
        WorkspaceScopeId::new("scope"),
        ProjectIdentity::non_git("/project".into(), "project", "/project"),
    );
    snapshot.tooling = ToolingSummary {
        servers: vec![LanguageServerStatus {
            language: "rust".into(),
            command: "rust-analyzer".into(),
            extensions: vec!["rs".into()],
            from_config: false,
            install_hint: Some("rustup component add rust-analyzer".into()),
            installed: true,
            running: true,
        }],
        adapters: vec![DebugAdapterStatus {
            id: "debugpy".into(),
            command: "python3 -m debugpy.adapter".into(),
            extensions: vec!["py".into()],
            from_config: false,
            installed: true,
        }],
        debug_sessions: Vec::new(),
        mcp_servers: vec![luxe_core::McpServerStatus {
            id: "demo".into(),
            transport: "stdio".into(),
            auth: "none".into(),
            tool_count: Some(3),
            enabled: true,
            ..Default::default()
        }],
        lsp_config_path: None,
        adapters_config_path: None,
        tools: Vec::new(),
    };
    app.workspace = Some(snapshot);

    let reg = crate::commands::CommandRegistry::all();
    run(&mut app, &reg.parse_slash("/lsp").expect("/lsp parses"));
    assert!(matches!(
        app.convo.last().map(|item| item.block()),
        Some(ConvoBlock::System(text)) if text.contains("rust") && text.contains("running")
    ));

    run(&mut app, &reg.parse_slash("/debug").expect("/debug parses"));
    assert!(matches!(
        app.convo.last().map(|item| item.block()),
        Some(ConvoBlock::System(text)) if text.contains("debugpy")
    ));

    run(&mut app, &reg.parse_slash("/mcp").expect("/mcp parses"));
    assert!(matches!(
        app.convo.last().map(|item| item.block()),
        Some(ConvoBlock::System(text)) if text.contains("demo") && text.contains("stdio")
    ));
}

#[test]
fn lsp_slash_is_graceful_before_the_first_snapshot() {
    let (tx, _rx) = mpsc::channel(1);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    let reg = crate::commands::CommandRegistry::all();
    run(&mut app, &reg.parse_slash("/lsp").expect("/lsp parses"));
    assert!(matches!(
        app.convo.last().map(|item| item.block()),
        Some(ConvoBlock::System(text)) if text.contains("isn't ready yet")
    ));
}

#[test]
fn palette_slash_backed_action_preserves_a_half_typed_prompt() {
    // Picking "Search conversation" from the palette while a draft is in the
    // composer must NOT clear it — the slash-backed action dispatches through the
    // editor-preserving action path, not the old composer-clearing slash handler.
    let (tx, _rx) = mpsc::channel(1);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    app.editor.insert_text("draft prompt");
    let search = crate::commands::CommandRegistry::all()
        .commands
        .into_iter()
        .find(|command| command.id == "tui.convo.search")
        .expect("search should be a palette command");

    dispatch(&mut app, search);

    assert!(app.search_state.is_active());
    assert_eq!(app.editor.current_input(), "draft prompt");
}

#[test]
fn new_session_slash_asks_the_engine_for_a_fresh_session() {
    // `/new` must dispatch `Action::NewSession` (not just clear the transcript locally) so the
    // engine switches sessions and the sidebar stops showing the prior session's title.
    let (tx, mut rx) = mpsc::channel(4);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    app.session_id = Some("prior-session".into());
    app.push_convo(ConvoBlock::User { text: "hi".into(), attached: vec![], chips: Vec::new() });
    let command = crate::commands::CommandRegistry::all().parse_slash("/new").expect("/new parses");

    run(&mut app, &command);

    assert!(app.convo.is_empty(), "/new should clear the transcript");
    assert!(app.session_id.is_none(), "/new should drop the active session id");
    assert!(matches!(rx.try_recv(), Ok(Action::NewSession)), "/new must dispatch NewSession");
}

#[test]
fn quit_works_from_the_palette_and_from_a_typed_slash() {
    let registry = crate::commands::CommandRegistry::all();

    // Palette selection of the "Quit" row.
    let (tx, _rx) = mpsc::channel(1);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    let quit = registry
        .commands
        .iter()
        .find(|command| command.id == "tui.app.quit")
        .expect("quit should be a palette command")
        .clone();
    dispatch(&mut app, quit);
    assert!(app.should_quit, "palette Quit should quit");

    // Typed `/quit` resolves to the same action.
    let (tx, _rx) = mpsc::channel(1);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    let typed = registry.parse_slash("/quit").expect("/quit should parse");
    run(&mut app, &typed);
    assert!(app.should_quit, "/quit should quit");
}

#[test]
fn dock_slash_forms_dispatch_visibility_actions() {
    use crate::workspace_dock::DockRuntimeOverride;

    let (tx, _rx) = mpsc::channel(1);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    let registry = crate::commands::CommandRegistry::all();

    for (input, expected) in [
        ("/dock", DockRuntimeOverride::Shown),
        ("/dock hide", DockRuntimeOverride::Hidden),
        ("/dock show", DockRuntimeOverride::Shown),
        ("/dock auto", DockRuntimeOverride::None),
    ] {
        let command = registry.parse_slash(input).expect("dock form should parse");
        run(&mut app, &command);
        assert_eq!(app.dock.runtime_override, expected, "{input}");
    }
}

#[test]
fn dock_position_action_and_slash_forms_move_the_sidebar() {
    use luxe_core::config::DockPosition;

    let (tx, _rx) = mpsc::channel(1);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    let registry = crate::commands::CommandRegistry::all();
    assert_eq!(app.dock.position, DockPosition::Left);

    assert!(dispatch_action(&mut app, UiAction::ToggleDockPosition));
    assert_eq!(app.dock.position, DockPosition::Right);

    run(&mut app, &registry.parse_slash("/dock left").expect("/dock left parses"));
    assert_eq!(app.dock.position, DockPosition::Left);
    run(&mut app, &registry.parse_slash("/dock right").expect("/dock right parses"));
    assert_eq!(app.dock.position, DockPosition::Right);
}

#[test]
fn statusline_visibility_actions_and_slash_forms_toggle_the_override() {
    use crate::statusline_state::StatuslineOverride;

    let (tx, _rx) = mpsc::channel(1);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    app.workspace_layout = crate::workspace_layout::WorkspaceLayout::Wide;
    let registry = crate::commands::CommandRegistry::all();

    // Auto would hide beside the side-by-side dock, so a bare toggle forces it shown.
    app.dock.show();
    assert!(dispatch_action(&mut app, UiAction::ToggleStatusline));
    assert_eq!(app.statusline_state.runtime_override, StatuslineOverride::Shown);

    for (input, expected) in [
        ("/statusline hide", StatuslineOverride::Hidden),
        ("/statusline show", StatuslineOverride::Shown),
        ("/statusline auto", StatuslineOverride::None),
    ] {
        let command = registry.parse_slash(input).expect("statusline form should parse");
        run(&mut app, &command);
        assert_eq!(app.statusline_state.runtime_override, expected, "{input}");
    }
}

#[test]
fn reload_config_refreshes_conversation_preferences() {
    let repo = tempfile::tempdir().unwrap();
    let luxe = repo.path().join(".luxe");
    std::fs::create_dir(&luxe).unwrap();
    std::fs::write(
            luxe.join("config.json"),
            r#"{"conversation":{"prose_max_width":72,"wide_block_max_width":120,"tool_details":"all"},"dock":{"mode":"shown"}}"#,
        )
        .unwrap();
    let (tx, _rx) = mpsc::channel(1);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    app.dock.hide();

    reload_config(&mut app, repo.path());

    assert_eq!(app.config.conversation.prose_max_width(), 72);
    assert_eq!(app.config.conversation.wide_block_max_width(), 120);
    assert_eq!(
        app.config.conversation.tool_details(),
        luxe_core::config::ToolDetailsPreference::All
    );
    assert_eq!(app.dock.configured, luxe_core::config::DockMode::Shown);
    assert_eq!(app.dock.runtime_override, crate::workspace_dock::DockRuntimeOverride::Hidden);
}

#[test]
fn reload_config_refreshes_dock_position_and_statusline_visibility() {
    use luxe_core::config::{DockPosition, StatuslineVisibility};

    let repo = tempfile::tempdir().unwrap();
    let luxe = repo.path().join(".luxe");
    std::fs::create_dir(&luxe).unwrap();
    std::fs::write(
        luxe.join("config.json"),
        r#"{"dock":{"position":"right"},"statusline":{"visibility":"hidden"}}"#,
    )
    .unwrap();
    let (tx, _rx) = mpsc::channel(1);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    assert_eq!(app.dock.position, DockPosition::Left);
    assert_eq!(app.statusline_state.configured, StatuslineVisibility::Auto);

    reload_config(&mut app, repo.path());

    assert_eq!(app.dock.position, DockPosition::Right);
    assert_eq!(app.statusline_state.configured, StatuslineVisibility::Hidden);
}
