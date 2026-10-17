//! `luxe init` — the setup flow, run on its own.
//!
//! On a terminal it opens the same four questions a first launch asks — a theme, previewed as you
//! move; a provider, through the Configuration surface's own sign-in; the tools; and the start
//! screen — and writes the answers to `~/.config/luxe/config.json` one key at a time, so running it
//! over a file you have edited by hand changes only what it asked about. `--defaults`, or a run
//! with no terminal, writes a file that chooses nothing, so every built-in default applies.
//!
//! The flow runs in the TUI's own event loop with no engine behind it: nothing here starts a
//! session or touches the project it is run in.

use crate::output::outln;
use anyhow::{Context, Result};
use luxe_core::config::Config;
use std::io::{self, IsTerminal};

pub(crate) async fn run_init(
    args: crate::cli::InitArgs,
    settings: crate::engine_setup::AgentSettings,
) -> Result<()> {
    let path = Config::global_path();
    let interactive = !args.defaults && io::stdin().is_terminal() && io::stdout().is_terminal();
    if !interactive && path.exists() && !args.force {
        outln!("A config already exists at {}.", path.display());
        outln!("Run `luxe init` in a terminal to change it, or `luxe init --force` to start over.");
        return Ok(());
    }
    if args.force && path.exists() {
        std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
    }

    if interactive {
        run_setup(settings).await?;
    } else {
        Config::ensure_global_file().context("creating the config file")?;
    }
    report(&path);
    Ok(())
}

/// The setup flow, in the TUI loop, with a channel standing where the engine would be.
async fn run_setup(settings: crate::engine_setup::AgentSettings) -> Result<()> {
    use luxe_tui::main_loop::{self, Opening};

    // Nothing answers: setup sends no turns, and a model chosen after a login is recorded by the
    // login itself. The receiver is drained so a stray action never blocks a full channel.
    let (engine_tx, mut engine_rx) = tokio::sync::mpsc::channel(64);
    let drain = tokio::spawn(async move { while engine_rx.recv().await.is_some() {} });
    let (_events, event_rx) = tokio::sync::mpsc::channel(1);
    let build: luxe_tui::model_picker::ModelMenuSource =
        std::sync::Arc::new(move || crate::model_catalog::build_menu(&settings));
    let status = luxe_tui::statusline::StatusContext {
        model: String::new(),
        provider: String::new(),
        context_window: 0,
        thinking: luxe_core::provider::ThinkingLevel::Off,
        git_branch: None,
        git_dirty: false,
        project: None,
        cwd: None,
        auto_approve: false,
    };
    let exit = main_loop::run(
        event_rx,
        engine_tx,
        status,
        Opening::Setup,
        main_loop::Catalogs {
            models: luxe_tui::model_picker::ModelSources::reading(build),
            sessions: None,
        },
        Some(crate::cli::auth_service()),
        false,
    )
    .await;
    drain.abort();
    exit.map(|_| ())
}

/// What the file now says, in a few lines on the terminal setup handed back.
fn report(path: &std::path::Path) {
    let config = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Config>(&text).ok())
        .unwrap_or_default();
    // The wordmark, set as text, bold where the terminal will show it
    let mark = match io::stdout().is_terminal() {
        true => "\x1b[1mluxe.\x1b[0m",
        false => "luxe.",
    };
    outln!("\n{mark}  Saved {}", luxe_core::paths::home_relative(path));
    outln!("  Theme         {}", config.theme.as_deref().unwrap_or("follows the desktop, or luxe"));
    let disabled = &config.tools.disabled;
    outln!(
        "  Tools         {}",
        match disabled.is_empty() {
            true => "every tool on".to_string(),
            false => format!("off: {}", disabled.join(", ")),
        }
    );
    let start = match (config.dashboard_shown(), config.dashboard_style()) {
        (false, _) => "straight into a conversation",
        (true, luxe_core::config::DashboardStyle::Minimal) => "just the prompt",
        _ => "the launcher",
    };
    outln!("  Start screen  {start}");
    outln!("\nChange any of it in /settings, or run `luxe init` again.");
    report_next_step();
}

/// Point at sign-in when no provider is configured yet.
///
/// A theme and a tool list still can't run a turn, and "what now?" is the question `luxe init`
/// leaves someone with.
fn report_next_step() {
    use luxe_core::auth::AuthService;
    if luxe_providers::auth::ProviderAuth::new().any_configured() {
        outln!("Run `luxe` to start.");
    } else {
        outln!("Next: run `luxe` and choose Set up a provider, or `luxe auth login`.");
    }
}
