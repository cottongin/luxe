//! Subcommand runners — the `loop`, `mcp`, and `sessions` management commands.

use super::args::{LoopCommand, McpCommand, SessionsCommand};
use crate::output::{out, outln};

/// Run a loop subcommand.
pub(crate) async fn run_loop_command(
    cmd: LoopCommand,
    mut settings: crate::engine_setup::AgentSettings,
) -> anyhow::Result<()> {
    use luxe_core::loop_controller::LoopController;

    // The one place this is set: everything the loop owns is opt-in, and an ordinary session must
    // not carry a tool for driving a loop that is not running.
    settings.loop_driven = true;
    // A loop builds; a plan to build from is what plan mode is for, before the loop starts
    settings.initial_mode = None;

    let cwd = settings.state_root.clone();
    let state_path = cwd.join(".luxe").join("loop_state.json");
    let stop_path = cwd.join(".luxe").join("STOP");

    // `.luxe/loop/`, emphatically not `.pi/agent-loop/`, which is another agent harness's directory.
    // Writing a journal there was bad manners; *archiving* it was destructive, because the archive
    // step renames over whatever `PROGRESS.archive.md` it finds and that file was 1.2 MB of
    // somebody else's history.
    tokio::fs::create_dir_all(cwd.join(".luxe").join("loop")).await?;

    let controller = LoopController::new(state_path.clone(), stop_path);
    // Read what the run already is before touching it: a subcommand acting on a default state
    // would report zeros whatever the file said, and the ones that save — `phase --set`, `stop`,
    // `pause`, `resume`, `fresh` — would write those defaults back over the iteration count, the
    // run's spend, the tuning fields and the `phase_baseline` a review diffs against.
    if state_path.exists() {
        if let Err(error) = controller.load().await {
            tracing::warn!(%error, path = %state_path.display(), "could not read the loop state");
        }
    }

    match cmd {
        LoopCommand::Start { quiet, max_turns, compact_percent, pause_percent } => {
            let flags = StartFlags { quiet, max_turns, compact_percent, pause_percent };
            start_loop(controller, settings, flags).await?;
        }
        LoopCommand::Stop => {
            outln!("Stopping loop...");
            controller.stop().await?;
            outln!("Loop stopped.");
        }
        LoopCommand::Pause => {
            outln!("Pausing loop...");
            controller.pause().await?;
            outln!("Loop paused.");
        }
        LoopCommand::Resume => {
            outln!("Resuming loop...");
            controller.resume().await?;
            // `resume` clears the run to continue; it does not drive one, and saying "resumed"
            // and stopping reads exactly like a loop that then did nothing.
            outln!("Loop resumed. Run `luxe loop start` to drive it.");
        }
        LoopCommand::Status => print_loop_status(&controller).await,
        LoopCommand::Phase { set } => {
            if let Some(phase) = set {
                controller.set_phase(phase).await?;
                outln!("Phase set to {}", phase);
            } else {
                let state = controller.get_state().await;
                outln!("Current phase: {}", state.current_phase);
            }
        }
        LoopCommand::Fresh => {
            // Each `start` opens a session of its own; what carries over is the turn count
            controller
                .update_state(|state| {
                    state.iteration = 0;
                })
                .await;
            controller.save().await?;
            let phase = controller.get_state().await.current_phase;
            outln!("Turn count reset, on phase {phase}. Run `luxe loop start` to go on.");
        }
    }

    Ok(())
}

/// What `luxe loop start` was asked for on the command line.
struct StartFlags {
    quiet: bool,
    max_turns: u32,
    compact_percent: Option<u8>,
    pause_percent: u8,
}

/// Builds the gate and the hooks, spawns the engine and drives it until the loop stops.
async fn start_loop(
    mut controller: luxe_core::loop_controller::LoopController,
    mut settings: crate::engine_setup::AgentSettings,
    flags: StartFlags,
) -> anyhow::Result<()> {
    let StartFlags { quiet, max_turns, compact_percent, pause_percent } = flags;
    let cwd = settings.state_root.clone();
    // Two loops in one project share a phase counter, a journal and a `STOP` sentinel, so
    // the second to start quietly corrupts the first one's run. Only a *live* owner
    // refuses: a crashed run must never lock a project out of starting again.
    if let Some(pid) = controller.owned_by_another_process().await {
        anyhow::bail!(
            "a loop is already running in this project (pid {pid}).\n\
             Watch it with `luxe loop status`, or end it with `luxe loop stop` and start \
             again."
        );
    }
    outln!("Starting continuous loop...");

    // One gate, cleared by both a `progress` claim and a `complete` claim.
    let shared = std::sync::Arc::new(controller.clone());
    // Set before either engine is built, so the reviewer's — built from a clone of these
    // settings — reports into the same run without further wiring.
    settings.spend_sink =
        Some(std::sync::Arc::new(crate::engine_setup::LoopSpendSink::new(shared.clone())));
    let gate = crate::engine_setup::build_phase_gate(&settings, shared.clone()).await;
    let reviewing = gate.has_reviewer();
    let commands = crate::engine_setup::phase_gate_config(&settings).gate.commands().len();
    let hook_registry = luxe_core::loop_hooks::build_hooks(shared, cwd.clone(), gate).await;
    controller.set_hook_registry(hook_registry.clone());
    // Asked, not asserted: this line was a hand-written list, so it described whatever the
    // hooks were on the day somebody typed it.
    outln!("Loop hooks, in the order they run: {}", hook_registry.hook_names().join(", "));
    match (commands, reviewing) {
        (0, false) => outln!(
            "Phase gate: nothing configured — a claimed phase is taken at its word (set gate.commands and/or gate.review)"
        ),
        (0, true) => outln!("Phase gate: a reviewer in a fresh context reads the change"),
        (n, false) => outln!(
            "Phase gate: {n} acceptance command(s) (set gate.review for a review too)"
        ),
        (n, true) => outln!(
            "Phase gate: {n} acceptance command(s), then a reviewer in a fresh context reads the change"
        ),
    }

    // After the gate, whose reviewer works in a session of its own
    let compact_percent = tune_for_loop(&mut settings, compact_percent);

    // Apply CLI flags to controller state so runaway guards use them (not defaults).
    controller
        .update_state(|s| {
            s.max_turns_per_run = max_turns;
            s.compact_percent = compact_percent;
            s.pause_percent = pause_percent;
        })
        .await;

    // Spawn the production engine with the real hooks.
    let (action_tx, event_rx) =
        crate::loop_run::spawn_production_engine(settings.clone(), hook_registry).await?;
    outln!("Engine spawned with real provider and loop hooks");

    controller.start().await?;
    outln!(
        "Loop started (max_turns={max_turns}, compact={compact_percent}%, pause={pause_percent}%)"
    );

    // Drive the engine loop until it stops. The kickoff says which phase this is and what
    // the plan says it is — the same thing every later iteration is told.
    let running_rx = controller.running_rx();
    let phase = controller.get_state().await.current_phase;
    let kickoff = luxe_core::hooks::self_continuation::steer_for(&cwd, phase, true).await;
    crate::loop_run::drive_loop(
        action_tx,
        event_rx,
        running_rx,
        kickoff,
        crate::loop_view::LoopView::new(quiet, phase),
    )
    .await?;
    Ok(())
}

/// Prints where the run stands, from the state file and not from what the run last claimed.
async fn print_loop_status(controller: &luxe_core::loop_controller::LoopController) {
    let status = controller.status_summary().await;
    outln!("Loop status:");
    // What the run *is*, not what the file says it intended: a killed run leaves
    // `running: true` behind and has no way to correct it.
    outln!("  Running: {}", status.is_live());
    if let Some(line) = status.liveness_line() {
        outln!("  Process: {line}");
    }
    outln!("  Iteration: {}", status.iteration);
    outln!("  Phase: {}", status.current_phase);
    outln!("  Max turns: {}", status.max_turns_per_run);

    let spend = controller.get_state().await.spend;
    let lines = spend.summary_lines();
    if !lines.is_empty() {
        outln!("  Spend this run:");
        for line in lines {
            outln!("    {line}");
        }
    }
}

/// Run an MCP subcommand against the durable `.luxe/mcp.json` config store.
pub(crate) async fn run_mcp_command(
    cmd: McpCommand,
    settings: &crate::engine_setup::AgentSettings,
) -> anyhow::Result<()> {
    use luxe_core::mcp_client::{McpServerConfig, McpTransport};
    use luxe_core::mcp_config::McpConfigStore;

    let cwd = std::env::current_dir()?;
    let store = McpConfigStore::for_repo(&cwd);

    match cmd {
        McpCommand::List => {
            let servers = McpConfigStore::resolved(&cwd);
            if servers.is_empty() {
                outln!("No MCP servers configured. Add one with `luxe mcp add`.");
                return Ok(());
            }
            outln!("Configured MCP servers ({}):", servers.len());
            for s in &servers {
                let (kind, detail) = match &s.transport {
                    McpTransport::Stdio { command, args, .. } => {
                        ("stdio", format!("{} {}", command, args.join(" ")).trim().to_string())
                    }
                    McpTransport::HttpSse { url } => ("http", url.clone()),
                };
                let state = if s.enabled { "enabled" } else { "disabled" };
                let origin = if s.declared_by_user { "" } else { " not started (project)" };
                outln!("  {} [{}] ({}) {}{} — {}", s.id, s.name, kind, state, origin, detail);
            }
        }
        McpCommand::Add { id, name, command, url } => {
            let transport = match (command, url) {
                (Some(_), Some(_)) => {
                    anyhow::bail!("cannot specify both --command and --url");
                }
                (Some(command), None) => stdio_transport(&command)?,
                (None, Some(url)) => McpTransport::HttpSse { url },
                (None, None) => anyhow::bail!("provide either --command (stdio) or --url (http)"),
            };
            store.add(McpServerConfig {
                id: id.clone(),
                name,
                transport,
                enabled: true,
                ..Default::default()
            })?;
            outln!("Added MCP server '{id}' to {}.", store.path().display());
        }
        McpCommand::Remove { id } => {
            if store.remove(&id)? {
                outln!("Removed MCP server '{id}'.");
            } else {
                outln!("No MCP server '{id}' was configured.");
            }
        }
        McpCommand::Tools => run_mcp_tools(settings).await,
        McpCommand::Login { id } => {
            use luxe_mcp::ServerRegistry;
            let servers = mcp_command_servers(settings);
            let manager = ServerRegistry::new(servers, None);
            let client = manager.oauth_client(&id).ok_or_else(|| {
                anyhow::anyhow!("'{id}' is not an OAuth-configured HTTP MCP server")
            })?;
            // flat on purpose: the chain would print the OS error a second time under `Caused by`
            client.interactive_login().await.map_err(|e| anyhow::anyhow!("{e}"))?;
        }
    }

    Ok(())
}

fn mcp_command_servers(
    settings: &crate::engine_setup::AgentSettings,
) -> Vec<luxe_core::mcp_client::McpServerConfig> {
    use crate::engine_setup::{mcp_servers_for_run, note_project_servers, project_servers_allowed};
    let config = luxe_core::config::Config::load(&settings.state_root);
    note_project_servers(project_servers_allowed(
        config.lsp.project_servers.unwrap_or_default(),
        false,
        settings.trust_project,
    ));
    let (servers, refused) = mcp_servers_for_run(&settings.state_root);
    if !refused.is_empty() {
        outln!(
            "Not starting project MCP servers: {}. Use --trust-project, or set lsp.project_servers in your own config.",
            refused.join(", ")
        );
    }
    servers
}

async fn run_mcp_tools(settings: &crate::engine_setup::AgentSettings) {
    let enabled: Vec<_> = mcp_command_servers(settings).into_iter().filter(|s| s.enabled).collect();
    if enabled.is_empty() {
        outln!("No enabled MCP servers. Add one with `luxe mcp add`.");
        return;
    }
    for server in &enabled {
        print_server_tools(server).await;
    }
}

/// Connect to one server, list its tools, and print them (namespaced). Best-effort:
/// connection failures are reported, not fatal.
///
/// Both transports, over the same [`McpConnection`](luxe_core::mcp_client::McpConnection) the
/// engine uses, so a remote server the runtime connects to is listed too. A listing that disagrees
/// with the runtime is worse than no listing: it reads as "this will not work".
pub(crate) async fn print_server_tools(server: &luxe_core::mcp_client::McpServerConfig) {
    let connect =
        tokio::time::timeout(std::time::Duration::from_secs(15), luxe_mcp::connect(server)).await;

    let client = match connect {
        Ok(Ok(c)) => c,
        Ok(Err(e)) => {
            outln!("{}: failed to connect — {e}", server.id);
            return;
        }
        Err(_) => {
            outln!("{}: timed out connecting", server.id);
            return;
        }
    };

    match client.list_tools().await {
        Ok(tools) => out!("{}", server_tools_listing(server, &tools)),
        Err(e) => outln!("{}: failed to list tools — {e}", server.id),
    }
}

/// What `luxe mcp tools` prints for one server: each tool the agent can call, by the name a
/// permission rule matches (`asana/create_task`), and how many `exclude_tools` hides.
///
/// Never a flattened `mcp_asana_create_task`, a name nothing else uses — a rule copied from the
/// listing would never match. Hidden tools are counted rather than listed: the reader chose them,
/// and the listing is for what is left.
fn server_tools_listing(
    server: &luxe_core::mcp_client::McpServerConfig,
    tools: &[luxe_core::mcp_client::McpTool],
) -> String {
    use std::fmt::Write as _;
    let excluded = |name: &str| server.exclude_tools.iter().any(|hidden| hidden == name);
    let callable: Vec<_> = tools.iter().filter(|tool| !excluded(&tool.name)).collect();
    let hidden = match tools.len() - callable.len() {
        0 => String::new(),
        count => format!("; {count} more hidden by exclude_tools"),
    };
    let mut out = match callable.len() {
        0 => format!("{}: (no tools{hidden})\n", server.id),
        1 => format!("{} (1 tool{hidden}):\n", server.id),
        count => format!("{} ({count} tools{hidden}):\n", server.id),
    };
    for tool in callable {
        let _ = writeln!(out, "  {}/{}  —  {}", server.id, tool.name, tool.description);
    }
    out
}

/// List resumable sessions for the current project (newest first).
pub(crate) async fn run_sessions_command(
    cmd: SessionsCommand,
    settings: crate::engine_setup::AgentSettings,
) -> anyhow::Result<()> {
    use luxe_core::message::{ContentBlock, Role};
    use luxe_core::session::{EntryKind, SessionStore};

    match cmd {
        SessionsCommand::List => {
            let store = crate::engine_setup::open_session_store(&settings.state_root);
            let ids = store.list_sessions();
            if ids.is_empty() {
                outln!("No sessions yet for this project. Start one with `luxe`.");
                return Ok(());
            }

            // Gather (mtime, id, message count, first-user-message snippet) per session.
            let mut rows: Vec<(std::time::SystemTime, String, usize, String)> = Vec::new();
            for id in &ids {
                let mtime = store
                    .session_path(id)
                    .and_then(|p| p.metadata().ok()?.modified().ok())
                    .unwrap_or(std::time::UNIX_EPOCH);
                let entries = store.load(id).unwrap_or_default();
                let msg_count =
                    entries.iter().filter(|e| matches!(&e.kind, EntryKind::Message(_))).count();
                let snippet = entries
                    .iter()
                    .find_map(|e| match &e.kind {
                        EntryKind::Message(m) if m.role == Role::User => {
                            m.content.iter().find_map(|b| match b {
                                ContentBlock::Text(t) if !t.trim().is_empty() => Some(t.clone()),
                                _ => None,
                            })
                        }
                        _ => None,
                    })
                    .unwrap_or_else(|| "(no messages)".to_string());
                rows.push((mtime, id.0.clone(), msg_count, first_line(&snippet, 60)));
            }
            rows.sort_by_key(|r| std::cmp::Reverse(r.0)); // newest first

            outln!("Sessions for this project ({}):", rows.len());
            for (_, id, count, snippet) in rows {
                outln!("  {id}  ({count} msgs)  {snippet}");
            }
            outln!("\nResume with `luxe --resume <id>` or the most recent with `luxe -c`.");
        }
    }
    Ok(())
}

/// A stdio server's transport from `--command`, split the way the platform's shell would, so a
/// quoted argument with a space in it stays one argument.
fn stdio_transport(command: &str) -> anyhow::Result<luxe_core::mcp_client::McpTransport> {
    // A backslash is an escape to a POSIX shell and a path separator on Windows
    let split = if cfg!(windows) { split_quoted(command) } else { shlex::split(command) };
    let mut parts = split
        .ok_or_else(|| anyhow::anyhow!("--command has an unclosed quote: {command}"))?
        .into_iter();
    let program = parts.next().ok_or_else(|| anyhow::anyhow!("--command must not be empty"))?;
    Ok(luxe_core::mcp_client::McpTransport::Stdio {
        command: program,
        args: parts.collect(),
        cwd: None,
        env: Default::default(),
    })
}

/// `command` split on whitespace outside double quotes, the way Windows reads a command line, with
/// every other character as written; `None` for an unclosed quote.
fn split_quoted(command: &str) -> Option<Vec<String>> {
    let (mut words, mut word, mut quote, mut started) = (Vec::new(), String::new(), None, false);
    for c in command.chars() {
        match (quote, c) {
            (None, '"') => (quote, started) = (Some(c), true),
            (Some(open), c) if c == open => quote = None,
            (None, c) if c.is_whitespace() => {
                if std::mem::take(&mut started) {
                    words.push(std::mem::take(&mut word));
                }
            }
            (_, c) => (word, started) = (word + &c.to_string(), true),
        }
    }
    if started {
        words.push(word);
    }
    quote.is_none().then_some(words)
}

/// Points a loop's engine at the threshold it compacts at, returning it in percent for the banner
/// and the loop state.
///
/// Only a flag, or nothing configured, overrides: a configured `compaction.at` reaches the engine
/// as written, rather than rounded through a percentage.
fn tune_for_loop(settings: &mut crate::engine_setup::AgentSettings, flag: Option<u8>) -> u8 {
    let configured = luxe_core::config::Config::load(&settings.state_root).compaction.at;
    let (compact_at, percent) = loop_compaction(flag, configured);
    settings.compact_at = compact_at;
    percent
}

/// `--compact-percent`, else your `compaction.at`, else 75%: the override the engine needs, if any,
/// and the threshold in percent.
fn loop_compaction(flag: Option<u8>, configured: Option<f64>) -> (Option<f64>, u8) {
    match (flag, configured.filter(|at| *at > 0.0 && *at <= 1.0)) {
        (Some(percent), _) => (Some(f64::from(percent) / 100.0), percent),
        (None, Some(at)) => (None, (at * 100.0).round() as u8),
        (None, None) => (Some(0.75), 75),
    }
}

/// First line of `s`, truncated to `max` chars with an ellipsis when longer.
pub(crate) fn first_line(s: &str, max: usize) -> String {
    let line = s.lines().next().unwrap_or("").trim();
    if line.chars().count() > max {
        let truncated: String = line.chars().take(max).collect();
        format!("{truncated}\u{2026}")
    } else {
        line.to_string()
    }
}

#[cfg(test)]
mod tests {
    use luxe_core::mcp_client::{McpServerConfig, McpTool};

    /// A quoted argument is one argument, the way the shell that typed it meant.
    ///
    /// The break this catches: the command was split on every space, so `--command 'node "my
    /// server.js"'` started `node` with `"my` and `server.js"`.
    #[test]
    fn a_quoted_mcp_command_argument_stays_whole() {
        let transport = super::stdio_transport(r#"node "my server.js" --port 3"#).unwrap();
        let luxe_core::mcp_client::McpTransport::Stdio { command, args, .. } = transport else {
            panic!("a stdio transport")
        };
        assert_eq!(command, "node");
        assert_eq!(args, ["my server.js", "--port", "3"]);
        assert!(super::stdio_transport(r#"node "unclosed"#).is_err());
        assert!(super::stdio_transport("   ").is_err());
    }

    /// On Windows a backslash is a path, not an escape, and quotes still group.
    ///
    /// The break this catches: the POSIX splitter read `C:\tools\server.exe` as `C:toolsserver.exe`.
    #[test]
    fn a_windows_command_keeps_its_backslashes() {
        let split = super::split_quoted(r#"C:\tools\server.exe --root "C:\My Files" "" x"#);
        assert_eq!(split.unwrap(), [r"C:\tools\server.exe", "--root", r"C:\My Files", "", "x"]);
        let apostrophe = super::split_quoted(r"C:\Users\O'Brien\srv.exe --stdio");
        assert_eq!(apostrophe.unwrap(), [r"C:\Users\O'Brien\srv.exe", "--stdio"]);
        assert_eq!(super::split_quoted(r#"a "b"#), None);
    }

    /// On Windows, `--command` goes through the Windows splitter, backslashes and all.
    #[cfg(windows)]
    #[test]
    fn a_windows_mcp_command_keeps_its_path() {
        let transport = super::stdio_transport(r"C:\tools\server.exe --stdio").unwrap();
        let luxe_core::mcp_client::McpTransport::Stdio { command, .. } = transport else {
            panic!("a stdio transport")
        };
        assert_eq!(command, r"C:\tools\server.exe");
    }

    /// The flag wins, then your own threshold as written, then the documented 75%.
    ///
    /// The break this catches: `--compact-percent` only decided when a "wrap up" note was added to
    /// every steer, and the engine compacted at its own threshold whatever it said.
    #[test]
    fn a_loop_compacts_at_the_flag_then_the_config_then_75() {
        assert_eq!(super::loop_compaction(Some(40), Some(0.25)), (Some(0.4), 40));
        assert_eq!(super::loop_compaction(None, Some(0.25)), (None, 25));
        assert_eq!(super::loop_compaction(None, None), (Some(0.75), 75));
        assert_eq!(super::loop_compaction(None, Some(7.5)), (Some(0.75), 75), "out of range");
    }

    /// The loop's settings carry the threshold to the engine they build.
    #[test]
    fn the_loop_s_settings_carry_its_compaction_threshold() {
        let _guard = crate::engine_setup::test_env::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let project = tempfile::tempdir().unwrap();
        let mut settings = crate::engine_setup::AgentSettings::resolve(
            project.path().to_path_buf(),
            crate::engine_setup::CliOverrides::default(),
        );

        assert_eq!(super::tune_for_loop(&mut settings, Some(30)), 30);
        assert_eq!(settings.compact_at, Some(0.3));
    }

    fn tool(name: &str) -> McpTool {
        McpTool {
            server_id: "asana".into(),
            name: name.into(),
            description: format!("does {name}"),
            input_schema: serde_json::Value::Null,
        }
    }

    /// The listing names each tool the way a permission rule matches it, leaves out what
    /// `exclude_tools` hides, and says how many that was.
    #[test]
    fn the_listing_uses_the_names_rules_match_and_counts_what_is_hidden() {
        let server = McpServerConfig {
            id: "asana".into(),
            exclude_tools: vec!["log_widget_event".into()],
            ..Default::default()
        };
        let tools = [tool("get_tasks"), tool("create_tasks"), tool("log_widget_event")];
        assert_eq!(
            super::server_tools_listing(&server, &tools),
            "asana (2 tools; 1 more hidden by exclude_tools):\n  asana/get_tasks  —  does \
             get_tasks\n  asana/create_tasks  —  does create_tasks\n"
        );
        let open = McpServerConfig { id: "asana".into(), ..Default::default() };
        assert_eq!(
            super::server_tools_listing(&open, &tools[..1]),
            "asana (1 tool):\n  asana/get_tasks  —  does get_tasks\n"
        );
        assert_eq!(
            super::server_tools_listing(&server, &tools[2..]),
            "asana: (no tools; 1 more hidden by exclude_tools)\n"
        );
    }
}
