//! Model picker — the searchable overlay behind `/model`, the palette's "Change model" and the
//! dock's provider and model rows.
//!
//! Pure state + rendering. The CLI assembles the [`ModelEntry`] catalog (configured models from
//! `models.json`, enriched/extended by a cached models.dev catalog); this module only filters,
//! renders, and returns the chosen [`ModelInfo`]. Selecting one sends `Action::SetModel`.
//!
//! Two steps, so choosing who serves a model and choosing the model are separate questions. The
//! model step lists the models of the provider in use, and `tab` widens it to every provider. The
//! provider step (`←`, or the dock's provider row) lists providers: choosing one that serves the
//! model in use switches to it there, and any other opens the model step on its models.

use crate::picker::PickerNav;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use luxe_core::provider::{ModelInfo, ModelRef};

mod meta;
mod providers;
mod render;
use providers::ProviderStep;

/// The model catalog + active model the TUI runner installs at startup.
#[derive(Debug, Clone, Default)]
pub struct ModelMenu {
    /// Browsable models (configured + models.dev extras).
    pub catalog: Vec<ModelEntry>,
    /// Full info for the model the engine starts on, when known.
    pub active: Option<ModelInfo>,
}

/// Builds the menu on demand.
///
/// The CLI owns config and catalog files, so it supplies this instead of a fixed snapshot; the
/// picker calls it each time it opens, which is what lets an edited `models.json` or a
/// freshly-refreshed models.dev cache show up without relaunching.
pub type ModelMenuSource = std::sync::Arc<dyn Fn() -> ModelMenu + Send + Sync>;

/// Builds the menu again, having asked anything that can be asked first.
///
/// [`ModelMenuSource`] reads files and must not block a frame; this one is allowed to make a
/// request, so the UI spawns it and applies the answer when it lands. What it exists for is a
/// local server: its model list is a fact about this machine a moment ago, and a model loaded in
/// LM Studio after the last connect would otherwise never appear.
pub type ModelMenuProbe = std::sync::Arc<
    dyn Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = ModelMenu> + Send>>
        + Send
        + Sync,
>;

/// Both ways the runner can rebuild the menu, as one thing to hand over.
///
/// They are never useful apart — the probe is the same menu with a question asked first — and
/// passing them separately is two more arguments on a function that already has enough.
#[derive(Clone)]
pub struct ModelSources {
    /// Rebuilds from what is on disk. Cheap enough to run while a frame is being laid out.
    pub build: ModelMenuSource,
    /// Rebuilds it again off the frame, having asked a local server what it serves.
    pub probe: Option<ModelMenuProbe>,
}

impl ModelSources {
    /// Sources that read what is on disk, with nothing to ask.
    pub fn reading(build: ModelMenuSource) -> Self {
        Self { build, probe: None }
    }
}

/// One selectable model row.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelEntry {
    /// Full model info sent to the engine on selection.
    pub info: ModelInfo,
    /// What the row calls it: the name the user gave it in config, else the model id.
    pub label: String,
    /// Whether this model is declared in the user's config (vs. a models.dev browse extra).
    pub configured: bool,
    /// Whether this is a catalog guess for an endpoint that can say what it serves, and did not.
    ///
    /// An Azure resource lists its deployments; until it has, the catalog's models are Azure's
    /// offer rather than the resource's, and the row says so instead of passing for one.
    pub unverified: bool,
}

impl ModelEntry {
    /// `provider/id` — the stable key shown as a subtitle and searched against.
    pub fn qualified_id(&self) -> String {
        format!("{}/{}", self.info.r#ref.provider, self.info.r#ref.id)
    }
}

/// Which step the model picker opens on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerStart {
    /// The providers, for the dock's provider row
    Providers,
    /// The models of the provider in use, for everything else
    Models,
}

/// Outcome of a model-picker keypress.
#[derive(Clone, Debug, PartialEq)]
pub enum ModelPickerOutcome {
    /// Switch to this model and close the picker.
    Select(Box<ModelInfo>),
    /// Close without switching.
    Close,
    /// Key consumed; picker stays open.
    Consumed,
}

/// Pure state machine for the model picker overlay.
#[derive(Debug, Clone)]
pub struct ModelPickerState {
    /// Every model in the catalog, whoever serves it
    all: Vec<ModelEntry>,
    /// The models on show: `all`, or the scoped provider's share of it
    items: Vec<ModelEntry>,
    nav: PickerNav,
    active: Option<ModelRef>,
    note: Vec<String>,
    /// The provider the model step is about, when it serves anything
    scope: Option<String>,
    /// Whether `tab` widened the model step past [`Self::scope`] to every provider
    widened: bool,
    /// The provider step, while it is the one showing
    providers: Option<ProviderStep>,
    /// Whether the model step was reached from the provider step, which `esc` returns to
    from_providers: bool,
}

impl ModelPickerState {
    /// Open the picker over a catalog, marking `active` as the current model.
    ///
    /// Opens on every model; [`Self::starting_on`] narrows it to the provider in use, or opens on
    /// the providers.
    pub fn open(items: Vec<ModelEntry>, active: Option<ModelRef>) -> Self {
        let nav = PickerNav::new(items.len());
        let (all, note) = (items.clone(), Vec::new());
        let mut picker = Self {
            all,
            items,
            nav,
            active,
            note,
            scope: None,
            widened: false,
            providers: None,
            from_providers: false,
        };
        // Start on the active model when it's in the list, so the current choice is preselected.
        // Nothing is filtered yet, so an item's index is also its row.
        let active = picker.active.as_ref();
        if let Some(row) =
            active.and_then(|it| picker.items.iter().position(|entry| &entry.info.r#ref == it))
        {
            picker.nav.select(row);
        }
        picker
    }

    /// Moves to the step `start` names: the provider in use's models, or the providers.
    pub fn starting_on(mut self, start: PickerStart) -> Self {
        let provider = self.active.as_ref().map(|active| active.provider.clone());
        match start {
            PickerStart::Models => self.show(provider, false),
            PickerStart::Providers => self.show_providers(),
        }
        self
    }

    /// Moves to `provider`'s models with the cursor on the first, for a login that just reached
    /// them.
    ///
    /// Those are the models the reader signed in for, whichever model is in use. The top row,
    /// because a cursor resting on one reads as luxe recommending it. A local server's models
    /// arrive when it answers, so until then the list is everything and [`Self::refresh`] narrows it.
    pub fn about_provider(mut self, provider: &str) -> Self {
        self.show(Some(provider.to_owned()), false);
        self.nav.select(0);
        self
    }

    /// Opens with something to say first — what a login landed, and which model it left you on.
    ///
    /// A successful sign-in walks straight into this picker rather than pausing on a screen that
    /// only reports success, so the report rides along here. Each line is drawn as written and
    /// truncated to the panel, which is what keeps the measured height honest.
    pub fn with_note(mut self, note: Vec<String>) -> Self {
        self.note = note;
        self
    }

    /// What the picker says above its list.
    #[cfg(test)]
    pub(crate) fn note(&self) -> &[String] {
        &self.note
    }

    /// Rows the note costs: its lines plus the blank one under them.
    fn note_rows(&self) -> u16 {
        match self.note.is_empty() {
            true => 0,
            false => self.note.len() as u16 + 1,
        }
    }

    /// Handle a key event. Returns the outcome (select/close/consumed).
    pub fn on_key(&mut self, key: &KeyEvent) -> ModelPickerOutcome {
        let on_models = self.providers.is_none();
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => return self.escape(),
            KeyCode::Enter => return self.enter(),
            KeyCode::Right if !on_models => return self.enter(),
            KeyCode::Tab if on_models => self.widen(),
            KeyCode::Left if on_models => self.show_providers(),
            KeyCode::Up => self.nav_mut().move_up(),
            KeyCode::Char('p') if ctrl => self.nav_mut().move_up(),
            KeyCode::Down => self.nav_mut().move_down(),
            KeyCode::Char('n') if ctrl => self.nav_mut().move_down(),
            // Everything else is swallowed: the picker is modal, so a stray key must not reach the
            // composer behind it
            _ => {
                let rows = self.searchable_rows();
                self.nav_mut().edit_query(key, || rows);
            }
        }
        ModelPickerOutcome::Consumed
    }

    /// `esc`: back to the providers when they led here, else close.
    fn escape(&mut self) -> ModelPickerOutcome {
        if self.providers.is_some() || !self.from_providers {
            return ModelPickerOutcome::Close;
        }
        self.show_providers();
        ModelPickerOutcome::Consumed
    }

    /// `enter`: switch to the model under the cursor, or choose the provider under it.
    fn enter(&mut self) -> ModelPickerOutcome {
        if self.providers.is_some() {
            return self.choose_provider();
        }
        match self.selected_entry() {
            Some(entry) => ModelPickerOutcome::Select(Box::new(entry.info.clone())),
            None => ModelPickerOutcome::Consumed,
        }
    }

    /// Chooses the provider under the cursor.
    ///
    /// Another provider serving the model in use switches to it there and closes, since the model
    /// was never the question. The provider in use, or one without that model, opens on its
    /// models for the reader to choose from.
    fn choose_provider(&mut self) -> ModelPickerOutcome {
        let chosen = self.providers.as_ref().and_then(ProviderStep::selected);
        let Some(provider) = chosen.map(|row| row.id.clone()) else {
            return ModelPickerOutcome::Consumed;
        };
        if let Some(same) = self.same_model_from(&provider) {
            return ModelPickerOutcome::Select(Box::new(same.info.clone()));
        }
        self.providers = None;
        self.from_providers = true;
        self.show(Some(provider), false);
        ModelPickerOutcome::Consumed
    }

    /// The model in use as `provider` serves it, when that is a different provider and it does.
    fn same_model_from(&self, provider: &str) -> Option<&ModelEntry> {
        let active = self.active.as_ref().filter(|active| active.provider != provider)?;
        self.all.iter().find(|entry| entry.info.r#ref == ModelRef::new(provider, &active.id))
    }

    /// `tab`: every provider's models, or back to the one the step is about.
    fn widen(&mut self) {
        let widened = !self.widened;
        self.show(self.scope.clone(), widened);
    }

    /// Opens the provider step, on the provider the model step is about.
    fn show_providers(&mut self) {
        let current =
            self.scope.clone().or_else(|| self.active.as_ref().map(|it| it.provider.clone()));
        self.providers = Some(ProviderStep::new(&self.all, current.as_deref()));
    }

    /// Shows the model step about `scope`, widened or not, keeping the query and the cursor's model
    /// where they still apply.
    ///
    /// The scope is kept even while it serves nothing, which is everything on show until a
    /// refresh brings its models.
    fn show(&mut self, scope: Option<String>, widened: bool) {
        self.scope = scope;
        self.widened = widened;
        let items = match self.scoped_provider() {
            Some(provider) => self
                .all
                .iter()
                .filter(|entry| entry.info.r#ref.provider == provider)
                .cloned()
                .collect(),
            None => self.all.clone(),
        };
        self.replace_items(items);
    }

    /// The provider the model list is narrowed to right now, if it is.
    fn scoped_provider(&self) -> Option<&str> {
        self.served_scope().filter(|_| !self.widened)
    }

    /// The provider the model step is about, while it serves anything.
    fn served_scope(&self) -> Option<&str> {
        let scope = self.scope.as_deref()?;
        self.all.iter().any(|entry| entry.info.r#ref.provider == scope).then_some(scope)
    }

    /// The navigation of the step showing.
    fn nav(&self) -> &PickerNav {
        self.providers.as_ref().map_or(&self.nav, |step| &step.nav)
    }

    /// The navigation of the step showing, to move.
    fn nav_mut(&mut self) -> &mut PickerNav {
        match self.providers.as_mut() {
            Some(step) => &mut step.nav,
            None => &mut self.nav,
        }
    }

    /// What a reader can type at the rows of the step showing.
    fn searchable_rows(&self) -> Vec<String> {
        match &self.providers {
            Some(step) => step.searchable(),
            None => searchable(&self.items),
        }
    }

    /// Moves the selection to `filtered_index` (a mouse click on that row); clamped to range.
    pub fn select(&mut self, filtered_index: usize) {
        self.nav_mut().select(filtered_index);
    }

    /// Filters by pasted text, the way typing it would.
    pub fn paste(&mut self, text: &str) {
        let rows = self.searchable_rows();
        self.nav_mut().paste_query(text, &rows);
    }

    /// Swap in a freshly-built catalog without disturbing the reader.
    ///
    /// A list that arrives while someone is typing at it must not move the cursor off what they
    /// had chosen, so the row is re-found by id and the query survives. A model that has gone
    /// away takes the cursor to the top, which is the only honest answer left.
    pub fn refresh(&mut self, items: Vec<ModelEntry>) {
        self.all = items;
        self.show(self.scope.clone(), self.widened);
        if let Some(step) = &self.providers {
            let current = step.selected().map(|row| row.id.clone());
            self.providers = Some(ProviderStep::new(&self.all, current.as_deref()));
        }
    }

    /// Replaces the models on show, keeping the query and the model under the cursor.
    fn replace_items(&mut self, items: Vec<ModelEntry>) {
        let chosen = self.selected_entry().map(ModelEntry::qualified_id);
        let query = self.nav.query().to_string();
        self.items = items;
        self.nav = PickerNav::new(self.items.len());
        self.nav.set_query(&query, &searchable(&self.items));
        let row = chosen.and_then(|chosen| {
            let filtered = self.nav.filtered_indices();
            filtered.iter().position(|index| self.items[*index].qualified_id() == chosen)
        });
        if let Some(row) = row {
            self.nav.select(row);
        }
    }

    /// The highlighted model, if any row of the model step matches.
    pub fn selected_entry(&self) -> Option<&ModelEntry> {
        self.items.get(self.nav.selected_item_index()?)
    }

    /// Whether the provider step is the one showing.
    pub fn choosing_provider(&self) -> bool {
        self.providers.is_some()
    }

    /// The cursor's row among the visible matches.
    pub(crate) fn cursor(&self) -> usize {
        self.nav().selected()
    }

    /// Number of visible matches.
    pub fn match_count(&self) -> usize {
        self.nav().match_count()
    }

    /// The current search query.
    pub fn query(&self) -> &str {
        self.nav().query()
    }
}

/// What a reader can type at a model row: its provider-qualified id and its label.
fn searchable(items: &[ModelEntry]) -> Vec<String> {
    items.iter().map(|entry| format!("{} {}", entry.qualified_id(), entry.label)).collect()
}

#[cfg(test)]
mod tests;
