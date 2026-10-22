//! The model, thinking and theme pickers — opening them, previewing a theme, and applying a
//! choice.

use super::{App, ConvoBlock};
use crate::theme::styles::ThemeStyles;
use luxe_core::engine::Action;

impl App {
    /// Installs the browsable model catalog and records the active model's full info.
    ///
    /// Called once at startup by the TUI runner. The active info seeds the thinking picker's
    /// supported-level annotations and the model picker's "current" marker.
    pub fn set_model_catalog(
        &mut self,
        catalog: Vec<crate::model_picker::ModelEntry>,
        active: Option<luxe_core::provider::ModelInfo>,
    ) {
        self.model_catalog = catalog;
        if let Some(active) = active {
            self.active_model_info = Some(active);
        }
    }

    /// Opens the model picker over the current catalog.
    ///
    /// The catalog is rebuilt first when the runner supplied a source, so an edited `models.json`
    /// or a freshly-refreshed models.dev cache shows up without relaunching.
    ///
    /// A machine with no credential is sent to setup instead. The catalog is what the providers
    /// you configured can answer for, so with nothing configured there is nothing to choose
    /// between — and a list of models that cannot authenticate is worse than no list, because
    /// picking one looks like progress and fails on the first turn.
    pub fn open_model_picker(&mut self) {
        self.open_model_picker_at(crate::model_picker::PickerStart::Models);
    }

    /// The same, on the step `start` names: the provider in use's models, or the providers.
    pub fn open_model_picker_at(&mut self, start: crate::model_picker::PickerStart) {
        let active = self.active_model_info.as_ref().map(|info| info.r#ref.clone());
        self.open_model_picker_on(Vec::new(), active, |picker| picker.starting_on(start));
    }

    /// The same, on the models of `provider`, which a login just reached, with the cursor on the
    /// first and something to say first. Args: `(note, provider)`.
    ///
    /// Not the provider in use: a login can land on another, and its models are the ones the
    /// reader signed in for.
    pub(crate) fn open_model_picker_after_login(&mut self, note: Vec<String>, provider: &str) {
        let active = self.active_model_info.as_ref().map(|info| info.r#ref.clone());
        self.open_model_picker_on(note, active, |picker| picker.about_provider(provider));
    }

    /// Opens the picker with `active` marked, moved to its first step by `step`.
    /// Args: `(note, active, step)`.
    fn open_model_picker_on(
        &mut self,
        note: Vec<String>,
        active: Option<luxe_core::provider::ModelRef>,
        step: impl FnOnce(
            crate::model_picker::ModelPickerState,
        ) -> crate::model_picker::ModelPickerState,
    ) {
        if let Some(sources) = &self.model_sources {
            self.model_catalog = (sources.build)().catalog;
        }
        if self.model_catalog.is_empty() {
            self.explain_no_models();
            return;
        }
        let picker =
            crate::model_picker::ModelPickerState::open(self.model_catalog.clone(), active);
        self.overlays.model_picker = Some(step(picker).with_note(note));
        // Drawn from what is on disk, then corrected by whatever a local server says when it
        // answers. The picker is already on screen either way: a list that updates a moment late
        // beats a frame that waits on a socket.
        self.outbox.model_probe =
            self.model_sources.as_ref().is_some_and(|sources| sources.probe.is_some());
    }

    /// Says why there is no picker, and sends a machine with no provider to setup.
    fn explain_no_models(&mut self) {
        if self.provider_configured() {
            self.push_convo(ConvoBlock::System(
                "No models available to switch to — add models under \"models\" in your luxe \
                 config."
                    .to_string(),
            ));
            return;
        }
        self.push_convo(ConvoBlock::System(
            "No provider is configured, so there are no models to choose from yet.".to_string(),
        ));
        self.open_configuration();
    }

    /// Apply a catalog rebuilt off the frame, leaving an open picker where the reader had it.
    pub fn apply_model_catalog_refresh(&mut self, catalog: Vec<crate::model_picker::ModelEntry>) {
        if catalog == self.model_catalog {
            return;
        }
        self.model_catalog = catalog;
        if let Some(picker) = self.overlays.model_picker.as_mut() {
            picker.refresh(self.model_catalog.clone());
        }
    }

    /// Whether a turn has a model to run on.
    ///
    /// luxe chooses none on the reader's behalf, so a session can legitimately open with nothing
    /// selected — the dashboard's first screen is one of the places you pick. The engine refuses a
    /// turn either way ([`luxe_core::engine::EngineError::NoModel`]); this is what lets the UI ask
    /// before the reader's message leaves the composer.
    pub fn model_selected(&self) -> bool {
        !self.status.model.is_empty()
    }

    /// Opens the thinking-level picker, annotated against the active model when known.
    pub fn open_thinking_picker(&mut self) {
        self.overlays.thinking_picker = Some(crate::thinking_picker::ThinkingPickerState::open(
            self.status.thinking,
            self.active_model_info.clone(),
        ));
    }

    /// Opens the theme picker on whichever theme is currently resolved.
    ///
    /// Reads the themes again first: the picker is where a reader goes to find a theme file they
    /// just added or edited, and the guide promises it appears there.
    pub fn open_theme_picker(&mut self) {
        self.themes.reread();
        let active = self.config.resolved_theme_name(&self.themes);
        self.overlays.theme_picker =
            Some(crate::theme_picker::ThemePickerState::open(self.themes.names(), &active));
    }

    /// Repaints the whole app in `theme` without recording it anywhere.
    ///
    /// The preview is the picker's whole point — a palette cannot be judged from its name — and it
    /// deliberately leaves the config alone, so cancelling is one more call to this and nothing else.
    pub fn preview_theme(&mut self, theme: &str) {
        // Per-token `colors` overrides ride along, so a preview shows what this reader would
        // actually get rather than the theme's own idea of itself.
        let mut previewing = self.config.clone();
        previewing.theme = Some(theme.to_string());
        let resolved = previewing.resolved_theme(&self.themes);
        self.theme_styles = ThemeStyles::from_resolved(&resolved, theme);
    }

    /// Reads the desktop's new palette, and repaints in it when this session is following it.
    ///
    /// Following means no theme is set, or it is set to the desktop's by name. A reader who chose
    /// a theme keeps it, and one halfway through the picker keeps the preview they are judging,
    /// but both get the new palette should they pick the desktop's theme later.
    pub fn follow_desktop_theme(&mut self) {
        self.themes.reread();
        let following = self
            .config
            .theme
            .as_deref()
            .is_none_or(|name| name == luxe_core::omarchy::DESKTOP_THEME);
        if !following || self.overlays.theme_picker.is_some() {
            return;
        }
        let name = self.config.resolved_theme_name(&self.themes);
        self.theme_styles =
            ThemeStyles::from_resolved(&self.config.resolved_theme(&self.themes), &name);
        tracing::info!(theme = name, "followed the desktop's theme");
    }

    /// Keeps `theme`: applies it, holds it for this session, and records it for the next one.
    ///
    /// A failed write is said out loud rather than swallowed. The theme still changes, but a reader
    /// told it was saved who finds it gone next launch has been lied to.
    pub fn apply_theme_selection(&mut self, theme: &str) {
        self.preview_theme(theme);
        self.config.theme = Some(theme.to_string());
        if let Err(error) = luxe_core::config::Config::set_global(&["theme"], theme.into()) {
            tracing::warn!(theme, %error, "could not record the theme");
            self.push_convo(ConvoBlock::System(format!(
                "Switched to {theme} for this session. Writing it to your config failed: {error}"
            )));
        }
    }

    /// Switches the active model: updates the status/dock display optimistically and asks the
    /// engine to route future turns through it (`Action::SetModel`).
    pub fn apply_model_selection(&mut self, info: luxe_core::provider::ModelInfo) {
        self.status.model = info.display().to_string();
        self.status.provider = info.r#ref.provider.clone();
        if info.context_window > 0 {
            self.status.context_window = info.context_window;
        }
        self.active_model_info = Some(info.clone());
        self.send_action(Action::SetModel(info), "set the model");
    }

    /// Sets the active thinking level (`Action::SetThinkingLevel`). The engine's `StateChanged`
    /// reply corrects the display to the model's effective (clamped) level.
    pub fn apply_thinking_selection(&mut self, level: luxe_core::provider::ThinkingLevel) {
        self.status.thinking = level;
        self.send_action(Action::SetThinkingLevel(level), "set the thinking level");
    }
}
