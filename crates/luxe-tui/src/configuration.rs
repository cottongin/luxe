//! Configuration — the surface that answers *how do I set luxe up?*
//!
//! One place for every way in: a subscription, a key, a server on this machine, or the files
//! themselves. It is what the dashboard's `Configuration` row opens, what `/config` reaches
//! mid-session, and where a credential-less first run is pointed.
//!
//! A **surface**, not an overlay. It draws the logotype and one panel under it, and the login
//! wizard and the model picker are drawn *inside* that panel's region — so setup never sits half on
//! top of a briefing the reader cannot use yet. Pure state plus rendering, in the same shape as the
//! overlays: keys in, a [`ConfigOutcome`] out, and the app owns what happens next.
//!
//! The menu asks for the **method** before the provider, which is the inverse of the wizard's own
//! order. `/login openai` knows its provider; someone who has just installed luxe knows they have a
//! ChatGPT plan. Each row hands the wizard a [`LoginScope`], so the list it opens is short and every
//! row in it starts its flow on Enter.

mod render;
mod settings;
pub mod setup;

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent};
use luxe_core::auth::{CredentialStatus, LoginScope, ProviderOption};

pub use render::{fits, frame, frame_with_width, render_banner, render_too_small, SurfaceFrame};
pub use settings::{Notice, SettingChange, SettingsState};
pub use setup::{SetupChoices, SetupMode, SetupState, StartScreen};

/// One row of the menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigChoice {
    /// Sign in to a plan the user already pays for.
    Subscription,
    /// Store a key the user already has.
    ApiKey,
    /// A local server, or an endpoint the user supplies.
    LocalOrCustom,
    /// The settings pages: appearance, the conversation, layout, tools and the agent.
    Settings,
    /// Open one of luxe's configuration files in an editor.
    EditFiles,
}

impl ConfigChoice {
    /// The rows, in the order they are offered — best first for someone with nothing configured.
    const ROWS: [Self; 5] =
        [Self::Subscription, Self::ApiKey, Self::LocalOrCustom, Self::Settings, Self::EditFiles];

    /// Row label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Subscription => "Sign in to a subscription",
            Self::ApiKey => "Add an API key",
            Self::LocalOrCustom => "Connect a local or custom model",
            Self::Settings => "Settings",
            Self::EditFiles => "Edit configuration files",
        }
    }

    /// The second line: which providers this actually reaches, so the row can be chosen on sight.
    pub fn detail(self) -> &'static str {
        match self {
            Self::Subscription => "ChatGPT Plus or Pro, GitHub Copilot, OpenRouter",
            Self::ApiKey => "OpenAI, Anthropic, Google, Groq, and a dozen others",
            Self::LocalOrCustom => "LM Studio, Ollama, Azure, any OpenAI-compatible endpoint",
            Self::Settings => "Appearance, the conversation, the dock, tools and the agent",
            Self::EditFiles => "Credentials, models, key bindings, and every setting",
        }
    }

    /// The login list this row opens, or `None` for the rows that open a page of the surface.
    pub fn scope(self) -> Option<LoginScope> {
        match self {
            Self::Subscription => Some(LoginScope::Subscription),
            Self::ApiKey => Some(LoginScope::ApiKey),
            Self::LocalOrCustom => Some(LoginScope::LocalOrCustom),
            Self::Settings | Self::EditFiles => None,
        }
    }
}

/// A configuration file the surface offers to open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigFile {
    /// File name, which is what a reader knows it by.
    pub name: &'static str,
    /// What lives in it.
    pub detail: &'static str,
    /// Where it is — resolved by the caller, because a credential store can be moved.
    pub path: PathBuf,
}

/// What a keypress asked the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigOutcome {
    /// Open the login wizard over this list of providers.
    SignIn(LoginScope),
    /// Hand this file to the user's editor, which means leaving the TUI.
    EditFile(PathBuf),
    /// Write this change to the global config, then hand the settings fresh layers.
    Write(SettingChange),
    /// Open the theme picker over the surface.
    PickTheme,
    /// Open the settings pages, which need the files read first.
    OpenSettings,
    /// Repaint in this theme without recording it, as setup's theme step moves.
    PreviewTheme(String),
    /// Setup is done: write these choices.
    FinishSetup(SetupChoices),
    /// Setup was skipped: write a file that chooses nothing, so it is not asked again.
    SkipSetup,
    /// Leave the surface.
    Close,
    /// Key consumed; the surface stays as it is.
    Consumed,
}

/// Which step the surface is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// The four ways in.
    Menu,
    /// The configuration files, one of which is about to be opened.
    Files,
    /// The settings pages.
    Settings,
    /// The setup flow.
    Setup,
}

/// The surface's state.
#[derive(Debug)]
pub struct ConfigurationState {
    stage: Stage,
    selected: usize,
    file_selected: usize,
    files: Vec<ConfigFile>,
    /// Providers that could authenticate a turn right now, as `OpenAI (API key)`.
    configured: Vec<String>,
    /// The settings pages, once the `Settings` row has been opened
    settings: Option<SettingsState>,
    /// The setup flow, on a first run or under `luxe init`
    setup: Option<SetupState>,
    /// The last frame was too small to draw the surface, so only esc may act
    undrawn: std::cell::Cell<bool>,
}

impl ConfigurationState {
    /// Open on the menu.
    ///
    /// `providers` is the auth service's view of the world, kept only as the summary line — the
    /// surface never holds a credential, and the lists it opens are built fresh when a row is
    /// chosen, so a login made in this session is reflected the next time a row is opened.
    pub fn open(providers: &[ProviderOption], files: Vec<ConfigFile>) -> Self {
        Self {
            stage: Stage::Menu,
            selected: 0,
            file_selected: 0,
            files,
            configured: providers.iter().filter_map(configured_label).collect(),
            settings: None,
            setup: None,
            undrawn: std::cell::Cell::new(false),
        }
    }

    /// Records whether the frame drew the surface or only its "resize" notice, whose keys are
    /// held while it shows: the settings pages', the menu's and setup's alike.
    pub(crate) fn note_drawn(&self, drawn: bool) {
        self.undrawn.set(!drawn);
        if !drawn {
            self.settings.as_ref().inspect(|settings| settings.mark_undrawn());
        }
    }

    /// Show the setup flow instead of the menu.
    pub fn show_setup(&mut self, setup: SetupState) {
        self.setup = Some(setup);
        self.stage = Stage::Setup;
    }

    /// The setup flow, when it is showing.
    pub fn setup(&self) -> Option<&SetupState> {
        self.setup.as_ref().filter(|_| self.stage == Stage::Setup)
    }

    /// What can answer a turn right now, as `OpenAI (API key)`.
    pub fn configured(&self) -> &[String] {
        &self.configured
    }

    /// Show the settings pages, the way `/settings` and the menu's `Settings` row both do.
    ///
    /// Esc from them comes back to the menu, so the rest of the surface is a step away.
    pub fn show_settings(&mut self, settings: SettingsState) {
        self.settings = Some(settings);
        self.stage = Stage::Settings;
        self.selected = ConfigChoice::ROWS
            .iter()
            .position(|choice| *choice == ConfigChoice::Settings)
            .unwrap_or(self.selected);
    }

    /// The settings pages, when they have been opened.
    pub fn settings(&self) -> Option<&SettingsState> {
        self.settings.as_ref()
    }

    /// The settings pages, to hand them fresh layers or a notice.
    pub fn settings_mut(&mut self) -> Option<&mut SettingsState> {
        self.settings.as_mut()
    }

    /// Whether the settings pages are what is showing.
    pub fn showing_settings(&self) -> bool {
        self.stage == Stage::Settings && self.settings.is_some()
    }

    /// Re-read what is configured, leaving the reader's place in the menu alone.
    pub fn refresh(&mut self, providers: &[ProviderOption]) {
        self.configured = providers.iter().filter_map(configured_label).collect();
    }

    /// Whether anything can authenticate a turn right now.
    pub fn is_configured(&self) -> bool {
        !self.configured.is_empty()
    }

    /// The rows on offer.
    pub fn choices(&self) -> &'static [ConfigChoice] {
        &ConfigChoice::ROWS
    }

    /// Highlighted row in whichever list is showing.
    pub fn selected_row(&self) -> usize {
        match self.stage {
            Stage::Menu => self.selected,
            Stage::Files => self.file_selected,
            Stage::Settings => self.settings.as_ref().map_or(0, SettingsState::selected),
            Stage::Setup => 0,
        }
    }

    /// The cursor and the row count of whichever list is showing.
    pub(crate) fn list_cursor(&self) -> (usize, usize) {
        match self.stage {
            Stage::Menu => (self.selected, ConfigChoice::ROWS.len()),
            Stage::Files => (self.file_selected, self.files.len()),
            Stage::Settings => self
                .settings
                .as_ref()
                .map_or((0, 0), |settings| (settings.selected(), settings.selectable_count())),
            Stage::Setup => self.setup.as_ref().map_or((0, 0), SetupState::step_list),
        }
    }

    /// The configuration files on offer.
    pub fn files(&self) -> &[ConfigFile] {
        &self.files
    }

    /// Whether the file list is showing rather than the menu.
    pub fn showing_files(&self) -> bool {
        self.stage == Stage::Files
    }

    /// Handle a keypress.
    pub fn on_key(&mut self, key: &KeyEvent) -> ConfigOutcome {
        // nothing but the notice is drawn, so nothing under it may change
        if self.undrawn.get() && key.code != KeyCode::Esc {
            return ConfigOutcome::Consumed;
        }
        if self.stage == Stage::Setup {
            let configured = !self.configured.is_empty();
            if let Some(setup) = self.setup.as_mut() {
                return setup.on_key(key, configured);
            }
        }
        if self.stage == Stage::Settings {
            if let Some(settings) = self.settings.as_mut() {
                // Esc out of a search is the settings' own; out of the pages, back to the menu
                if key.code == KeyCode::Esc && !settings.searching() {
                    self.stage = Stage::Menu;
                    return ConfigOutcome::Consumed;
                }
                return settings.on_key(key);
            }
        }
        let rows = self.rows();
        let plain = key.modifiers.is_empty();
        match key.code {
            // Esc steps back exactly one: out of the file list to the menu, out of the menu to
            // wherever the surface was opened from.
            KeyCode::Esc | KeyCode::Left if self.stage == Stage::Files => {
                self.stage = Stage::Menu;
                ConfigOutcome::Consumed
            }
            KeyCode::Esc => ConfigOutcome::Close,
            KeyCode::Up | KeyCode::Char('k') if plain => {
                self.move_selection(-1);
                ConfigOutcome::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') if plain => {
                self.move_selection(1);
                ConfigOutcome::Consumed
            }
            KeyCode::Enter | KeyCode::Right => self.activate(),
            // The rows are numbered on screen, so the numbers have to work.
            KeyCode::Char(digit) if plain && digit.is_ascii_digit() => {
                let row = (digit as usize).wrapping_sub('1' as usize);
                match row < rows {
                    true => {
                        self.select(row);
                        self.activate()
                    }
                    false => ConfigOutcome::Consumed,
                }
            }
            _ => ConfigOutcome::Consumed,
        }
    }

    /// Run the highlighted row.
    pub fn activate(&mut self) -> ConfigOutcome {
        match self.stage {
            Stage::Menu => match ConfigChoice::ROWS.get(self.selected).copied() {
                Some(choice) => match choice.scope() {
                    Some(scope) => ConfigOutcome::SignIn(scope),
                    None if choice == ConfigChoice::Settings => ConfigOutcome::OpenSettings,
                    None => {
                        self.stage = Stage::Files;
                        ConfigOutcome::Consumed
                    }
                },
                None => ConfigOutcome::Consumed,
            },
            Stage::Settings => match self.settings.as_mut() {
                Some(settings) => settings.activate(),
                None => ConfigOutcome::Consumed,
            },
            Stage::Setup => {
                let configured = !self.configured.is_empty();
                match self.setup.as_mut() {
                    Some(setup) => setup.activate(configured),
                    None => ConfigOutcome::Consumed,
                }
            }
            Stage::Files => match self.files.get(self.file_selected) {
                Some(file) => ConfigOutcome::EditFile(file.path.clone()),
                None => ConfigOutcome::Consumed,
            },
        }
    }

    /// Move the cursor to `row` (a click, or a numbered shortcut), clamped.
    pub fn select(&mut self, row: usize) {
        let last = self.rows().saturating_sub(1);
        match self.stage {
            Stage::Menu => self.selected = row.min(last),
            Stage::Files => self.file_selected = row.min(last),
            Stage::Settings => {
                if let Some(settings) = self.settings.as_mut() {
                    settings.select(row);
                }
            }
            Stage::Setup => {
                if let Some(setup) = self.setup.as_mut() {
                    setup.select(row);
                }
            }
        }
    }

    /// The summary line: what is configured, or that nothing is.
    ///
    /// "Configured", not "signed in": one of these rows can be a local server, which is connected
    /// rather than signed in to, and a line naming several of them has only one verb to spend.
    fn summary(&self) -> String {
        match self.configured.is_empty() {
            true => "No provider is configured yet.".to_string(),
            false => format!("Configured: {}", self.configured.join(" · ")),
        }
    }

    /// Rows in the list currently showing.
    fn rows(&self) -> usize {
        match self.stage {
            Stage::Menu => ConfigChoice::ROWS.len(),
            Stage::Files => self.files.len(),
            Stage::Settings | Stage::Setup => usize::MAX,
        }
    }

    fn move_selection(&mut self, delta: isize) {
        let last = self.rows().saturating_sub(1);
        let cursor = match self.stage {
            Stage::Menu | Stage::Settings | Stage::Setup => &mut self.selected,
            Stage::Files => &mut self.file_selected,
        };
        *cursor = match delta {
            d if d < 0 => cursor.saturating_sub(1),
            _ => (*cursor + 1).min(last),
        };
    }
}

/// How a configured provider reads in the summary, or `None` when it has no credential.
///
/// Names where the credential came from, because "signed in" and "there is a variable in your
/// shell" fail in different ways and the reader is the one who has to tell them apart.
fn configured_label(option: &ProviderOption) -> Option<String> {
    let how = match &option.status {
        CredentialStatus::Missing => return None,
        CredentialStatus::Env(name) => name.clone(),
        CredentialStatus::Config(file) => file.clone(),
        CredentialStatus::Local(endpoint) => format!("local — {endpoint}"),
        CredentialStatus::Stored(info) => info.kind.label().to_string(),
    };
    Some(format!("{} ({how})", option.name))
}

#[cfg(test)]
mod tests;
