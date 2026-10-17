//! Setup — the four questions a first run, and `luxe init`, ask: a look, a provider, the tools,
//! and the start screen.
//!
//! A stage of the Configuration surface rather than a surface of its own, because the provider
//! step *is* that surface's job: its sign-in rows open the same login wizard, drawn in the same
//! region, and a finished login hands to the same model picker. Everything else here only gathers
//! choices. Nothing is written until the last step — or until the reader skips, which writes a
//! file that chooses nothing, so the questions are asked once.
//!
//! Keys in, a [`ConfigOutcome`] out, like the rest of the surface; the app writes the file.

pub(super) mod render;

use crossterm::event::{KeyCode, KeyEvent};
use luxe_core::config::{Config, DashboardSetting, DashboardStyle};
use luxe_core::tool_catalog::CatalogTool;

use super::{ConfigChoice, ConfigOutcome};

/// Why setup is on screen, which decides where finishing it goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupMode {
    /// The first launch on a machine with no `config.json`: finishing lands on the start screen
    FirstRun,
    /// `luxe init`: finishing writes the file and leaves
    Init,
}

/// One step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Theme,
    Provider,
    Tools,
    Start,
}

impl Step {
    const ALL: [Self; 4] = [Self::Theme, Self::Provider, Self::Tools, Self::Start];

    fn label(self) -> &'static str {
        match self {
            Self::Theme => "Theme",
            Self::Provider => "Provider",
            Self::Tools => "Tools",
            Self::Start => "Start",
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|step| *step == self).unwrap_or(0)
    }
}

/// The start screen's three answers, in the order they are offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartScreen {
    Launcher,
    Minimal,
    Off,
}

impl StartScreen {
    const ALL: [Self; 3] = [Self::Launcher, Self::Minimal, Self::Off];

    fn of(config: &Config) -> Self {
        match (config.dashboard_shown(), config.dashboard_style()) {
            (false, _) => Self::Off,
            (true, DashboardStyle::Minimal) => Self::Minimal,
            _ => Self::Launcher,
        }
    }

    /// What `dashboard` should hold, or `None` for the default (the key removed).
    pub fn setting(self) -> Option<DashboardSetting> {
        match self {
            Self::Launcher => None,
            Self::Minimal => Some(DashboardSetting::Styled { style: DashboardStyle::Minimal }),
            Self::Off => Some(DashboardSetting::Enabled(false)),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Launcher => "The launcher",
            Self::Minimal => "Just the prompt",
            Self::Off => "Straight into a conversation",
        }
    }

    fn detail(self) -> &'static str {
        match self {
            Self::Launcher => "A prompt, with your sessions and commands listed under it",
            Self::Minimal => "The same prompt, with the list folded away until you press ↓",
            Self::Off => "No start screen: a new session every launch",
        }
    }
}

/// What the reader chose, for the app to write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupChoices {
    /// A theme by name, or `None` to follow the desktop (or take the default)
    pub theme: Option<String>,
    /// Tools to leave unregistered
    pub disabled_tools: Vec<String>,
    /// The start screen
    pub start: StartScreen,
}

/// The setup flow's state.
#[derive(Debug)]
pub struct SetupState {
    mode: SetupMode,
    step: Step,
    /// Every theme on offer, `None` standing for "follow the desktop" when there is one to follow
    themes: Vec<Option<String>>,
    theme: usize,
    /// The theme in force when setup opened, put back when it is skipped
    opened_with: String,
    /// The desktop's theme, which the "follow the desktop" row previews in
    desktop: Option<String>,
    /// The provider step's rows: three ways to sign in, then on
    provider: usize,
    tools: Vec<(&'static CatalogTool, bool)>,
    tool: usize,
    start: usize,
}

/// The provider step's sign-in rows; the step's last row moves on.
const SIGN_IN: [ConfigChoice; 3] =
    [ConfigChoice::Subscription, ConfigChoice::ApiKey, ConfigChoice::LocalOrCustom];

impl SetupState {
    /// Open on the first step, with every answer pre-set to what `config` already says.
    ///
    /// `themes` is every theme by name; `desktop` the one the desktop is wearing, when Luxe can
    /// follow one; `active` the theme in force right now.
    pub fn open(
        mode: SetupMode,
        config: &Config,
        themes: Vec<String>,
        desktop: Option<String>,
        active: String,
    ) -> Self {
        let mut offered: Vec<Option<String>> = Vec::new();
        if desktop.is_some() {
            offered.push(None);
        }
        offered.extend(themes.into_iter().map(Some));
        let theme = match config.theme.as_deref() {
            Some(name) => offered.iter().position(|offer| offer.as_deref() == Some(name)),
            // Nothing chosen yet: the desktop's when there is one, Luxe's own otherwise
            None if desktop.is_some() => Some(0),
            None => offered.iter().position(|offer| offer.as_deref() == Some("luxe")),
        }
        .unwrap_or(0);
        let tools = luxe_core::tool_catalog::toggleable()
            .into_iter()
            .map(|tool| (tool, config.tools.is_enabled(tool.name)))
            .collect();
        let start = StartScreen::ALL
            .iter()
            .position(|start| *start == StartScreen::of(config))
            .unwrap_or(0);
        Self {
            mode,
            step: Step::Theme,
            themes: offered,
            theme,
            opened_with: active,
            provider: 0,
            tools,
            tool: 0,
            start,
            desktop,
        }
    }

    /// Why setup is showing.
    pub fn mode(&self) -> SetupMode {
        self.mode
    }

    /// The step showing.
    pub fn step(&self) -> Step {
        self.step
    }

    /// The theme the cursor is on, by the name a preview repaints in.
    pub fn previewed_theme(&self) -> String {
        match self.themes.get(self.theme).cloned().flatten() {
            Some(name) => name,
            None => self.desktop.clone().unwrap_or_else(|| self.opened_with.clone()),
        }
    }

    /// The theme in force when setup opened.
    pub fn opened_with(&self) -> &str {
        &self.opened_with
    }

    /// What has been chosen so far.
    pub fn choices(&self) -> SetupChoices {
        SetupChoices {
            theme: self.themes.get(self.theme).cloned().flatten(),
            disabled_tools: self
                .tools
                .iter()
                .filter(|(_, enabled)| !enabled)
                .map(|(tool, _)| tool.name.to_string())
                .collect(),
            start: StartScreen::ALL[self.start],
        }
    }

    /// Handle a keypress. `configured` is whether a provider can answer a turn right now.
    pub fn on_key(&mut self, key: &KeyEvent, configured: bool) -> ConfigOutcome {
        let plain = key.modifiers.is_empty();
        match key.code {
            KeyCode::Char('s') if plain => ConfigOutcome::SkipSetup,
            KeyCode::Esc | KeyCode::Left => {
                match Step::ALL.get(self.step.index().wrapping_sub(1)) {
                    Some(previous) => {
                        self.step = *previous;
                        ConfigOutcome::Consumed
                    }
                    // Back from the first step is out
                    None if key.code == KeyCode::Esc => ConfigOutcome::SkipSetup,
                    None => ConfigOutcome::Consumed,
                }
            }
            KeyCode::Up | KeyCode::Char('k') if plain => self.step_cursor(-1),
            KeyCode::Down | KeyCode::Char('j') if plain => self.step_cursor(1),
            KeyCode::Char(' ') if self.step == Step::Tools => {
                if let Some((_, enabled)) = self.tools.get_mut(self.tool) {
                    *enabled = !*enabled;
                }
                ConfigOutcome::Consumed
            }
            KeyCode::Enter | KeyCode::Right => self.advance(configured),
            _ => ConfigOutcome::Consumed,
        }
    }

    /// Put the cursor on row `index` of the step showing (a click, or the pointer over it).
    pub fn select(&mut self, index: usize) -> ConfigOutcome {
        match self.step {
            Step::Theme if index < self.themes.len() => {
                self.theme = index;
                return ConfigOutcome::PreviewTheme(self.previewed_theme());
            }
            Step::Provider if index <= SIGN_IN.len() => self.provider = index,
            Step::Tools if index < self.tools.len() => self.tool = index,
            Step::Start if index < StartScreen::ALL.len() => self.start = index,
            _ => {}
        }
        ConfigOutcome::Consumed
    }

    /// The row under the cursor, as Enter does it (a click lands here too).
    pub fn activate(&mut self, configured: bool) -> ConfigOutcome {
        if self.step == Step::Tools {
            if let Some((_, enabled)) = self.tools.get_mut(self.tool) {
                *enabled = !*enabled;
            }
            return ConfigOutcome::Consumed;
        }
        self.advance(configured)
    }

    fn step_cursor(&mut self, delta: isize) -> ConfigOutcome {
        let (cursor, rows) = match self.step {
            Step::Theme => (&mut self.theme, self.themes.len()),
            Step::Provider => (&mut self.provider, SIGN_IN.len() + 1),
            Step::Tools => (&mut self.tool, self.tools.len()),
            Step::Start => (&mut self.start, StartScreen::ALL.len()),
        };
        *cursor = cursor.saturating_add_signed(delta).min(rows.saturating_sub(1));
        match self.step {
            Step::Theme => ConfigOutcome::PreviewTheme(self.previewed_theme()),
            _ => ConfigOutcome::Consumed,
        }
    }

    /// Enter: the provider step's sign-in rows open the wizard; everywhere else, on to the next
    /// step, and from the last one, done.
    fn advance(&mut self, configured: bool) -> ConfigOutcome {
        if self.step == Step::Provider {
            if let Some(choice) = SIGN_IN.get(self.provider) {
                return choice.scope().map_or(ConfigOutcome::Consumed, ConfigOutcome::SignIn);
            }
        }
        match Step::ALL.get(self.step.index() + 1) {
            Some(next) => {
                self.step = *next;
                // With a provider already answering, the step's likeliest answer is to move on
                if self.step == Step::Provider && configured {
                    self.provider = SIGN_IN.len();
                }
                ConfigOutcome::Consumed
            }
            None => ConfigOutcome::FinishSetup(self.choices()),
        }
    }
}

/// The scope a provider row signs in to, for the renderer and the tests.
pub(super) fn sign_in_rows() -> &'static [ConfigChoice] {
    &SIGN_IN
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;
    use luxe_core::auth::LoginScope;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn themes() -> Vec<String> {
        vec!["luxe".into(), "nord".into(), "gruvbox".into()]
    }

    fn fresh(desktop: Option<&str>) -> SetupState {
        SetupState::open(
            SetupMode::FirstRun,
            &Config::default(),
            themes(),
            desktop.map(str::to_string),
            "luxe".into(),
        )
    }

    /// Moving through the themes repaints the screen in each, and the one chosen is what is
    /// written; following the desktop is a choice to write nothing.
    #[test]
    fn the_theme_step_previews_as_it_moves_and_records_the_choice() {
        let mut setup = fresh(Some("tokyo-night"));
        assert_eq!(setup.choices().theme, None, "the desktop leads when there is one");
        assert_eq!(setup.previewed_theme(), "tokyo-night");
        assert_eq!(
            setup.on_key(&key(KeyCode::Down), false),
            ConfigOutcome::PreviewTheme("luxe".into())
        );
        setup.on_key(&key(KeyCode::Down), false);
        assert_eq!(setup.choices().theme.as_deref(), Some("nord"));

        let plain = fresh(None);
        assert_eq!(plain.choices().theme.as_deref(), Some("luxe"), "no desktop: Luxe's own");
    }

    #[test]
    fn enter_walks_the_steps_and_the_last_one_finishes() {
        let mut setup = fresh(None);
        setup.on_key(&key(KeyCode::Enter), true);
        assert_eq!(setup.step(), Step::Provider);
        // Past the sign-in rows to "continue"
        for _ in 0..3 {
            setup.on_key(&key(KeyCode::Down), true);
        }
        setup.on_key(&key(KeyCode::Enter), true);
        assert_eq!(setup.step(), Step::Tools);
        setup.on_key(&key(KeyCode::Enter), true);
        assert_eq!(setup.step(), Step::Start);
        match setup.on_key(&key(KeyCode::Enter), true) {
            ConfigOutcome::FinishSetup(choices) => {
                assert_eq!(choices.start, StartScreen::Launcher);
                assert!(choices.disabled_tools.is_empty(), "every tool stays on by default");
            }
            other => panic!("expected the end of setup, got {other:?}"),
        }
    }

    /// The sign-in rows are the Configuration surface's own, so each opens the wizard on the list
    /// it names.
    #[test]
    fn the_provider_step_signs_in_through_the_surfaces_own_wizard() {
        let mut setup = fresh(None);
        setup.on_key(&key(KeyCode::Enter), false);
        assert_eq!(
            setup.on_key(&key(KeyCode::Enter), false),
            ConfigOutcome::SignIn(LoginScope::Subscription)
        );
        setup.on_key(&key(KeyCode::Down), false);
        assert_eq!(
            setup.on_key(&key(KeyCode::Enter), false),
            ConfigOutcome::SignIn(LoginScope::ApiKey)
        );
    }

    /// A credential found in the environment means the provider step can be passed with Enter.
    #[test]
    fn with_a_provider_already_answering_enter_moves_past_the_provider_step() {
        let mut setup = fresh(None);
        setup.on_key(&key(KeyCode::Enter), true);
        assert_eq!(setup.step(), Step::Provider);
        setup.on_key(&key(KeyCode::Enter), true);
        assert_eq!(setup.step(), Step::Tools);
    }

    #[test]
    fn space_turns_a_tool_off_and_the_choice_names_it() {
        let mut setup = fresh(None);
        setup.on_key(&key(KeyCode::Enter), true);
        for _ in 0..3 {
            setup.on_key(&key(KeyCode::Down), true);
        }
        setup.on_key(&key(KeyCode::Enter), true);
        assert_eq!(setup.step(), Step::Tools);
        setup.on_key(&key(KeyCode::Char(' ')), true);
        let first = luxe_core::tool_catalog::toggleable()[0].name;
        assert_eq!(setup.choices().disabled_tools, vec![first.to_string()]);
    }

    /// Re-running `luxe init` opens on what the file already says, so finishing without touching
    /// anything changes nothing.
    #[test]
    fn every_answer_starts_from_the_config_in_force() {
        let config: Config = serde_json::from_str(
            r#"{"theme":"gruvbox","tools":{"disabled":["debug"]},"dashboard":{"style":"minimal"}}"#,
        )
        .unwrap();
        let setup = SetupState::open(SetupMode::Init, &config, themes(), None, "gruvbox".into());
        let choices = setup.choices();
        assert_eq!(choices.theme.as_deref(), Some("gruvbox"));
        assert_eq!(choices.disabled_tools, vec!["debug".to_string()]);
        assert_eq!(choices.start, StartScreen::Minimal);
    }

    #[test]
    fn s_skips_from_anywhere_and_esc_steps_back_until_it_leaves() {
        let mut setup = fresh(None);
        assert_eq!(setup.on_key(&key(KeyCode::Char('s')), false), ConfigOutcome::SkipSetup);
        setup.on_key(&key(KeyCode::Enter), false);
        assert_eq!(setup.on_key(&key(KeyCode::Esc), false), ConfigOutcome::Consumed);
        assert_eq!(setup.step(), Step::Theme);
        assert_eq!(setup.on_key(&key(KeyCode::Esc), false), ConfigOutcome::SkipSetup);
    }
}
