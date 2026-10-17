//! App configuration — the Configuration surface, and what applying its outcomes does.

use super::*;

impl App {
    /// Transition out of the dashboard in response to a user action.
    pub fn handle_dashboard_outcome(&mut self, outcome: crate::dashboard::DashOutcome) {
        navigation::apply_dashboard_outcome(self, outcome);
    }

    /// Opens the list of instruction files a turn would read, over whatever asked for it.
    ///
    /// Built fresh each time rather than snapshotted: writing an `AGENTS.md` in the editor this
    /// surface opened must change the list you come back to, and the walk is a handful of `stat`
    /// calls.
    pub fn open_context_files(&mut self) {
        let cwd = std::env::current_dir().unwrap_or_default();
        self.overlays.context_files = Some(crate::context_files::ContextFilesState::open(&cwd));
    }

    /// Opens the Configuration surface over whatever is showing.
    ///
    /// Rebuilt from the auth service every time, so a login made a moment ago is reflected in the
    /// summary rather than remembered from when the app started.
    pub fn open_configuration(&mut self) {
        let providers =
            self.auth_service.as_ref().map(|service| service.providers()).unwrap_or_default();
        self.overlays.configuration =
            Some(crate::configuration::ConfigurationState::open(&providers, self.config_files()));
        self.current_view = ViewMode::Configuration;
    }

    /// Closes the Configuration surface, returning to whatever it was opened from.
    ///
    /// The dashboard is rebuilt against the current credential state on the way out, so a provider
    /// configured here stops the menu leading with `Set up a provider`.
    pub fn close_configuration(&mut self) {
        self.overlays.configuration = None;
        self.close_auth_wizard();
        self.overlays.model_picker = None;
        let configured = self.provider_configured();
        self.set_provider_configured(configured);
        self.current_view = match self.dashboard.is_some() {
            true => ViewMode::Dashboard,
            false => ViewMode::Chat,
        };
    }

    /// Tells the dashboard whether a turn could authenticate right now.
    ///
    /// The menu leads with setup when it could not, which is the whole first-run path: everyone
    /// gets the same dashboard, and a machine with no credential gets it pointing at the fix.
    pub fn set_provider_configured(&mut self, configured: bool) {
        if let Some(dashboard) = self.dashboard.as_mut() {
            dashboard.set_provider_configured(configured);
        }
    }

    /// Apply what a Configuration keypress asked for. `EditFile` is the caller's: it ends the TUI.
    pub fn apply_configuration_outcome(&mut self, outcome: crate::configuration::ConfigOutcome) {
        self.apply_configuration_outcome_inner(outcome);
        // Setup's theme step repaints the whole app in the row the cursor is on, whichever way
        // the cursor got there — a key, a click, or a step back onto it
        if self.setup_showing() {
            self.preview_setup_theme();
        }
    }

    pub(super) fn apply_configuration_outcome_inner(
        &mut self,
        outcome: crate::configuration::ConfigOutcome,
    ) {
        use crate::configuration::ConfigOutcome;
        match outcome {
            ConfigOutcome::SignIn(scope) => self.open_auth_wizard_scoped(None, scope),
            ConfigOutcome::Close => self.close_configuration(),
            ConfigOutcome::OpenSettings => self.show_settings(),
            ConfigOutcome::Write(change) => self.apply_setting_change(change),
            ConfigOutcome::PickTheme => self.open_theme_picker(),
            ConfigOutcome::PreviewTheme(_) => {}
            ConfigOutcome::FinishSetup(choices) => self.finish_setup(choices),
            ConfigOutcome::SkipSetup => self.skip_setup(),
            // The editor handoff leaves the TUI, so the surface's owner drives it.
            ConfigOutcome::EditFile(_) | ConfigOutcome::Consumed => {}
        }
    }

    /// The configuration files the surface offers to open.
    ///
    /// The credential store's path comes from the service, which is the thing that actually reads
    /// it; the rest are where this build looks for them.
    pub(super) fn config_files(&self) -> Vec<crate::configuration::ConfigFile> {
        use crate::configuration::ConfigFile;
        let directory = luxe_core::user_dirs::luxe_config_dir();
        let auth = self
            .auth_service
            .as_ref()
            .map(|service| service.store_path())
            .unwrap_or_else(|| luxe_core::user_dirs::luxe_config_file("auth.json"));
        vec![
            ConfigFile {
                name: "config.json",
                detail: "Theme, dock, statusline, and the rest of the interface",
                path: luxe_core::config::Config::global_path(),
            },
            ConfigFile {
                name: "models.json",
                detail: "Providers, models, and which one Luxe starts on",
                path: luxe_core::user_dirs::luxe_config_file("models.json"),
            },
            ConfigFile { name: "auth.json", detail: "Credentials — written 0600", path: auth },
            ConfigFile {
                name: "keybindings.json",
                detail: "Key bindings, by action id",
                path: directory.join("keybindings.json"),
            },
        ]
    }
}
