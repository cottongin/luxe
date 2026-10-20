//! The TUI run — dashboard, engine, main loop, and what to do when it exits.

use super::auth_service;
use super::worktree::{
    check_out_pull_request, enter_session_worktree, remove_worktree, take_up_issue_in_worktree,
};

/// The one line under the dashboard, or nothing: that an update exists, and how to take it. The
/// versions and the command wait for `/update`, where the reader decides.
fn footer_note(repo_root: &std::path::Path) -> Option<String> {
    crate::update_check::cached_notice(repo_root).map(|_| UPDATE_HINT.to_string())
}

/// What the dashboard says when a newer Luxe exists.
const UPDATE_HINT: &str = "Update available · /update";

/// Whether the startup dashboard should be shown.
///
/// Suppressed when resuming, when the session already has its work — `--issue` or `--pr` said
/// what it is for, so a menu of other things to do is in the way — or when the config sets
/// `dashboard = false`.
pub(super) fn should_show_dashboard(
    config: &luxe_core::config::Config,
    resume: &crate::engine_setup::ResumeMode,
    started_on_work: bool,
) -> bool {
    if *resume != crate::engine_setup::ResumeMode::New || started_on_work {
        return false;
    }
    config.dashboard_shown()
}

/// Run the TUI mode against a fully-wired engine.
///
/// The loop exists for the one exit that comes back: opening a file in `$EDITOR` and returning to
/// the surface that asked. Everything else leaves the process, and the project a run works on is
/// fixed the moment [`super::enter_project`] answers.
pub(super) async fn run_tui(
    mut settings: crate::engine_setup::AgentSettings,
) -> anyhow::Result<()> {
    use luxe_core::config::Config;
    use luxe_tui::main_loop::{self, Opening};

    let mut show_dashboard = should_show_dashboard(
        &Config::load(&settings.state_root),
        &settings.resume,
        settings.issue.is_some() || settings.pull_request.is_some(),
    );
    let mut carried = Carried::default();

    loop {
        let dashboard = match show_dashboard {
            true => Some(open_dashboard(&settings).await),
            false => {
                tracing::info!(resume = ?settings.resume, "skipping dashboard");
                None
            }
        };
        let status = starting_status(&settings).await;
        let catalogs = main_loop::Catalogs {
            models: model_menu_sources(&settings),
            sessions: Some(session_catalog_source(&settings)),
        };
        let crate::engine_setup::EngineHandle {
            actions: engine_tx,
            events: engine_rx,
            closer,
            session_id,
            claim: _claim,
        } = start_engine(&settings).await?;
        let event_rx = crate::workspace_runtime::bridge(
            engine_rx,
            crate::workspace_runtime::Roots {
                state: settings.state_root.clone(),
                work: settings.work_root.clone(),
            },
            crate::workspace_runtime::pull_request_watch(&settings.work_root),
        );
        let (event_rx, _sharing) =
            super::share::share(event_rx, engine_tx.clone(), &settings, &session_id.0);
        let exit = main_loop::run(
            event_rx,
            engine_tx.clone(),
            status,
            match (carried.fixing.take(), dashboard, carried.resumed.take()) {
                (Some(launch), _, _) => Opening::FixPullRequest(launch),
                (None, Some(dashboard), _) => Opening::Dashboard(Box::new(dashboard)),
                (None, None, Some(launch)) => Opening::Resumed(launch),
                (None, None, None) => Opening::Chat,
            },
            catalogs,
            Some(auth_service()),
            false,
        )
        .await;
        // The next engine, whichever session it runs, starts where this one was
        settings.carryover = closer.close(&engine_tx).await;
        if let Some(carryover) = &settings.carryover {
            settings.auto_approve = carryover.auto_approves();
        }

        let mut next = Next { settings: &mut settings, show_dashboard: &mut show_dashboard };
        match next.after(exit?, session_id, &mut carried).await? {
            Flow::Leave => return Ok(()),
            Flow::Again => {}
        }
    }
}

/// What one lap of the loop hands the next.
#[derive(Default)]
struct Carried {
    /// A pull request checked out to fix, with the comments that go with it
    fixing: Option<luxe_tui::main_loop::FixLaunch>,
    /// What the reader carried out of the session or start screen they left for another
    resumed: Option<luxe_tui::main_loop::ResumeLaunch>,
}

/// What the loop does once the surface has exited.
enum Flow {
    /// The process is done
    Leave,
    /// Another session, or this one again
    Again,
}

/// The state a TUI exit changes before the next lap: which session runs, and whether it opens on
/// the dashboard.
struct Next<'a> {
    settings: &'a mut crate::engine_setup::AgentSettings,
    show_dashboard: &'a mut bool,
}

impl Next<'_> {
    /// Settles an exit: quit, or the session to carry on with.
    ///
    /// A session that cannot be started leaves this one running with a notice saying why, rather
    /// than taking the reader out of the TUI.
    async fn after(
        &mut self,
        exit: luxe_tui::main_loop::TuiExit,
        session_id: luxe_core::SessionId,
        carried: &mut Carried,
    ) -> anyhow::Result<Flow> {
        use crate::engine_setup::ResumeMode;
        use luxe_tui::main_loop::TuiExit;

        match exit {
            TuiExit::Quit => {
                // Graceful LSP `shutdown`+`exit` before the process ends (kinder than the bare
                // kill_on_drop for servers like clangd/gopls); bounded so a stuck server can't
                // hang quit.
                shutdown_workspace_services().await;
                return Ok(Flow::Leave);
            }
            TuiExit::RemoveWorktree(path) => {
                shutdown_workspace_services().await;
                remove_worktree(&self.settings.state_root, &path).await;
                return Ok(Flow::Leave);
            }
            TuiExit::Update => {
                // The terminal is the reader's again, so the package manager's output reaches them
                shutdown_workspace_services().await;
                super::update_command::run(false).await?;
                return Ok(Flow::Leave);
            }
            TuiExit::TakeUpIssue(number) => {
                // A new session on the issue, in its worktree; the one left stays as it was
                shutdown_workspace_services().await;
                let mut next = self.fresh(ResumeMode::New);
                match take_up_issue_in_worktree(number, &mut next).await {
                    Ok(()) => *self.settings = next,
                    Err(error) => self.carry_on(
                        session_id,
                        format!(
                            "A session on issue #{number} could not start, so this one carries \
                             on: {error:#}."
                        ),
                    ),
                }
            }
            TuiExit::ResumeSession(launch) => {
                // Another of your sessions, in its own worktree when it has one
                shutdown_workspace_services().await;
                let mut next = self.fresh(ResumeMode::Session(launch.session.clone()));
                let state_root = &self.settings.state_root;
                let entered =
                    match crate::engine_setup::open_elsewhere_now(state_root, &launch.session) {
                        Some(why) => Err(anyhow::anyhow!(why)),
                        None => enter_session_worktree(&launch.session, &mut next).await,
                    };
                match entered {
                    Ok(()) => *self.settings = next,
                    Err(error) => self.carry_on(
                        session_id,
                        format!(
                            "That session could not be resumed, so this one carries on: \
                             {error:#}."
                        ),
                    ),
                }
                // Whichever session runs, it takes up what the reader carried
                carried.resumed = Some(launch);
            }
            TuiExit::FixPullRequest(launch) => {
                // A new session, in the pull request's worktree; the one left stays as it was
                shutdown_workspace_services().await;
                let mut next = self.fresh(ResumeMode::New);
                match check_out_pull_request(launch.number, &mut next).await {
                    Ok(()) => {
                        *self.settings = next;
                        carried.fixing = Some(launch);
                    }
                    Err(error) => self.carry_on(
                        session_id,
                        format!(
                            "Pull request #{} could not be checked out, so this session carries \
                             on: {error:#}.",
                            launch.number
                        ),
                    ),
                }
            }
            TuiExit::OpenInEditor(path) => {
                let path = crate::config_editor::open_config_file(path).await?;
                tracing::info!(path = %path.display(), "editor closed");
                let config = luxe_core::config::Config::load(&self.settings.state_root);
                *self.show_dashboard = config.dashboard_shown();
            }
        }
        Ok(Flow::Again)
    }

    /// The settings for a session that starts over: none of the last one's startup notices,
    /// issue, pull request or worktree, and no dashboard in the way.
    fn fresh(
        &mut self,
        resume: crate::engine_setup::ResumeMode,
    ) -> crate::engine_setup::AgentSettings {
        *self.show_dashboard = false;
        crate::engine_setup::AgentSettings {
            resume,
            startup_notices: Vec::new(),
            issue: None,
            pull_request: None,
            worktree: None,
            ..self.settings.clone()
        }
    }

    /// Stays on the session that was running, saying why the other one did not start.
    fn carry_on(&mut self, session: luxe_core::SessionId, notice: String) {
        self.settings.resume = crate::engine_setup::ResumeMode::Session(session.0);
        self.settings.startup_notices = vec![notice];
    }
}

/// The dashboard's state: recent work, the sessions to resume, and the one line under it.
async fn open_dashboard(
    settings: &crate::engine_setup::AgentSettings,
) -> luxe_tui::dashboard::DashboardState {
    let store = crate::engine_setup::open_session_store(&settings.state_root);
    let data = crate::dashboard_data::collect(store, &settings.state_root).await;
    luxe_tui::dashboard::DashboardState::new(data.recent)
        .with_sessions(data.sessions)
        .with_worktree(settings.worktree.as_ref().map(|binding| binding.branch.clone()))
        .with_resume(data.resume)
        .with_briefing(data.briefing)
        .with_footer_note(footer_note(&settings.state_root))
}

/// What the status bar shows before the first turn.
///
/// The bar reports the model when there is one and stays quiet about it when there is not; the
/// provider and the window belong to whatever a turn would actually run on, which is the
/// stand-in's catalog metadata until something is chosen.
async fn starting_status(
    settings: &crate::engine_setup::AgentSettings,
) -> luxe_tui::statusline::StatusContext {
    let launch = settings.launch_model();
    crate::status_data::collect(
        &settings.work_root,
        settings.display_model(),
        launch.r#ref.provider.clone(),
        launch.context_window,
        settings.starting_thinking(),
        settings.auto_approve,
    )
    .await
}

/// Where the `/model` picker gets its list.
///
/// Rebuilt on every open, so an edited `models.json` or a models.dev refresh that landed
/// mid-session shows up without relaunching. The probe is the same menu built after asking every
/// connected local server what it is serving — a list that changes the moment somebody loads a
/// model, and that only the endpoint knows. An Azure resource is the same question asked of a
/// cloud: what is deployed there changes in a portal Luxe never sees. A server that is off leaves
/// the record standing.
fn model_menu_sources(
    settings: &crate::engine_setup::AgentSettings,
) -> luxe_tui::model_picker::ModelSources {
    let menu_settings = settings.clone();
    let build: luxe_tui::model_picker::ModelMenuSource =
        std::sync::Arc::new(move || crate::model_catalog::build_menu(&menu_settings));
    let probe_settings = settings.clone();
    let probe: luxe_tui::model_picker::ModelMenuProbe = std::sync::Arc::new(move || {
        let settings = probe_settings.clone();
        Box::pin(async move {
            let auth = luxe_providers::auth::ProviderAuth::new();
            tokio::join!(auth.refresh_local_models(), auth.refresh_deployments());
            crate::model_catalog::build_menu(&settings)
        })
    });
    luxe_tui::model_picker::ModelSources { build, probe: Some(probe) }
}

/// Where `/sessions` gets its list mid-session: the same catalog the start screen reads, read
/// again each time so it includes the sessions started since launch.
fn session_catalog_source(
    settings: &crate::engine_setup::AgentSettings,
) -> luxe_tui::dashboard::SessionCatalogSource {
    let state_root = settings.state_root.clone();
    std::sync::Arc::new(move || {
        let store = crate::engine_setup::open_session_store(&state_root);
        Box::pin(crate::dashboard_data::session_catalog(store))
    })
}

/// Builds the engine and applies the launch flags that are the engine's to carry.
async fn start_engine(
    settings: &crate::engine_setup::AgentSettings,
) -> anyhow::Result<crate::engine_setup::EngineHandle> {
    use luxe_core::engine::Action;

    let engine = crate::engine_setup::build_engine(
        settings,
        luxe_core::HookRegistry::new(),
        luxe_core::permission::AskResolution::Prompt,
    )
    .await?;
    if let Some(level) = settings.initial_thinking.filter(|_| settings.carryover.is_none()) {
        engine.actions.send(Action::SetThinkingLevel(level)).await.ok();
    }
    // Walk-away mode from launch: the engine's runtime override, the same one `/permissions on`
    // reaches, so the reader can turn it back off without restarting.
    if settings.auto_approve {
        engine.actions.send(Action::SetPermissionAutoApprove(true)).await.ok();
    }
    Ok(engine)
}

/// Shut down the process-wide warm LSP servers + live debug sessions. Bounded overall so a
/// misbehaving server can't wedge quit.
pub(super) async fn shutdown_workspace_services() {
    let shutdown = async {
        crate::engine_setup::build_lsp_service().shutdown_all().await;
        crate::engine_setup::shared_dap_service().terminate_all().await;
    };
    if tokio::time::timeout(std::time::Duration::from_secs(6), shutdown).await.is_err() {
        tracing::warn!("workspace service shutdown timed out; relying on kill-on-drop reaping");
    }
}

#[cfg(test)]
mod tests {
    use luxe_core::update::UpdateState;

    /// The start screen says only that an update exists and how to take it, and nothing when it
    /// does not; the versions and the command wait for `/update`.
    ///
    /// Mutates the environment, which nextest makes safe by giving each test its own process.
    #[test]
    fn the_footer_says_an_update_is_available_and_how() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cache = dir.path().join("update.json");
        std::env::set_var("LUXE_UPDATE_PATH", &cache);
        std::env::set_var("LUXE_UPDATE_CHECK", "1");
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap();
        for (latest, footer) in [("99.0.0", Some("Update available · /update")), ("0.0.1", None)] {
            let state = UpdateState { latest: Some(latest.into()), checked_at: now.as_secs() };
            state.store(&cache).expect("store");
            assert_eq!(super::footer_note(dir.path()).as_deref(), footer, "{latest}");
        }
        std::env::remove_var("LUXE_UPDATE_PATH");
        std::env::remove_var("LUXE_UPDATE_CHECK");
    }
}
