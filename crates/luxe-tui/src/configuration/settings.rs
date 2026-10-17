//! Settings — the Configuration surface's pages of controls over `config.json`.
//!
//! Down the left, the pages ([`luxe_core::settings::Page`]); on the right, the settings on the open
//! one, each with its value and where that value came from. Keys in, a [`ConfigOutcome`] out: the
//! surface never writes a file itself, it asks the app to ([`SettingChange`]), and the app hands it
//! back fresh [`ConfigLayers`] afterwards. That keeps it pure, and it keeps the one place that
//! touches the reader's files in one place.
//!
//! The file stays the source of truth. Every row names its key, `e` opens the file, and a value the
//! project's `.luxe/config.json` sets is shown and explained but not changed from here — writes go
//! to the reader's own file, and a project override would quietly win over them.

pub(super) mod render;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use luxe_core::config::{ConfigLayers, Layer};
use luxe_core::settings::{Control, Page, Setting};
use luxe_core::tool_catalog::{CatalogTool, ToolGroup};
use serde_json::Value;
use std::cell::Cell;
use std::path::PathBuf;

use super::ConfigOutcome;

/// A change the surface asks the app to write to the global config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingChange {
    /// Set a key to a value
    Set {
        /// The key, as a path into the file
        path: &'static [&'static str],
        /// What to write
        value: Value,
    },
    /// Remove a key, so its default applies again
    Reset {
        /// The key, as a path into the file
        path: &'static [&'static str],
    },
    /// Turn one tool on or off in `tools.disabled`
    Tool {
        /// The tool's name
        name: &'static str,
        /// Whether it should be registered
        enabled: bool,
    },
}

impl SettingChange {
    /// The setting this change is to, when it is one of the table's.
    pub fn setting(&self) -> Option<&'static Setting> {
        match self {
            Self::Set { path, .. } | Self::Reset { path } => luxe_core::settings::find(path),
            Self::Tool { .. } => None,
        }
    }
}

/// One line of a page.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Row {
    /// A group's heading, which the cursor steps over
    Heading(&'static str),
    /// Air between groups
    Blank,
    /// The tools that cannot be turned off, named once
    Essentials,
    Setting(&'static Setting),
    Tool(&'static CatalogTool),
}

impl Row {
    fn selectable(self) -> bool {
        matches!(self, Self::Setting(_) | Self::Tool(_))
    }
}

/// The line under the list after a change: what happened, or why nothing did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    /// What to say
    pub text: String,
    /// Whether it is a refusal or a failure rather than news
    pub warning: bool,
}

/// The settings pages' state.
#[derive(Debug)]
pub struct SettingsState {
    page: usize,
    /// Index into [`Self::rows`], always on a selectable row when there is one
    cursor: usize,
    /// First row drawn, kept by the renderer so the cursor stays on screen
    scroll: Cell<usize>,
    layers: ConfigLayers,
    /// The theme in force, by name — the file may not name one, and the desktop may decide it
    theme: String,
    /// Where writes go, for the header and for `e`
    path: PathBuf,
    notice: Option<Notice>,
    /// Text typed after `/`, filtering every page at once
    query: Option<String>,
    /// The panel was too small to draw and its notice stood in, so only esc may act
    too_small: Cell<bool>,
}

impl SettingsState {
    /// Open on the first page.
    pub fn open(layers: ConfigLayers, theme: String, path: PathBuf) -> Self {
        let mut state = Self {
            page: 0,
            cursor: 0,
            scroll: Cell::new(0),
            layers,
            theme,
            path,
            notice: None,
            query: None,
            too_small: Cell::new(false),
        };
        state.settle();
        state
    }

    /// Open on `page`.
    pub fn open_on(mut self, page: Page) -> Self {
        self.select_page(Page::ALL.iter().position(|candidate| *candidate == page).unwrap_or(0));
        self
    }

    /// Take the files as they are now, after a write or a theme change, keeping the reader's place.
    pub fn refresh(&mut self, layers: ConfigLayers, theme: String) {
        self.layers = layers;
        self.theme = theme;
    }

    /// Say something under the list until the next keypress.
    pub fn set_notice(&mut self, notice: Notice) {
        self.notice = Some(notice);
    }

    /// Whether a search is open, which Esc closes before it leaves the pages.
    pub fn searching(&self) -> bool {
        self.query.is_some()
    }

    /// The page showing.
    pub fn page(&self) -> Page {
        Page::ALL[self.page]
    }

    /// Show page `index`, cursor on its first setting.
    pub fn select_page(&mut self, index: usize) {
        self.page = index.min(Page::ALL.len() - 1);
        self.query = None;
        self.cursor = 0;
        self.scroll.set(0);
        self.settle();
    }

    /// Put the cursor on the `index`th selectable row (a click, or the pointer passing over it).
    pub fn select(&mut self, index: usize) {
        if let Some(row) = self.selectable().get(index) {
            self.cursor = *row;
        }
    }

    /// The cursor's position among the selectable rows.
    pub fn selected(&self) -> usize {
        self.selectable().iter().position(|row| *row == self.cursor).unwrap_or(0)
    }

    /// Handle a keypress.
    pub fn on_key(&mut self, key: &KeyEvent) -> ConfigOutcome {
        self.notice = None;
        // the controls are not drawn, so none of them may change anything
        if self.too_small.get() && key.code != KeyCode::Esc {
            return ConfigOutcome::Consumed;
        }
        let plain = key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT;
        if let Some(query) = self.query.as_mut() {
            match key.code {
                KeyCode::Esc => {
                    self.query = None;
                    self.cursor = 0;
                    self.settle();
                    return ConfigOutcome::Consumed;
                }
                KeyCode::Backspace => {
                    query.pop();
                    self.cursor = 0;
                    self.settle();
                    return ConfigOutcome::Consumed;
                }
                KeyCode::Char(character) if plain && character != ' ' => {
                    query.push(character);
                    self.cursor = 0;
                    self.settle();
                    return ConfigOutcome::Consumed;
                }
                _ => {}
            }
        }
        match key.code {
            KeyCode::Up | KeyCode::Char('k') if plain => self.step(-1),
            KeyCode::Down | KeyCode::Char('j') if plain => self.step(1),
            KeyCode::Tab => self.select_page((self.page + 1) % Page::ALL.len()),
            KeyCode::BackTab => {
                self.select_page((self.page + Page::ALL.len() - 1) % Page::ALL.len())
            }
            KeyCode::Enter | KeyCode::Char(' ') | KeyCode::Right | KeyCode::Char('l') => {
                return self.activate_forward(true)
            }
            KeyCode::Left | KeyCode::Char('h') => return self.activate_forward(false),
            KeyCode::Char('r') if plain => return self.reset(),
            KeyCode::Char('e') if plain => return ConfigOutcome::EditFile(self.path.clone()),
            KeyCode::Char('/') if plain => {
                self.query = Some(String::new());
                self.cursor = 0;
                self.settle();
            }
            KeyCode::Char(digit @ '1'..='9') if plain => {
                let index = digit as usize - '1' as usize;
                if index < Page::ALL.len() {
                    self.select_page(index);
                }
            }
            _ => {}
        }
        ConfigOutcome::Consumed
    }

    /// Change the setting under the cursor the way Enter does (a click lands here too).
    pub fn activate(&mut self) -> ConfigOutcome {
        self.activate_forward(true)
    }

    fn activate_forward(&mut self, forward: bool) -> ConfigOutcome {
        match self.rows().get(self.cursor).copied() {
            Some(Row::Setting(setting)) => {
                // checked before the theme row opens its picker, so it is guarded here like the rest
                let (layer, current) = setting.current(&self.layers);
                if layer == Layer::Project {
                    return self.refuse_project(setting.label);
                }
                if setting.control == Control::Theme {
                    return ConfigOutcome::PickTheme;
                }
                let Some(value) = setting.stepped(&current, forward) else {
                    return ConfigOutcome::Consumed;
                };
                // Back to the default is the key gone, so the file says only what was chosen
                let change = match value == setting.default_value() {
                    true => SettingChange::Reset { path: setting.path },
                    false => {
                        SettingChange::Set { path: setting.path, value: setting.file_value(value) }
                    }
                };
                ConfigOutcome::Write(change)
            }
            Some(Row::Tool(tool)) => {
                let (layer, enabled) = self.tool_state(tool);
                if layer == Layer::Project {
                    return self.refuse_project(tool.name);
                }
                ConfigOutcome::Write(SettingChange::Tool { name: tool.name, enabled: !enabled })
            }
            _ => ConfigOutcome::Consumed,
        }
    }

    /// `r`: put the default back by removing what the reader's own file says.
    fn reset(&mut self) -> ConfigOutcome {
        let layer = match self.rows().get(self.cursor).copied() {
            Some(Row::Setting(setting)) => {
                let (layer, _) = setting.current(&self.layers);
                if layer == Layer::Global {
                    return ConfigOutcome::Write(SettingChange::Reset { path: setting.path });
                }
                layer
            }
            Some(Row::Tool(tool)) => {
                let (layer, _) = self.tool_state(tool);
                if layer == Layer::Global {
                    return ConfigOutcome::Write(SettingChange::Tool {
                        name: tool.name,
                        enabled: true,
                    });
                }
                layer
            }
            _ => return ConfigOutcome::Consumed,
        };
        let text = match layer {
            Layer::Project => "This project's .luxe/config.json sets it; change it there.",
            _ => "Already the default.",
        };
        self.notice = Some(Notice { text: text.into(), warning: false });
        ConfigOutcome::Consumed
    }

    fn refuse_project(&mut self, what: &str) -> ConfigOutcome {
        self.notice = Some(Notice {
            text: format!(
                "{what} is set by this project's .luxe/config.json, which wins over yours. \
                 Change it there."
            ),
            warning: true,
        });
        ConfigOutcome::Consumed
    }

    /// Whether `tool` is on, and which file said so.
    ///
    /// `tools.disabled` is a union of the two files rather than an override, so a tool either file
    /// turns off is off; the project's word is reported first because only that one cannot be
    /// undone from here.
    fn tool_state(&self, tool: &CatalogTool) -> (Layer, bool) {
        let lists = |document: &Option<Value>| {
            document
                .as_ref()
                .and_then(|document| luxe_core::config::value_at(document, &["tools", "disabled"]))
                .and_then(Value::as_array)
                .is_some_and(|list| list.iter().any(|entry| entry.as_str() == Some(tool.name)))
        };
        match (lists(&self.layers.project), lists(&self.layers.global)) {
            (true, _) => (Layer::Project, false),
            (false, true) => (Layer::Global, false),
            (false, false) => (Layer::Default, true),
        }
    }

    /// The lines of the page showing, or every match when a search is open.
    fn rows(&self) -> Vec<Row> {
        if let Some(query) = &self.query {
            return search(query);
        }
        let page = self.page();
        let mut rows = Vec::new();
        if page == Page::Tools {
            rows.push(Row::Essentials);
            for group in ToolGroup::OFFERED {
                rows.push(Row::Blank);
                rows.push(Row::Heading(group.heading()));
                rows.extend(
                    luxe_core::tool_catalog::toggleable()
                        .into_iter()
                        .filter(|tool| tool.group == group)
                        .map(Row::Tool),
                );
            }
            rows.push(Row::Blank);
            rows.push(Row::Heading("OPTIONS"));
        }
        rows.extend(luxe_core::settings::on(page).map(Row::Setting));
        rows
    }

    /// Indexes into [`Self::rows`] the cursor may rest on.
    fn selectable(&self) -> Vec<usize> {
        self.rows().iter().enumerate().filter(|(_, row)| row.selectable()).map(|(i, _)| i).collect()
    }

    fn step(&mut self, delta: isize) {
        let selectable = self.selectable();
        let Some(at) = selectable.iter().position(|row| *row == self.cursor) else {
            self.settle();
            return;
        };
        let next = at.saturating_add_signed(delta).min(selectable.len().saturating_sub(1));
        self.cursor = selectable[next];
    }

    /// Move the cursor onto a selectable row, the first one from where it is.
    fn settle(&mut self) {
        let selectable = self.selectable();
        if !selectable.contains(&self.cursor) {
            self.cursor = selectable
                .iter()
                .copied()
                .find(|row| *row >= self.cursor)
                .unwrap_or(selectable.first().copied().unwrap_or(0));
        }
    }
}

/// Every setting and tool whose name, label or key holds `query`, across all pages.
fn search(query: &str) -> Vec<Row> {
    let wanted = query.to_lowercase();
    let hits = |text: &str| text.to_lowercase().contains(&wanted);
    let mut rows: Vec<Row> = luxe_core::settings::SETTINGS
        .iter()
        .filter(|setting| hits(setting.label) || hits(&setting.key()))
        .map(Row::Setting)
        .collect();
    rows.extend(
        luxe_core::tool_catalog::toggleable()
            .into_iter()
            .filter(|tool| hits(tool.name) || hits(tool.summary))
            .map(Row::Tool),
    );
    rows
}

#[cfg(test)]
mod tests;
