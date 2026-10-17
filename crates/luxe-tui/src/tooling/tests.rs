use super::*;
use luxe_core::{DebugAdapterStatus, LanguageServerStatus, McpServerStatus};

fn sample() -> ToolingSummary {
    ToolingSummary {
        servers: vec![
            LanguageServerStatus {
                language: "rust".into(),
                command: "rust-analyzer".into(),
                extensions: vec!["rs".into()],
                from_config: false,
                install_hint: Some("rustup component add rust-analyzer".into()),
                installed: true,
                running: true,
            },
            LanguageServerStatus {
                language: "ruby".into(),
                command: "ruby-lsp".into(),
                extensions: vec!["rb".into()],
                from_config: true,
                install_hint: Some("gem install ruby-lsp".into()),
                installed: false,
                running: false,
            },
        ],
        adapters: vec![DebugAdapterStatus {
            id: "codelldb".into(),
            command: "codelldb".into(),
            extensions: vec![],
            from_config: false,
            installed: false,
        }],
        debug_sessions: vec![DebugSessionStatus {
            id: "dbg-1".into(),
            adapter: "debugpy".into(),
            program: "/tmp/app.py".into(),
            terminated: false,
            stopped: true,
            stop_reason: Some("breakpoint".into()),
        }],
        mcp_servers: vec![
            McpServerStatus {
                id: "chrome".into(),
                transport: "stdio".into(),
                auth: "none".into(),
                tool_count: Some(26),
                enabled: true,
                ..Default::default()
            },
            McpServerStatus {
                id: "linear".into(),
                transport: "http".into(),
                auth: "oauth".into(),
                tool_count: None,
                enabled: true,
                ..Default::default()
            },
        ],
        lsp_config_path: Some("/home/x/.config/luxe/lsp-servers.json".into()),
        adapters_config_path: Some("/home/x/.config/luxe/debug-adapters.json".into()),
        tools: Vec::new(),
    }
}

fn with_tools() -> ToolingSummary {
    let mut summary = sample();
    summary.tools = vec![
        ToolStatus { name: "read".into(), availability: ToolAvailability::Active },
        ToolStatus { name: "screenshot".into(), availability: ToolAvailability::Deferred },
        ToolStatus {
            name: "web_fetch".into(),
            availability: ToolAvailability::Disabled { source: "config".into() },
        },
        ToolStatus {
            name: "web_search".into(),
            availability: ToolAvailability::Disabled { source: "--no-tool".into() },
        },
    ];
    summary
}

/// The whole reason `/tools` exists: "gone" and "one activation away" must not read the same.
#[test]
fn the_tools_view_separates_deferred_from_disabled() {
    let out = render_tools(&with_tools());
    assert!(out.contains("In every request (1)"), "{out}");
    assert!(out.contains("Available on demand (1)"), "{out}");
    assert!(out.contains("activate_tools"), "says how a deferred tool becomes callable: {out}");
    assert!(out.contains("Disabled (2)"), "{out}");
}

/// An absence has to name what caused it, or the reader goes looking in the wrong file.
#[test]
fn a_disabled_tool_names_what_disabled_it() {
    let out = render_tools(&with_tools());
    assert!(out.contains("web_fetch · by config"), "{out}");
    assert!(out.contains("web_search · by --no-tool"), "{out}");
}

/// Before the registry is assembled there is nothing true to say, so say that rather than
/// rendering an empty list that reads as "the agent has no tools".
#[test]
fn the_tools_view_admits_when_it_is_not_ready() {
    let out = render_tools(&sample());
    assert!(out.contains("not ready yet"), "{out}");
    assert!(!out.contains("Disabled"), "no empty sections: {out}");
}

#[test]
fn lsp_view_marks_state_and_hint() {
    let out = render_lsp(&sample());
    assert!(out.contains("✓ rust"), "installed+running server: {out}");
    assert!(out.contains("· running"), "running marker present");
    assert!(out.contains("✗ ruby"), "missing server flagged");
    assert!(out.contains("not installed — gem install ruby-lsp"));
    assert!(out.contains("· config"), "config source labeled");
    assert!(out.contains("lsp-servers.json"));
}

#[test]
fn debug_view_lists_adapters_sessions_and_routing() {
    let out = render_debug(&sample());
    assert!(out.contains("✗ codelldb"));
    assert!(out.contains("compiled binaries"), "extensionless adapter routing: {out}");
    assert!(out.contains("Active debug sessions:"));
    assert!(out.contains("dbg-1 · debugpy · /tmp/app.py · live"));
    assert!(out.contains("debug-adapters.json"));
}

#[test]
fn debug_view_reports_no_sessions() {
    let mut summary = sample();
    summary.debug_sessions.clear();
    let out = render_debug(&summary);
    assert!(out.contains("Active debug sessions:\n  (none)"));
}

#[test]
fn mcp_view_lists_servers_with_transport_auth_and_tool_counts() {
    let out = render_mcp(&sample());
    assert!(out.contains("chrome · stdio · auth=none · 26 tools"), "{out}");
    assert!(out.contains("linear · http · auth=oauth · not yet discovered"), "{out}");
    assert!(out.contains("luxe mcp login"));
}

#[test]
fn mcp_view_reports_none_configured() {
    let mut summary = sample();
    summary.mcp_servers.clear();
    assert!(render_mcp(&summary).contains("(none configured)"));
}

/// The whole job of this view: say what is happening, why, and what to do about it. A reader
/// who sees a line where they expected a picture has exactly one question.
#[test]
fn the_image_view_gives_the_state_the_reason_and_the_lever() {
    use crate::graphics::ImageTier;

    let on = render_images(ImageTier::Pixels, "Ghostty draws images inline", 24);
    assert!(on.contains("on — Ghostty draws images inline"), "{on}");
    assert!(on.contains("24 rows"), "and the size it will take: {on}");

    let off =
        render_images(ImageTier::Text, "this terminal is not known to draw images inline", 24);
    assert!(off.contains("off — this terminal is not known"), "{off}");
    assert!(off.contains("click to open"), "and what happens instead: {off}");

    for view in [on, off, render_images(ImageTier::Blocks, "set to `blocks` in config", 12)] {
        assert!(view.contains("\"images\""), "every state names the lever: {view}");
        assert!(view.contains("kitty") && view.contains("Ghostty"), "{view}");
    }
}
