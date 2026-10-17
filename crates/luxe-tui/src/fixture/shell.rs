//! Shell fixtures — the surfaces around the conversation.
//!
//! The startup dashboard, the Configuration surface, the session picker, and the Context Dock at
//! its three widths.

use super::*;

/// Seeds `app` for a shell scenario, reporting whether this family owns it.
///
/// `Ok(false)` means another family does — the caller tries the next one. An `Err` is a fixture
/// that claimed the scenario and then could not build it, which is a bug in the fixture.
pub(super) fn seed(app: &mut App, scenario: FixtureScenario) -> Result<bool, String> {
    match scenario {
        FixtureScenario::Dashboard
        | FixtureScenario::DashboardTyping
        | FixtureScenario::DashboardMinimal => seed_dashboard(app, scenario),
        FixtureScenario::DashboardContextFiles => seed_context_files(app)?,
        FixtureScenario::Configuration => seed_configuration(app),
        FixtureScenario::Setup | FixtureScenario::SetupTools => seed_setup(app, scenario),
        FixtureScenario::Settings | FixtureScenario::SettingsTools => seed_settings(app, scenario),
        FixtureScenario::SessionPicker => seed_sessions(app),
        FixtureScenario::DockWide
        | FixtureScenario::DockOverlay
        | FixtureScenario::DockCompact
        | FixtureScenario::DockPullRequest => seed_dock(app, scenario),
        FixtureScenario::ChangesWide
        | FixtureScenario::ChangesNarrow
        | FixtureScenario::ChangesFailed
        | FixtureScenario::ChangesExternal => seed_changes_pane(app, scenario),
        _ => return Ok(false),
    }
    Ok(true)
}

/// The `Dashboard`, `DashboardTyping` and `DashboardMinimal` scenarios.
fn seed_dashboard(app: &mut App, scenario: FixtureScenario) {
    let recent = |id: &str, name: &str, age: &str| SessionSummary {
        id: id.into(),
        name: name.into(),
        age: age.into(),
        search_text: name.into(),
        preview: Vec::new(),
        worktree: None,
    };
    // A static render can't show motion, so the shine's frame is nameable: point
    // `LUXE_FIXTURE_BANNER_TICK` at any tick of the pass to capture that instant.
    let tick = std::env::var("LUXE_FIXTURE_BANNER_TICK")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    app.status.thinking = luxe_core::provider::ThinkingLevel::High;
    // The bar the session will have, which the start screen shows under its prompt
    app.status.model = "qwen3.8-27b".into();
    app.status.provider = "lmstudio".into();
    app.status.context_window = 200_000;
    app.status.project = Some("luxe".into());
    app.status.git_branch = Some("feature/context-dock".into());
    app.status.git_dirty = true;
    app.workspace = Some(dock_workspace_snapshot());
    app.enter_dashboard(
        DashboardState::new(vec![
            recent("fixture-session", "Refine the workspace overview", "12m ago"),
            recent("fixture-audit", "Audit the current project structure", "1d ago"),
            recent("fixture-review", "Please explain why this provider stalls", "2d ago"),
            recent("fixture-alpha", "Trace the stalled provider handshake", "2d ago"),
        ])
        .with_resume(Some(crate::dashboard::ResumeTarget {
            id: "fixture-session".into(),
            title: "Refine the workspace overview".into(),
            age: "12m ago".into(),
        }))
        .with_briefing(crate::dashboard::DashBriefing {
            changed_files: 12,
            added: 640,
            removed: 118,
        }),
    );
    if let Some(dashboard) = app.dashboard.as_mut() {
        // `enter_dashboard` takes the reader's own style; the fixture names the one it shows
        dashboard.set_style(match scenario {
            FixtureScenario::DashboardMinimal => luxe_core::config::DashboardStyle::Minimal,
            _ => luxe_core::config::DashboardStyle::Launcher,
        });
    }
    if scenario == FixtureScenario::DashboardTyping {
        app.editor.insert_text("provider");
        if let Some(dashboard) = app.dashboard.as_mut() {
            dashboard.sync(&app.editor);
        }
    }
    app.banner = app.banner.with_tick(tick);
}

/// The context-file list, open over the dashboard.
fn seed_context_files(app: &mut App) -> Result<(), String> {
    // The dashboard behind it, then the list over the top — the surface never renders
    // alone, and the prompt it was searched from should still be readable around it.
    seed(app, FixtureScenario::Dashboard)?;
    app.overlays.context_files =
        Some(crate::context_files::ContextFilesState::from_rows(context_rows()));
    Ok(())
}

/// The Configuration surface, as a machine with nothing configured opens it.
fn seed_configuration(app: &mut App) {
    // Seeded through the production path, so a capture shows what a machine with no
    // credential really opens — including where this build looks for each file.
    app.auth_service = Some(std::sync::Arc::new(UnconfiguredAuth));
    // The step after a login that suggested no model, which the surface hosts in the same
    // region: without a catalog it would be unreachable here.
    app.model_catalog = configurable_models();
    app.open_configuration();
}

/// The `Setup` and `SetupTools` scenarios.
fn seed_setup(app: &mut App, scenario: FixtureScenario) {
    // Built from the shipped themes and no desktop, rather than this machine's, so the
    // capture is the same everywhere
    app.auth_service = Some(std::sync::Arc::new(UnconfiguredAuth));
    app.open_configuration();
    let setup = crate::configuration::SetupState::open(
        crate::configuration::SetupMode::FirstRun,
        &luxe_core::config::Config::default(),
        luxe_core::theme::builtin_theme_names(),
        None,
        "luxe".into(),
    );
    if let Some(configuration) = app.overlays.configuration.as_mut() {
        configuration.show_setup(setup);
    }
    if scenario == FixtureScenario::SetupTools {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        let keys = [
            KeyCode::Enter,
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::Down,
            KeyCode::Down,
            KeyCode::Enter,
            KeyCode::Down,
            KeyCode::Char(' '),
            KeyCode::Down,
        ];
        for code in keys {
            if let Some(configuration) = app.overlays.configuration.as_mut() {
                configuration.on_key(&key(code));
            }
        }
    }
}

/// The `Settings` and `SettingsTools` scenarios.
fn seed_settings(app: &mut App, scenario: FixtureScenario) {
    // Seeded from fixed documents rather than this machine's files, so a capture is the
    // same everywhere: a reader who chose a theme and turned the mouse off, in a project
    // that turns `debug` off for everyone who opens it.
    app.auth_service = Some(std::sync::Arc::new(UnconfiguredAuth));
    app.open_configuration();
    let layers = luxe_core::config::ConfigLayers {
        global: Some(serde_json::json!({
            "theme": "luxe",
            "mouse": false,
            "conversation": { "growth": "top" },
            "tools": { "disabled": ["screenshot"] }
        })),
        project: Some(serde_json::json!({ "tools": { "disabled": ["debug"] } })),
    };
    let page = match scenario {
        FixtureScenario::SettingsTools => luxe_core::settings::Page::Tools,
        _ => luxe_core::settings::Page::Appearance,
    };
    let settings = crate::configuration::SettingsState::open(
        layers,
        "luxe".into(),
        // Under whoever's home this runs in, so it reads `~/.config/…` on every machine
        luxe_core::user_dirs::home_dir()
            .unwrap_or_default()
            .join(".config")
            .join("luxe")
            .join("config.json"),
    )
    .open_on(page);
    if let Some(configuration) = app.overlays.configuration.as_mut() {
        configuration.show_settings(settings);
    }
}

/// Find-session picker over the dashboard with a deterministic session catalog.
fn seed_sessions(app: &mut App) {
    let session = |name: &str, age: &str, transcript: &str| SessionEntry {
        id: name.into(),
        name: name.into(),
        age: age.into(),
        search_text: transcript.into(),
        preview: Vec::new(),
    };
    let turn = |role: PreviewRole, text: &str| PreviewTurn { role, text: text.into() };
    // The first (selected) row carries a rich, role-tagged preview so the pane shows off
    // markdown rendering and user/assistant/tool separation for screenshot review.
    let feature_session = SessionEntry {
        id: "todo-overdue".into(),
        name: "Add a todo overdue command that lists open tasks past their due date".into(),
        age: "2h ago".into(),
        search_text: "add a todo overdue command that lists open tasks past their due date, \
                      oldest first, with a test"
            .into(),
        preview: vec![
            turn(
                PreviewRole::User,
                "add a `todo overdue` command that lists open tasks past their due date, oldest \
                 first, with a test",
            ),
            turn(PreviewRole::Assistant, "Reading the command table and the filters first."),
            turn(PreviewRole::Tool, "read(path: \"src/cli.js\")"),
            turn(
                PreviewRole::Assistant,
                "Added `todo overdue`. It lists open tasks past their due date, **oldest \
                 first**.\n\n- `src/filter.js`: a new `overdue` filter\n- `src/cli.js`: the \
                 command\n\nTwo new tests cover it, and all five pass.",
            ),
        ],
    };
    let sessions = vec![
        feature_session,
        session(
            "Why does todo import drop tasks with an empty title?",
            "5h ago",
            "Why does todo import drop tasks with an empty title? The CSV has 40 rows and only 37 \
             arrive.",
        ),
        session(
            "Fix the flaky reminder test on CI",
            "yesterday",
            "Fix the flaky reminder test on CI. It fails about one run in ten with a timezone \
             off by an hour.",
        ),
        session(
            "Review #142: store due dates as UTC",
            "yesterday",
            "Review pull request 142, which stores due dates as UTC.",
        ),
        session(
            "Plan the move from JSON storage to SQLite",
            "3d ago",
            "Plan the move from JSON storage to SQLite, keeping old files readable.",
        ),
        session(
            "Add shell completions for bash and zsh",
            "4d ago",
            "Add shell completions for bash and zsh, generated from the command table.",
        ),
        session(
            "Bump the minimum Python to 3.11",
            "1w ago",
            "Bump the minimum Python to 3.11 and drop the tomli fallback.",
        ),
    ];
    app.status.model = "claude-sonnet-5".into();
    app.status.provider = "anthropic".into();
    app.enter_dashboard(DashboardState::new(Vec::new()));
    app.overlays.session_picker =
        Some(SessionPickerState::for_kind(sessions, PickerKind::FindSession));
    app.current_view = ViewMode::SessionPicker;
}

/// The `DockWide`, `DockOverlay`, `DockCompact` and `DockPullRequest` scenarios.
fn seed_dock(app: &mut App, scenario: FixtureScenario) {
    app.push_convo(ConvoBlock::Assistant(
        "Context stays available without displacing the active conversation.".into(),
    ));
    app.status.model = "qwen3.8-27b".into();
    app.status.provider = "lmstudio".into();
    app.status.context_window = 200_000;
    app.status.context_pct = 12.4;
    app.status.prompt_tokens = 1_240;
    app.status.completion_tokens = 380;
    // Session totals are what the dock reports; the per-turn figures reset every turn.
    app.status.session = dock_session_usage();
    app.status.thinking = luxe_core::provider::ThinkingLevel::Medium;
    app.workspace = Some(dock_workspace_snapshot());
    // One place under ELSEWHERE already looked at, so the rail shows both states it has:
    // a place still asking for attention, and one folded down to the heading because
    // everything under it has been read.
    app.external_seen.insert("/home/dev/.config/luxe/models.json".into(), (4, 1));
    app.dock.show();
    if scenario == FixtureScenario::DockPullRequest {
        if let Some(workspace) = app.workspace.as_mut() {
            workspace.pull_request = luxe_core::workspace::PullRequestPresence::Found(
                luxe_core::github::PullRequestStatus {
                    number: 142,
                    title: "Draw the context dock beside the conversation".into(),
                    url: "https://github.com/acme/luxe/pull/142".into(),
                    state: luxe_core::github::PullRequestState::Open,
                    draft: false,
                    review_decision: Some(luxe_core::github::ReviewDecision::ChangesRequested),
                    checks: luxe_core::github::CheckRollup {
                        passed: 4,
                        failed: 1,
                        pending: 0,
                        skipped: 0,
                    },
                    unresolved_threads: 2,
                    head_oid: "3992868".into(),
                    stack: None,
                },
            );
        }
    }
}

/// The `ChangesWide`, `ChangesNarrow`, `ChangesFailed` and `ChangesExternal` scenarios.
fn seed_changes_pane(app: &mut App, scenario: FixtureScenario) {
    // A real exchange, because on a wide terminal the pane keeps the conversation beside
    // the diff and a one-line transcript would not show what that is worth.
    app.push_convo(ConvoBlock::User {
        text: "Why does the dock's change list show directories instead of files?".into(),
        attached: Vec::new(),
        chips: Vec::new(),
    });
    app.push_convo(ConvoBlock::Assistant(
        "The data was always there \u{2014} `DockChanges.files` carries a path, a status and \
         both counts per file. Only the renderer withheld them, reducing every file to its \
         directory before drawing."
            .into(),
    ));
    app.push_convo(ConvoBlock::Assistant(
        "I put the names back under their directory headings, middle-eliding the long ones \
         so the basename always survives."
            .into(),
    ));
    if matches!(scenario, FixtureScenario::ChangesWide | FixtureScenario::ChangesNarrow) {
        // Still at it: the point of peek is reading the diff while the agent keeps going,
        // so the turn is live, with its tests running and the clock pinned mid-turn
        app.push_convo(ConvoBlock::Assistant(
            "Now the dock's tests, to check nothing else leaned on the old grouping.".into(),
        ));
        app.push_convo(ConvoBlock::Tool(Box::new(ToolItem {
            activity: luxe_core::ToolActivity::running(
                "fixture-session",
                "call-test",
                "test",
                "cargo nextest run -p luxe-tui dock",
                1,
            ),
            expanded: false,
            details_toggled: false,
            waited_on_user_ms: 0,
        })));
        let now = std::time::Instant::now();
        let ago = |seconds| now.checked_sub(std::time::Duration::from_secs(seconds));
        app.status.frame_clock = Some(now);
        app.status.state = crate::app::AppState::Streaming;
        app.status.active_tool = Some("test".into());
        app.status.turn_started = ago(38);
        app.status.tool_started = ago(6);
        app.status.spinner_frame = 30;
    }
    app.status.model = "qwen3.8-27b".into();
    app.status.provider = "lmstudio".into();
    app.status.context_window = 200_000;
    app.status.context_pct = 12.4;
    app.status.session = dock_session_usage();
    app.status.thinking = luxe_core::provider::ThinkingLevel::Medium;
    app.workspace = Some(dock_workspace_snapshot());
    if scenario == FixtureScenario::ChangesWide {
        app.dock.show();
    } else {
        // The dock away, so the bar carries the change instead — and, for the failure,
        // so the message is not sitting under an overlay.
        app.dock.hide();
        app.statusline_state.show();
    }
    // A file the session changed somewhere else opens in the same pane, from the same
    // click, against whatever witness that place has — and without the review shortcut,
    // which is the one thing a review of *this* project could not honor.
    let path = if scenario == FixtureScenario::ChangesExternal {
        "/home/dev/.config/luxe/models.json"
    } else {
        "crates/luxe-tui/src/view/dock.rs"
    };
    app.update(Msg::OpenChanges(path.to_string()));
    let request = luxe_core::file_diff::FileDiffRequestId::new();
    if let Some(view) = app.changes.as_mut() {
        view.request = Some(request.clone());
    }
    // Through the production reducer, so the fixture exercises the same path a live
    // capture takes rather than assigning the state it is supposed to produce.
    app.update(Msg::Engine(if scenario == FixtureScenario::ChangesFailed {
        Event::FileDiffFailed {
            request_id: request,
            path: path.into(),
            error: luxe_core::file_diff::FileDiffFailure::TooLarge,
        }
    } else {
        Event::FileDiffLoaded {
            request_id: request,
            diff: Box::new(luxe_core::file_diff::FileDiff {
                path: path.into(),
                file: Some(Box::new(if scenario == FixtureScenario::ChangesExternal {
                    external_fixture_file(path)
                } else {
                    changes_fixture_file(path)
                })),
                untracked: false,
            }),
        }
    }));
}

/// The project context both dashboard scenarios draw from.
///
/// One list, because the section on the dashboard is the first five of exactly what the full list
/// shows — a fixture that seeded them separately could capture a count that disagrees with the rows
/// behind it, which is the bug the counts were replaced to stop having.
fn context_rows() -> Vec<crate::context_files::ContextFileRow> {
    use crate::context_files::{ContextFileRow, ContextKind};
    use luxe_core::instructions::ContextScope;
    let row = |kind, label: &str, scope: &str, path: &str| ContextFileRow {
        kind,
        label: label.into(),
        scope: scope.into(),
        path: path.into(),
        creates: false,
        reach: None,
        duplicate: false,
    };
    let instructions = |reach, label: &str, scope: &str, path: &str| ContextFileRow {
        reach: Some(reach),
        ..row(ContextKind::Instructions, label, scope, path)
    };
    vec![
        instructions(
            ContextScope::Global,
            "~/.config/luxe/agents.md",
            "every project",
            "/home/you/.config/luxe/agents.md",
        ),
        instructions(ContextScope::Project, "AGENTS.md", "this project", "/repo/AGENTS.md"),
        row(
            ContextKind::Skill,
            "reviewing-diffs",
            "How this project reads a change",
            "/repo/.luxe/skills/reviewing-diffs/SKILL.md",
        ),
        row(
            ContextKind::Skill,
            "release-notes",
            "Turning a milestone into prose",
            "/repo/.luxe/skills/release-notes/SKILL.md",
        ),
        row(
            ContextKind::Skill,
            "wire-audit",
            "Reading a capture of what reached the model",
            "/repo/.luxe/skills/wire-audit/SKILL.md",
        ),
        row(
            ContextKind::Skill,
            "release-checklist",
            "What ships before a tag does",
            "/repo/.luxe/skills/release-checklist/SKILL.md",
        ),
        row(ContextKind::McpServer, "postgres", ".luxe/mcp.json", "/repo/.luxe/mcp.json"),
    ]
}
