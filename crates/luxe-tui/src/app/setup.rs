//! Setup, applied — the first run's questions and `luxe init`'s, and the file they write.
//!
//! The flow itself is a stage of the Configuration surface ([`crate::configuration::setup`]); this
//! is where its answers reach `config.json`, one key at a time, so a re-run over a file the reader
//! has edited by hand changes only what was asked.

use luxe_core::config::{Config, ConfigWriteError};

use super::{App, ConvoBlock};
use crate::configuration::{SetupChoices, SetupMode, SetupState};

impl App {
    /// Open the setup flow, on the Configuration surface.
    pub fn open_setup(&mut self, mode: SetupMode) {
        let desktop = self.themes.canonical(luxe_core::omarchy::DESKTOP_THEME).map(str::to_string);
        let active = self.config.resolved_theme_name(&self.themes);
        let setup = SetupState::open(mode, &self.config, self.themes.names(), desktop, active);
        self.open_configuration();
        if let Some(configuration) = self.overlays.configuration.as_mut() {
            configuration.show_setup(setup);
        }
    }

    /// Whether the setup flow is what is showing.
    pub fn setup_showing(&self) -> bool {
        self.overlays
            .configuration
            .as_ref()
            .is_some_and(|configuration| configuration.setup().is_some())
    }

    /// Repaint in whatever theme setup's cursor is on, so a theme is judged by looking at it.
    pub(crate) fn preview_setup_theme(&mut self) {
        let theme = self
            .overlays
            .configuration
            .as_ref()
            .and_then(|c| c.setup())
            .map(SetupState::previewed_theme);
        if let Some(theme) = theme {
            self.preview_theme(&theme);
        }
    }

    /// Write what setup gathered, and go where finishing goes: the start screen on a first run,
    /// out of Luxe under `luxe init`.
    pub(crate) fn finish_setup(&mut self, choices: SetupChoices) {
        let mode = self.setup_mode();
        if let Err(error) = write_choices(&choices) {
            tracing::warn!(%error, "could not write the setup choices");
            self.push_convo(ConvoBlock::System(format!("Setup was not saved: {error}")));
        }
        let root = self.project_root.clone().unwrap_or_default();
        super::commands::reload_config(self, &root);
        self.leave_setup(mode);
    }

    /// Leave setup without choosing, writing a file that says nothing so it is not asked again.
    pub(crate) fn skip_setup(&mut self) {
        let mode = self.setup_mode();
        let opened_with = self
            .overlays
            .configuration
            .as_ref()
            .and_then(|c| c.setup())
            .map(|setup| setup.opened_with().to_string());
        if let Some(theme) = opened_with {
            self.preview_theme(&theme);
        }
        if let Err(error) = Config::ensure_global_file() {
            tracing::warn!(%error, "could not write a config file on skipping setup");
        }
        self.leave_setup(mode);
    }

    fn setup_mode(&self) -> SetupMode {
        self.overlays
            .configuration
            .as_ref()
            .and_then(|c| c.setup())
            .map_or(SetupMode::FirstRun, SetupState::mode)
    }

    fn leave_setup(&mut self, mode: SetupMode) {
        match mode {
            SetupMode::Init => {
                self.overlays.configuration = None;
                self.should_quit = true;
            }
            SetupMode::FirstRun => {
                self.close_configuration();
                let style = self.config.dashboard_style();
                if let Some(dashboard) = self.dashboard.as_mut() {
                    dashboard.set_style(style);
                }
            }
        }
    }
}

/// Each answer as one key: set when it differs from the default, removed when it is the default,
/// so the file says what was chosen and nothing more.
fn write_choices(choices: &SetupChoices) -> Result<(), ConfigWriteError> {
    Config::ensure_global_file()
        .map_err(|error| ConfigWriteError::Io { path: Config::global_path(), error })?;
    match &choices.theme {
        Some(theme) => Config::set_global(&["theme"], theme.as_str().into())?,
        None => Config::unset_global(&["theme"])?,
    };
    match choices.disabled_tools.is_empty() {
        true => Config::unset_global(&["tools", "disabled"])?,
        false => Config::set_global(&["tools", "disabled"], choices.disabled_tools.clone().into())?,
    };
    match choices.start.setting() {
        None => Config::unset_global(&["dashboard"])?,
        Some(setting) => Config::set_global(
            &["dashboard"],
            serde_json::to_value(setting).unwrap_or(serde_json::Value::Bool(true)),
        )?,
    };
    Ok(())
}
