//! Settings, applied — where the Configuration surface's changes reach the reader's file.
//!
//! The surface is pure: it asks for a [`SettingChange`] and draws whatever [`ConfigLayers`] it is
//! handed. This is the other half, and the only place a settings change touches disk: write the
//! one key, reload what reloads live, and give the surface the files as they are now.

use luxe_core::config::{Config, ConfigLayers};

use super::App;
use crate::configuration::{Notice, SettingChange, SettingsState};

impl App {
    /// Open the Configuration surface straight onto its settings pages (`/settings`).
    pub fn open_settings(&mut self) {
        self.open_configuration();
        self.show_settings();
    }

    /// Put the settings pages up on the Configuration surface that is already open.
    pub(crate) fn show_settings(&mut self) {
        let settings = SettingsState::open(
            ConfigLayers::read(&self.settings_root()),
            self.resolved_theme_name(),
            Config::global_path(),
        );
        if let Some(configuration) = self.overlays.configuration.as_mut() {
            configuration.show_settings(settings);
        }
    }

    /// Write `change`, apply it where it applies live, and tell the surface how it went.
    ///
    /// A failed write is said where the reader is looking, and nothing is reloaded: the file is
    /// what it was, so the surface keeps showing what is actually in force.
    pub(crate) fn apply_setting_change(&mut self, change: SettingChange) {
        let applies = change.setting().map(|setting| setting.applies);
        let written = match &change {
            SettingChange::Set { path, value } => Config::set_global(path, value.clone()),
            SettingChange::Reset { path } => Config::unset_global(path),
            SettingChange::Tool { name, enabled } => {
                Config::set_tool_enabled_globally(name, *enabled)
            }
        };
        let notice = match written {
            Ok(path) => {
                let root = self.settings_root();
                super::commands::reload_config(self, &root);
                if let SettingChange::Set { path: ["mouse"], value } = &change {
                    self.mouse_enabled = value.as_bool().unwrap_or(true);
                } else if let SettingChange::Reset { path: ["mouse"] } = &change {
                    self.mouse_enabled = true;
                }
                let when = applies.unwrap_or(luxe_core::settings::Applies::NextLaunch).note();
                Notice {
                    text: format!("Saved to {}. {when}.", luxe_core::paths::home_relative(&path)),
                    warning: false,
                }
            }
            Err(error) => {
                tracing::warn!(%error, "could not write a setting");
                Notice { text: format!("Not saved: {error}"), warning: true }
            }
        };
        self.refresh_settings();
        if let Some(settings) = self.overlays.configuration.as_mut().and_then(|c| c.settings_mut())
        {
            settings.set_notice(notice);
        }
    }

    /// Give the settings pages the files as they are now, when they are open.
    pub(crate) fn refresh_settings(&mut self) {
        if self.overlays.configuration.as_ref().and_then(|c| c.settings()).is_none() {
            return;
        }
        let layers = ConfigLayers::read(&self.settings_root());
        let theme = self.resolved_theme_name();
        if let Some(settings) = self.overlays.configuration.as_mut().and_then(|c| c.settings_mut())
        {
            settings.refresh(layers, theme);
        }
    }

    fn settings_root(&self) -> std::path::PathBuf {
        self.project_root.clone().unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
    }

    fn resolved_theme_name(&self) -> String {
        self.config.resolved_theme_name(&self.themes)
    }
}

#[cfg(test)]
mod tests {
    use crate::configuration::ConfigOutcome;
    use crate::fixture::{seeded_app, FixtureScenario};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::{backend::TestBackend, Terminal};

    /// A terminal too small for the whole surface never draws the panel, and its controls still
    /// change nothing; esc still leaves, and the controls answer again once it fits.
    ///
    /// The break this catches: only the panel's own renderer set the guard, so below the surface's
    /// minimum it kept whatever it last was and Enter wrote a setting nobody could see.
    #[test]
    fn settings_below_the_surface_minimum_hold_their_keys() {
        let mut app = seeded_app("nord", FixtureScenario::Settings).expect("fixture");
        let press = |app: &mut crate::app::App, (width, height): (u16, u16), code| {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
            terminal.draw(|f| crate::view::render(f, app)).expect("draw");
            let configuration = app.overlays.configuration.as_mut().expect("the surface is open");
            configuration.on_key(&KeyEvent::new(code, KeyModifiers::NONE))
        };
        assert_eq!(press(&mut app, (30, 8), KeyCode::Enter), ConfigOutcome::Consumed);
        let fits = press(&mut app, (120, 40), KeyCode::Enter);
        assert_ne!(fits, ConfigOutcome::Consumed, "the row acts once the panel is drawn");
        press(&mut app, (30, 8), KeyCode::Esc);
        let configuration = app.overlays.configuration.as_ref().expect("the surface is open");
        assert!(!configuration.showing_settings(), "esc leaves the pages");
    }
}
