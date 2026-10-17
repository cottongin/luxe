//! CLI entry point — parses arguments and dispatches to TUI, headless, or a subcommand.

use anyhow::Context;

mod args;
mod auth_command;
mod commands;
mod docs_command;
mod doctor_command;
pub(crate) mod headless;
mod models_command;
mod rpc;
mod tui;
pub(crate) mod update_command;
mod wire_command;
mod worktree;

pub(crate) use args::{Args, InitArgs, SubCommand, UsageError};

use args::{permission_mode_from_args, resume_from_args};
use auth_command::run_auth_command;
use commands::{run_loop_command, run_mcp_command, run_sessions_command};
use doctor_command::run_doctor_command;
use headless::run_headless;
use models_command::run_models_command;
use tui::run_tui;
use wire_command::run_wire_command;
use worktree::enter_worktree;

/// The login engine the TUI drives for `/login`.
///
/// Built here because `luxe-tui` may not depend on `luxe-providers`; the TUI only ever sees the
/// `AuthService` trait.
pub(crate) fn auth_service() -> luxe_tui::auth_wizard::AuthServiceHandle {
    std::sync::Arc::new(luxe_providers::auth::ProviderAuth::new())
}

/// Top-level dispatch: parse args and route to the requested mode.
pub(crate) async fn run(mut args: Args) -> anyhow::Result<()> {
    // The guide is in the binary and needs nothing else: no project, no settings, no network
    if let Some(SubCommand::Docs(docs)) = &args.command {
        return docs_command::run(docs);
    }
    // An update is about this binary, not a project, so it runs from anywhere
    if let Some(SubCommand::Update(update)) = &args.command {
        return update_command::run(update.check).await;
    }
    args.validate()?;
    // Where the project begins, which is not always where the shell was standing. Everything after
    // this — sessions, logs, instructions, the file index, every tool's relative path — is measured
    // from the one answer.
    let invoked_from = std::env::current_dir()?;
    let root = enter_project(&mut args, &invoked_from)?;
    // Resolve provider/model/auth once (CLI flags > env > config) and share it downstream.
    let mut settings = crate::engine_setup::AgentSettings::resolve(root, args.overrides());
    settings.resume = resume_from_args(&args);
    settings.permission_mode = permission_mode_from_args(&args);
    settings.trust_project = args.trust_project;
    settings.disable_tools = args.no_tool.clone();
    settings.enable_tools = args.with_tool.clone();
    if let Some(raw) = args.thinking.as_deref() {
        match args::parse_thinking_level(raw) {
            Some(level) => {
                settings.initial_thinking = Some(level);
                settings.thinking_forced = true;
            }
            None => eprintln!("luxe: ignoring unknown --thinking level '{raw}'"),
        }
    }
    settings.auto_approve = args.auto_approve;
    if let Some(on_ask) = args.on_ask {
        settings.on_ask = on_ask.into();
    }
    if args.auto_approve && !args.prompts_a_human() && args.starts_an_engine() {
        settings.startup_notices.push(
            "--auto-approve: an unattended run already allows an `ask`. `--on-ask deny` is how to \
             make one refuse instead."
                .to_string(),
        );
    }
    if args.plan {
        settings.initial_mode = Some(luxe_core::mode::PLAN.to_string());
    }

    if args.starts_an_engine() {
        enter_worktree(&args, &mut settings).await?;
    }

    // Redacting file logger — safe for the TUI (never touches the terminal) and for headless/loop
    // runs. Best-effort; never blocks. A run that owns no project state logs to the user's state
    // directory instead, so a reporting command cannot leave a `.luxe/` behind that re-roots every
    // repository under it.
    crate::logging::init(&match args.owns_project_state() {
        true => crate::logging::LogHome::Project(settings.state_root.clone()),
        false => crate::logging::LogHome::User,
    });

    // Keep the models.dev catalog current for every entry point, not just the TUI — a headless or
    // loop run needs the right context window just as much. Best-effort and non-blocking.
    crate::model_catalog::spawn_refresh();

    // Ask, at most daily, whether a newer Luxe exists. The answer lands in a cache and is read by
    // the *next* launch's dashboard, so nothing here is on the path to a first token.
    crate::update_check::spawn_check(&settings.state_root);

    if let Some(cmd) = args.command {
        match cmd {
            SubCommand::Auth(auth_args) => return run_auth_command(auth_args.command).await,
            SubCommand::Init(init_args) => return crate::init::run_init(init_args, settings).await,
            SubCommand::Loop(loop_args) => {
                return run_loop_command(loop_args.command, settings).await;
            }
            SubCommand::Mcp(mcp_args) => return run_mcp_command(mcp_args.command, &settings).await,
            SubCommand::Models(models_args) => {
                return run_models_command(models_args.command, settings).await
            }
            SubCommand::Sessions(sessions_args) => {
                return run_sessions_command(sessions_args.command, settings).await
            }
            SubCommand::Doctor => return run_doctor_command(settings).await,
            SubCommand::Wire(wire_args) => return run_wire_command(wire_args),
            SubCommand::Docs(_) | SubCommand::Update(_) => {
                unreachable!("`luxe docs` and `luxe update` return before a project is entered")
            }
        }
    }

    if args.rpc {
        rpc::run_rpc(settings, args.github_writes).await?;
    } else if args.headless {
        let prompt = args.prompt.as_deref().expect("`validate` refuses --headless with no prompt");
        run_headless(prompt, settings, args.format).await?;
    } else {
        run_tui(settings).await?;
    }
    Ok(())
}

/// Moves the process to the project root, and reports it.
///
/// The chdir is what keeps the root a *single* concept: [`luxe_core::tool::ToolContext`] takes its
/// working directory from the process, so leaving the shell's directory in place would give tools
/// one root and storage another — relative paths resolving against a subdirectory whose sessions
/// live somewhere else. Opening a project from the dashboard has always done exactly this.
///
/// Any path an argument already carries is absolutized first, against the directory the user typed
/// it in. A relative path means "from here", and here is about to move.
fn enter_project(
    args: &mut Args,
    invoked_from: &std::path::Path,
) -> anyhow::Result<std::path::PathBuf> {
    let root = luxe_core::project::discover(invoked_from);
    if root == invoked_from {
        return Ok(root);
    }
    // The shell's directory keeps one job after the move: naming which instruction files apply,
    // so a monorepo's `packages/app/AGENTS.md` still reaches a session launched there.
    luxe_core::project::note_launched_from(invoked_from);
    if let Some(SubCommand::Wire(wire)) = args.command.as_mut() {
        if wire.log.is_relative() {
            wire.log = invoked_from.join(&wire.log);
        }
    }
    std::env::set_current_dir(&root)
        .with_context(|| format!("entering project {}", root.display()))?;
    tracing::debug!(root = %root.display(), from = %invoked_from.display(), "entered project");
    Ok(root)
}

#[cfg(test)]
mod tests;
