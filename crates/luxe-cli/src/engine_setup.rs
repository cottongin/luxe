//! Engine wiring — assembles a production `EngineConfig` shared by every entry point.
//!
//! `Action → Engine::spawn → Event`. The TUI, GUI, headless mode, and the continuous
//! loop all go through [`build_engine`] so provider/auth, tools, session, and task
//! memory are wired identically. UIs only ever see the returned channels.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use luxe_core::engine::{Action, Engine, EngineConfig};
use luxe_core::{Event, HookRegistry};
use tokio::sync::mpsc;

mod config;
mod phase_reviewer;
mod provider;

mod services;
mod session;
mod settings;
mod spend;
#[cfg(test)]
pub(crate) mod test_env;
use session::attach_session;
pub(crate) use session::{
    open_elsewhere_now, resumed_session, IssueBinding, ResumeMode, WorktreeBinding,
};
pub(crate) use settings::{AgentSettings, CliOverrides};
pub(crate) use spend::LoopSpendSink;
mod tooling_summary;
mod tools;

pub(crate) use config::effective_permission_mode;
use config::{env_disabled, global_permissions_path, load_command_hooks, load_permission_engine};
pub(crate) use config::{env_nonempty, load_luxe_config, models_dev_cache_path, provider_key_env};
pub(crate) use phase_reviewer::{build_phase_gate, phase_gate_config};
use provider::{build_active_adapter, build_provider_registry};
pub(crate) use services::{
    build_graph_service, build_lsp_service, note_project_servers, open_session_store,
    project_servers_allowed, project_tooling_allowed, shared_dap_service, shared_diagnostics_store,
    shared_external_work, SharedLspService,
};
pub(crate) use services::{merged_debug_adapters, read_debug_adapters, read_lsp_servers};
pub(crate) use tooling_summary::collect_tooling_summary;
pub(crate) use tools::mcp_servers_for_run;
use tools::{apply_lazy_tools, apply_tool_deny_list, build_tool_registry, register_mcp_tools};

/// A spawned engine: the channels a front-end drives it with, and what it attached to.
pub(crate) struct EngineHandle {
    /// Front-end → engine intents.
    pub(crate) actions: mpsc::Sender<Action>,
    /// Engine → front-end stream.
    pub(crate) events: mpsc::Receiver<Event>,
    /// The session this engine resolved to — created, or resumed from `--resume`/`--continue`.
    ///
    /// Resolved inside [`build_engine`], so a caller that asks the store again would either race
    /// it or create a second session. RPC announces it in its handshake.
    pub(crate) session_id: luxe_core::SessionId,
    /// How a front-end that is exiting lets the engine finish first.
    pub(crate) closer: EngineCloser,
    /// The claim on the session taken before the engine existed, so nobody else can take it
    /// before the engine claims it too; dropping it leaves the engine's own
    pub(crate) claim: Option<luxe_core::session::SessionClaim>,
}

/// What an idle engine needs to wind down with no `SessionEnd` hooks to wait on.
///
/// Also the whole of the wait when a turn is still running, since `Close` queues behind it: a quit
/// mid-turn should not stall on a turn nobody will read.
const CLOSE_FLOOR: std::time::Duration = std::time::Duration::from_secs(1);

/// The engine's task, and how long to give it once told to close.
pub(crate) struct EngineCloser {
    task: tokio::task::JoinHandle<luxe_core::engine::Carryover>,
    grace: std::time::Duration,
}

impl EngineCloser {
    /// Tell the engine to close and wait for it, bounded by the `SessionEnd` hooks' own deadlines.
    ///
    /// Without the wait a front-end returned from `main` while the engine was still on its way
    /// out, the runtime dropped the task, and every `SessionEnd` hook died with it — headless runs
    /// never fired one at all. An engine still running at the deadline is aborted rather than
    /// detached: the TUI starts the next session's engine straight after, possibly in another
    /// checkout, and the old one's tools must not run on beside it.
    ///
    /// Answers the engine's [`luxe_core::engine::Carryover`], for the TUI's next engine to start
    /// from; one that had to be stopped hands nothing over.
    pub(crate) async fn close(
        self,
        actions: &mpsc::Sender<Action>,
    ) -> Option<luxe_core::engine::Carryover> {
        actions.send(Action::Close).await.ok();
        let mut task = self.task;
        match tokio::time::timeout(self.grace, &mut task).await {
            Ok(finished) => finished.ok(),
            Err(_) => {
                tracing::warn!(grace = ?self.grace, "the engine did not finish closing in time; stopping it");
                task.abort();
                let _ = task.await;
                None
            }
        }
    }
}

/// Assemble a fully-wired engine and spawn it.
///
/// The caller supplies the hook registry (empty for a plain interactive session; the
/// continuous-loop hooks otherwise). Provider, model, tools, session, and the durable checklist
/// are wired here so every front-end is identical.
pub(crate) async fn build_engine(
    settings: &AgentSettings,
    hook_registry: HookRegistry,
    ask: luxe_core::permission::AskResolution,
) -> anyhow::Result<EngineHandle> {
    // Once, and then read: a load per block would be seven parses of the same two files, and seven
    // chances for them to disagree if one landed mid-edit.
    let config = luxe_core::config::Config::load(&settings.state_root);

    // Here rather than at argument parsing, because this is the first place that holds both facts
    // the answer needs: whether a front-end can prompt, and whether the run trusts this project.
    // It must precede the `build_lsp_service()` below, which is what consumes it.
    note_project_servers(project_servers_allowed(
        config.lsp.project_servers.unwrap_or_default(),
        matches!(ask, luxe_core::permission::AskResolution::Prompt),
        settings.trust_project,
    ));

    let todo_root = prepare_state_dirs(settings).await?;

    let mut store = open_session_store(&settings.state_root);
    let (session_id, claim) = attach_session(settings, &mut store)?;

    let model = settings.launch_model();

    // Provider routing: the adapter the settings selected, plus a registry of every provider luxe
    // knows about, so an in-session model switch *or* a mid-session `/login` reaches the right one.
    let provider = build_active_adapter(settings);
    let providers = build_provider_registry(settings, &provider);

    // One warm LSP service backs both the tools and the diagnostics-on-edit hook.
    let lsp_service = build_lsp_service();
    let (tool_registry, denied) = assemble_tools(settings, &config, lsp_service.clone()).await;
    let (hook_registry, session_end_budget) =
        assemble_hooks(settings, &config, hook_registry, lsp_service, &todo_root);

    let engine_config = EngineConfig {
        session_store: std::sync::RwLock::new(Box::new(store)),
        provider,
        providers,
        model,
        tool_registry,
        hook_registry,
        session_id,
        system_prompt: None,
        // What the CLI found before the engine existed, plus what wiring the registry turned up:
        // an instruction that disabled nothing is otherwise only in the log.
        startup_notices: settings.startup_notices.iter().cloned().chain(denied.notices()).collect(),
        todo_store: Some(luxe_core::TodoStore::new(todo_root)),
        compaction_settings: compaction_settings(settings.compact_at, &config.compaction),
        // Policy = defaults ⊕ mode ⊕ global config, with a tighten-only project overlay; the
        // front-end chooses whether an `ask` prompts (TUI) or auto-allows (headless/loop).
        permission: luxe_core::permission::PermissionSettings {
            engine: load_permission_engine(
                &settings.state_root,
                settings.permission_mode,
                settings.trust_project,
            ),
            ask,
        },
        modes: build_modes(settings, &config),
        // A reviewer has nobody to ask for a second opinion of its own
        reviewer: (!settings.review_engine)
            .then(|| phase_reviewer::on_demand_review(settings, &config)),
        formatter_registry: None,
        spend_sink: settings.spend_sink.clone(),
        guided_review_project_root: Some(settings.work_root.clone()),
        invoked_from: luxe_core::project::launched_from(),
        // The record the external-work hook writes, so a peek at a file in no repository can be
        // measured against the copy retained before this session first wrote to it.
        external_work: Some(services::shared_external_work()),
        review_context_files: config.review.context_files.clone(),
        review_exclude: config.review.exclude.clone(),
        // A real session remembers what the user picked, here and anywhere; embedders and tests
        // leave no trace.
        remember_choices_in: config.agent.remember_model().then(|| settings.work_root.clone()),
        cache_warming: config.agent.cache_warming(),
        // Ground Guided Review in real structure (relevant files + test gaps) from the warm graph.
        review_insight_provider: Some(Arc::new(crate::review_insight::GraphReviewInsight::new(
            build_graph_service(&settings.work_root),
        ))),
        // Pull-request reviews read through the user's own `gh`
        github: Some(Arc::new(luxe_core::github::GhCli::new())),
        // "Remember this" approvals go to the *global* file — a project ruleset is tighten-only,
        // so an approval written there would be ignored.
        permissions_file: Some(global_permissions_path()),
        max_tool_iterations: settings.max_tool_iterations,
        action_tx: None,
    };

    let attached = engine_config.session_id.clone();
    let (actions, events, task) =
        Engine::spawn_task_carrying(engine_config, settings.carryover.clone());
    let closer = EngineCloser { task, grace: CLOSE_FLOOR + session_end_budget };

    // A session starts in build; `--plan` and `agent.default_mode` are how it starts in another
    // mode, for every front-end at once. An engine carrying over from another starts in its mode
    let starting_mode = settings.initial_mode.clone().filter(|_| settings.carryover.is_none());
    if let Some(mode) = starting_mode.filter(|_| !settings.review_engine) {
        tracing::info!(%mode, "starting in the requested mode");
        actions
            .send(Action::SetMode(mode))
            .await
            .map_err(|error| anyhow::anyhow!("engine channel closed: {error}"))?;
    }

    // The issue rides with the first message the session sends, once
    if let Some(issue) = &settings.issue {
        actions
            .send(Action::SetSessionBrief(issue.brief.clone()))
            .await
            .map_err(|error| anyhow::anyhow!("engine channel closed: {error}"))?;
    }

    Ok(EngineHandle { actions, events, session_id: attached, closer, claim })
}

/// Makes the directories a session writes into, and answers where its checklists go.
async fn prepare_state_dirs(settings: &AgentSettings) -> anyhow::Result<PathBuf> {
    let session_dir = settings.state_root.join(".luxe").join("sessions");
    tokio::fs::create_dir_all(&session_dir)
        .await
        .with_context(|| format!("creating session dir {}", session_dir.display()))?;
    let todo_root = settings.state_root.join(".luxe").join("todos");
    if let Err(error) = tokio::fs::create_dir_all(&todo_root).await {
        // The checklist still works for this run if the directory turns up some other way; it just
        // cannot be saved, which the reader would otherwise find out only on the next resume
        tracing::warn!(%error, path = %todo_root.display(), "could not create the todo directory");
    }
    Ok(todo_root)
}

/// The tools this run has: the base set, any opt-in MCP servers, then the deny-list and the
/// deferred tiers, and what was left out.
async fn assemble_tools(
    settings: &AgentSettings,
    config: &luxe_core::config::Config,
    lsp_service: SharedLspService,
) -> (luxe_core::ToolRegistry, tools::ToolDenyReport) {
    // Base tools + any opt-in MCP servers from `.luxe/mcp.json`.
    let mut tool_registry = build_tool_registry(
        &settings.state_root,
        &settings.work_root,
        lsp_service,
        settings.loop_driven,
    );
    register_mcp_tools(&mut tool_registry, &settings.state_root).await;
    // Honor the config's per-tool deny-list (`luxe init` writes it) with this run's
    // `--no-tool`/`--with-tool` layered over it; essential tools stay.
    let denied = apply_tool_deny_list(
        &mut tool_registry,
        &settings.state_root,
        &settings.disable_tools,
        &settings.enable_tools,
    );
    if settings.review_engine {
        keep_only_reviewer_tools(&mut tool_registry, &config.reviewer);
    }
    apply_lazy_tools(&mut tool_registry, &settings.state_root);
    // Settled here rather than per snapshot: tiering is the last thing to touch the registry, and
    // what `/tools` reports is a wiring-time decision, not a live one.
    services::note_tool_statuses(tools::collect_tool_statuses(&tool_registry, denied.removals()));
    (tool_registry, denied)
}

/// The hooks every session carries on top of the caller's, and how long the `SessionEnd` ones may
/// take to wind down.
fn assemble_hooks(
    settings: &AgentSettings,
    config: &luxe_core::config::Config,
    mut hook_registry: HookRegistry,
    lsp_service: SharedLspService,
    todo_root: &std::path::Path,
) -> (HookRegistry, std::time::Duration) {
    // Format-on-save + diagnostics-on-edit fire via the FileChanged hook that the edit/write
    // tools raise. We register it here (not via EngineConfig.formatter_registry) so it shares the
    // warm LSP service, which core can't construct.
    hook_registry.register(Arc::new(build_file_changed_hook(
        lsp_service,
        &settings.work_root,
        config.tools.format_on_save.unwrap_or(true),
    )));
    // The checklist is the only view the user has of the plan, and it drifts one way: a model
    // opens one, works, finishes, and never marks anything. Told mid-turn, on the result of a call
    // it was already making, the agent can still fix it.
    hook_registry.register(Arc::new(luxe_core::hooks::PlanReminderHook::new(
        luxe_core::TodoStore::new(todo_root.to_path_buf()),
    )));
    // Forget the code graph and the symbol index on every file change, so both stay current
    let graph = build_graph_service(&settings.work_root);
    let invalidate = crate::graph_hook::GraphInvalidateHook::new(graph);
    hook_registry.register(Arc::new(invalidate.with_symbol_index(services::shared_symbol_index())));
    // Git stops at the worktree, so an edit outside it leaves no trace for the Changes surface to
    // find. This retains what such a path held before the tool ran, on the same call the
    // external-directory guard is evaluated on.
    hook_registry.register(Arc::new(luxe_core::external_work::ExternalWorkHook::new(
        settings.work_root.clone(),
        services::shared_external_work(),
    )));
    // User-configured external command hooks (global config; project hooks only when trusted).
    let mut session_end_budget = std::time::Duration::ZERO;
    if let Some(command_hook) = load_command_hooks(&settings.state_root, settings.trust_project) {
        tracing::info!("registered command hooks from config");
        session_end_budget = command_hook.budget(luxe_core::HookPoint::SessionEnd);
        hook_registry.register(Arc::new(command_hook));
    }
    if let Some(herdr_hook) = herdr_hook(config) {
        tracing::info!("herdr: reporting agent state to the current pane");
        hook_registry.register(Arc::new(herdr_hook));
    }
    (hook_registry, session_end_budget)
}

/// The hook that reports the agent's state to the Herdr pane luxe runs in, so its sidebar tracks
/// luxe. `None` outside Herdr, under `LUXE_HERDR=0`, or with `"herdr": false`.
fn herdr_hook(config: &luxe_core::config::Config) -> Option<luxe_core::hooks::herdr::HerdrHook> {
    config.herdr().then(luxe_core::hooks::herdr::HerdrHook::detect).flatten()
}

/// The project's compaction settings, at the loop's mark when it set one.
///
/// A named summarizer is resolved through the model catalog, so its call is priced and its reply
/// capped to its own window like any other model's.
fn compaction_settings(
    compact_at: Option<f64>,
    config: &luxe_core::config::CompactionConfig,
) -> luxe_core::compaction::CompactionSettings {
    let at = compact_at.or(config.at);
    let mut settings = luxe_core::config::CompactionConfig { at, ..config.clone() }.settings();
    if let Some(named) = settings.summarizer.take() {
        let models = load_luxe_config();
        let (provider, id) = (&named.r#ref.provider, &named.r#ref.id);
        settings.summarizer = Some(crate::model_catalog::resolve_model(&models, provider, id).0);
    }
    settings
}

/// The modes this engine can be in: build and plan, or a reviewer's single read-only one.
///
/// Plan mode's model is resolved here rather than when the mode is entered, because a model brings
/// a context window and a price with it. A model named on the command line outranks it, the same as
/// any other configured default: a flag is a one-off.
fn build_modes(
    settings: &AgentSettings,
    config: &luxe_core::config::Config,
) -> luxe_core::mode::Modes {
    use luxe_core::mode::{Mode, Modes};
    if settings.review_engine {
        return Modes::only(Mode::read_only(luxe_core::phase_review::REVIEW_MODE));
    }
    let plan = &config.plan;
    let mut mode = Mode::plan(plan.files.clone());
    match (plan.model.as_deref(), plan.model_ref()) {
        (Some(_), Some((provider, id))) if !settings.model_forced => {
            let models = load_luxe_config();
            mode = mode.with_model(crate::model_catalog::resolve_model(&models, provider, id).0);
        }
        (Some(raw), None) => {
            tracing::warn!(model = %raw, "ignoring a plan.model that is not `provider/id`");
        }
        _ => {}
    }
    if let Some(thinking) = plan.thinking.filter(|_| !settings.thinking_forced) {
        mode = mode.with_thinking(thinking);
    }
    Modes::built_in(mode)
}

/// Narrows a review engine's tools to the reviewer's: `reviewer.tools`, or the built-in list.
///
/// Whatever is named, the review's mode refuses every write, so this decides what a reviewer is
/// offered rather than what it may change. `activate_tools` stays, because half the built-in list
/// is deferred behind it.
fn keep_only_reviewer_tools(
    registry: &mut luxe_core::ToolRegistry,
    reviewer: &luxe_core::config::ReviewerConfig,
) {
    let wanted: Vec<String> = match reviewer.tools.as_ref().filter(|tools| !tools.is_empty()) {
        Some(tools) => tools.clone(),
        None => {
            luxe_core::phase_review::REVIEWER_TOOLS.iter().map(|tool| tool.to_string()).collect()
        }
    };
    let names: Vec<String> = registry.names().into_iter().map(str::to_string).collect();
    for name in names {
        if !wanted.contains(&name) && name != luxe_core::tool::ACTIVATE_TOOLS_NAME {
            registry.unregister(&name);
        }
    }
}

/// Build the format-on-save + diagnostics-on-edit hook, sharing the warm LSP service.
///
/// Format-on-save and diagnostics-on-edit are both **on by default**. Which formatter runs is
/// [`luxe_core::formatter::FormatterRegistry::formatter_for`]'s call, and `format_on_save` turns
/// it off. The service keeps servers warm across edits, so only the first edit pays
/// startup/indexing, and a missing server degrades to "no diagnostics" rather than blocking. Set
/// `LUXE_LSP_ON_EDIT=0` to suppress diagnostics-on-edit (formatting still runs). Diagnostics
/// published here also feed the Context Dock via the shared [`shared_diagnostics_store`].
///
/// The registry is bound to this project, so it formats against the edition the repository declares
/// and — under the same answer the language servers get — prefers the formatter the repository
/// installed for itself.
fn build_file_changed_hook(
    lsp_service: SharedLspService,
    repo_root: &std::path::Path,
    format_on_save: bool,
) -> luxe_core::hooks::file_changed::FileChangedHook {
    let formatter = format_on_save.then(|| {
        luxe_core::formatter::FormatterRegistry::for_project(repo_root, project_tooling_allowed())
    });
    let lsp: Option<Arc<dyn luxe_core::lsp::DiagnosticsProvider>> =
        if env_disabled("LUXE_LSP_ON_EDIT") { None } else { Some(lsp_service) };
    let mut hook = luxe_core::hooks::file_changed::FileChangedHook::new(formatter, lsp)
        .with_diagnostics_store(shared_diagnostics_store());
    // The size standards the system prompt sets, said at the edit that crosses one
    if !env_disabled("LUXE_SIZE_CHECK") {
        hook = hook.with_outline(Arc::new(luxe_lsp::syntax::SyntaxOutline));
    }
    // A test gap reaches the agent while it can still act on it, rather than the reviewer afterwards.
    // Rides the same warm graph the review's insight uses, and honors the same opt-out as
    // diagnostics-on-edit: both are "tell me what my edit implies".
    if env_disabled("LUXE_LSP_ON_EDIT") {
        hook
    } else {
        hook.with_test_gaps(Arc::new(crate::review_insight::GraphTestGaps::new(
            build_graph_service(repo_root),
        )))
    }
}

#[cfg(test)]
mod tests;
