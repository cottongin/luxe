//! Dashboard — the start screen: one prompt that is also a search over everything else.
//!
//! The composer is the screen, so every key is text: a letter types a letter, and nothing waits
//! for a single key to mean "open the session finder" the way the old menu's did. What the menu
//! offered is still here, as the list under the prompt — Resume, a new session, the session
//! finder, review, configuration — and typing narrows it: the top row becomes "Ask in a new
//! session", and under it come the sessions whose titles match and the commands that do. `↑`/`↓`
//! move through the draft's lines, and choose once the cursor is on its edge; `enter` runs. A
//! `minimal` style keeps the list folded until `↓` asks for it.
//!
//! Pure state plus rendering, in the `Action → update → Command → Event` loop like any surface.
//! The input itself is the app's own [`crate::editor::Editor`], drawn by the composer that draws
//! it in a session, so a draft carries straight into the session that answers it. The CLI gathers
//! the bounded session catalog before the view is entered.

mod catalog;
mod entries;
mod keys;
mod render;
mod search;

pub use catalog::{PreviewTurn, SessionCatalogSource, SessionRecord, SessionSummary};
pub(crate) use search::Entry;

use catalog::{compute_age, normalize_title};
pub use catalog::{relative_age, PreviewRole};
use keys::outcome_for_ui_action;
use search::{mentions_project_context, reads_as_prompt, selectable_in};

#[cfg(test)]
mod tests;

use crate::commands::{Command, CommandContext};
use crate::editor::Editor;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use luxe_core::config::DashboardStyle;
use luxe_core::keymap::KeyMap;

pub use luxe_core::dashboard::PickerKind;

/// Recent sessions the list names under the menu rows.
const RECENT_LIMIT: usize = 5;
/// Sessions a search names before the finder takes over.
const SESSION_MATCHES: usize = 5;
/// Commands a search names.
const COMMAND_MATCHES: usize = 4;
/// Words past which a draft is a prompt rather than a search.
///
/// Session titles are first prompts, so a search for one is a fragment of a sentence: "dock group
/// direc", "why the provider stalls". Six words in, the draft has stopped looking for anything,
/// and a fuzzy match that needs every character in order has stopped finding it.
const SEARCH_WORDS: usize = 5;
/// Characters past which a draft is a prompt, for one that is long without many words
const SEARCH_CHARS: usize = 48;

/// The session the dashboard offers to resume, and what it left behind.
///
/// A specific session rather than "the latest": the row names one, and pressing it must open the
/// one it named. Resolving `latest` again at keypress time is how a background write — a loop run,
/// another terminal — opens something the reader never saw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeTarget {
    /// Durable session identifier
    pub id: String,
    /// Human title derived from the first user message
    pub title: String,
    /// Relative modification time
    pub age: String,
}

/// What the working tree has waiting, for the review row.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DashBriefing {
    /// Files with uncommitted changes
    pub changed_files: usize,
    /// Lines added across them
    pub added: u64,
    /// Lines removed across them
    pub removed: u64,
}

/// What the app should do after a key or a click.
#[derive(Debug, Clone, PartialEq)]
pub enum DashOutcome {
    /// Starts a new session.
    NewSession,
    /// Starts a new session and sends the draft in the prompt as its first message.
    Ask,
    /// Resumes a named session — the one the row showed — with any draft carried into it.
    Resume(String),
    /// Resumes the most recent session, for a binding pressed with no row selected.
    ResumeLatest,
    /// Opens Guided Review on the working tree.
    ReviewChanges,
    /// Opens a session sub-picker.
    OpenPicker(PickerKind),
    /// Opens the session finder searching transcripts for this text. Args: `(query)`.
    GrepSessions(String),
    /// Opens the Configuration surface.
    OpenConfig,
    /// Opens the model picker over the dashboard.
    OpenModelPicker,
    /// Opens the list of instruction files, skills and MCP servers a turn reads.
    OpenContextFiles,
    /// Moves to the next mode, the one the session will start in.
    CycleMode,
    /// Runs a command from the registry, as the palette would. Args: `(command)`.
    RunCommand(Command),
    /// Quits the TUI.
    Quit,
    /// Consumes the key without dispatching an action.
    None,
}

/// Pure dashboard state.
#[derive(Debug)]
pub struct DashboardState {
    /// Recent non-empty sessions the empty prompt lists
    pub recent: Vec<SessionSummary>,
    /// Bounded session catalog the search and the pickers read
    pub sessions: Vec<SessionSummary>,
    /// The session the Resume row offers, when there is one worth offering
    pub resume: Option<ResumeTarget>,
    /// What the working tree has waiting
    pub briefing: DashBriefing,
    /// One quiet line under the surface: an available update, and nothing else
    pub footer_note: Option<String>,
    /// The branch of the worktree this run works in, or `None` in the main checkout
    worktree: Option<String>,
    /// Whether a turn could authenticate right now, which decides whether the list leads with setup
    provider_configured: bool,
    style: DashboardStyle,
    /// Whether the minimal style's list has been asked for
    expanded: bool,
    /// The draft as of the last edit, which is what the list is built from
    query: String,
    /// Whether the draft reads as a prompt rather than a search: the list folds away, and Enter
    /// asks
    prompt: bool,
    /// Whether the composer's `/` or `@` flyout is open under the prompt, in the list's place
    flyout: bool,
    /// The key that switches the mode and the mode it switches to, for the keys under the list
    mode_switch: Option<(String, String)>,
    /// Index into [`Self::entries`], always on a selectable row
    selected: usize,
    /// First list row drawn, kept by the renderer so the cursor stays on screen
    scroll: std::cell::Cell<usize>,
    /// What a search can reach besides sessions: the commands a fresh session offers
    commands: Vec<Command>,
}

impl DashboardState {
    /// Creates a dashboard over `recent` sessions.
    pub fn new(recent: Vec<SessionSummary>) -> Self {
        let sessions = recent.clone();
        let commands = crate::commands::CommandRegistry::shared()
            .iter()
            .filter(|command| command.available(&CommandContext::default()))
            .cloned()
            .collect();
        Self {
            recent: recent.into_iter().take(RECENT_LIMIT).collect(),
            sessions,
            resume: None,
            briefing: DashBriefing::default(),
            footer_note: None,
            worktree: None,
            provider_configured: true,
            style: DashboardStyle::Launcher,
            expanded: false,
            query: String::new(),
            prompt: false,
            flyout: false,
            mode_switch: None,
            selected: 0,
            scroll: std::cell::Cell::new(0),
            commands,
        }
    }

    /// Adds the full bounded session catalog gathered by the CLI.
    pub fn with_sessions(mut self, sessions: Vec<SessionSummary>) -> Self {
        self.sessions = sessions;
        self
    }

    /// Sets the session the Resume row offers.
    pub fn with_resume(mut self, resume: Option<ResumeTarget>) -> Self {
        self.resume = resume;
        // The recent list names other sessions: the one Resume offers is already on screen
        if let Some(resume) = &self.resume {
            self.recent.retain(|session| session.id != resume.id);
        }
        self.settle();
        self
    }

    /// Sets what the working tree has waiting.
    pub fn with_briefing(mut self, briefing: DashBriefing) -> Self {
        self.briefing = briefing;
        self
    }

    /// One line of news under the surface — an available update — or nothing.
    pub fn with_footer_note(mut self, note: Option<String>) -> Self {
        self.footer_note = note;
        self
    }

    /// Says which worktree this run works in, by branch, or `None` for the main checkout.
    pub fn with_worktree(mut self, worktree: Option<String>) -> Self {
        self.worktree = worktree;
        self
    }

    /// Whether `session` can open here, in place: it works where this run does, in the same
    /// worktree or in none, so nothing has to move first. A session the catalog does not name is
    /// taken to work in the main checkout.
    pub(crate) fn opens_in_place(&self, session: &str) -> bool {
        let named = self.sessions.iter().chain(&self.recent).find(|known| known.id == session);
        named.and_then(|known| known.worktree.as_deref()) == self.worktree.as_deref()
    }

    /// Picks the start screen's shape.
    pub fn with_style(mut self, style: DashboardStyle) -> Self {
        self.set_style(style);
        self
    }

    /// Picks the start screen's shape, in place.
    pub fn set_style(&mut self, style: DashboardStyle) {
        self.style = match style {
            DashboardStyle::Unknown => DashboardStyle::Launcher,
            style => style,
        };
    }

    /// Says whether a turn could authenticate right now.
    ///
    /// The list leads with setup, selected, when it could not: everything else on it needs a model.
    pub fn set_provider_configured(&mut self, configured: bool) {
        if self.provider_configured != configured {
            self.provider_configured = configured;
            self.selected = 0;
            self.settle();
        }
    }

    /// Whether a turn could authenticate right now.
    pub fn provider_configured(&self) -> bool {
        self.provider_configured
    }

    /// Whether the list is showing: not for a draft that has become a prompt, and not in the
    /// minimal style until `↓` or a draft asks for it.
    pub fn list_shown(&self) -> bool {
        if self.prompt || self.flyout {
            return false;
        }
        self.style != DashboardStyle::Minimal || self.expanded || !self.query.trim().is_empty()
    }

    /// Says whether the composer's `/` or `@` flyout is open, which takes the list's place.
    pub fn set_flyout(&mut self, open: bool) {
        self.flyout = open;
    }

    /// Says which key switches the mode and the mode it switches to, or that none is bound.
    ///
    /// Args: `(chord, next mode)`.
    pub(crate) fn set_mode_switch(&mut self, mode_switch: Option<(String, String)>) {
        self.mode_switch = mode_switch;
    }

    /// Whether a flyout has the room under the prompt.
    pub fn flyout_open(&self) -> bool {
        self.flyout
    }

    /// Whether the draft reads as a prompt, so Enter asks and nothing is searched.
    pub fn drafting_a_prompt(&self) -> bool {
        self.prompt
    }

    /// Builds the bounded searchable catalog used by the session pickers, leaving out a session
    /// whose title has no words.
    pub fn populate_session_catalog(
        sessions: Vec<SessionRecord>,
        now: std::time::SystemTime,
    ) -> Vec<SessionSummary> {
        sessions
            .into_iter()
            .filter_map(|record| {
                Some(SessionSummary {
                    name: normalize_title(&record.title)?,
                    age: compute_age(record.modified, now),
                    id: record.id,
                    search_text: record.search_text,
                    preview: record.preview,
                    worktree: record.worktree,
                })
            })
            .collect()
    }

    /// Handles a key, editing `editor` when the key is text.
    ///
    /// Plain letters are always text. A key the reader bound to a `tui.dashboard.*` action, or to
    /// switching the mode, still runs it as long as it is not text: a bare letter bound there
    /// would steal a character out of every prompt that contains it.
    pub fn on_key(
        &mut self,
        event: &KeyEvent,
        keymap: &KeyMap,
        editor: &mut Editor,
    ) -> DashOutcome {
        let chorded = event.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        let text = matches!(event.code, KeyCode::Char(_)) && !chorded;
        if !text {
            let action = crate::app::input::resolve_key_event(keymap, *event);
            if let Some(outcome) = action.and_then(outcome_for_ui_action) {
                return outcome;
            }
        }
        match event.code {
            KeyCode::Char('c') if event.modifiers == KeyModifiers::CONTROL => DashOutcome::Quit,
            KeyCode::Esc => {
                // Clears a draft, folds the minimal list, and otherwise does nothing: quitting by
                // reflex from a screen whose job is to take text is how a draft gets lost
                if !editor.current_input().is_empty() {
                    editor.cancel();
                    self.sync(editor);
                } else if self.expanded {
                    self.expanded = false;
                }
                DashOutcome::None
            }
            KeyCode::Up | KeyCode::Down => self.vertical_key(event, editor),
            KeyCode::Enter if event.modifiers.is_empty() => self.activate(),
            KeyCode::Enter => {
                editor.insert_newline();
                self.sync(editor);
                DashOutcome::None
            }
            _ => {
                editor.handle_key(*event);
                self.sync(editor);
                DashOutcome::None
            }
        }
    }

    /// `↑`/`↓` move through the draft while there is a line to move to, and choose a row once the
    /// cursor is on the draft's edge, so a one-line search still picks with the arrows.
    fn vertical_key(&mut self, event: &KeyEvent, editor: &mut Editor) -> DashOutcome {
        let up = event.code == KeyCode::Up;
        if self.arrow_moves_text(up, event, editor) {
            editor.handle_key(*event);
            self.sync(editor);
        } else if up {
            self.step(-1);
        } else if !self.list_shown() {
            self.expanded = true;
        } else {
            self.step(1);
        }
        DashOutcome::None
    }

    /// Whether a vertical arrow belongs to the text: shift is selecting, no list is up to choose
    /// from, or the cursor has a line to move to.
    fn arrow_moves_text(&self, up: bool, event: &KeyEvent, editor: &Editor) -> bool {
        if event.modifiers.contains(KeyModifiers::SHIFT) || self.prompt || self.flyout {
            return true;
        }
        match up {
            true => !editor.cursor_on_first_row(),
            false => !editor.cursor_on_last_row(),
        }
    }

    /// Takes the draft as it stands now, after a key or a paste, and starts the list over.
    pub fn sync(&mut self, editor: &Editor) {
        let query = editor.current_input().to_string();
        // A paste or an image is something to send, never something to search for
        let attached = editor.has_images() || editor.expanded_input() != query;
        let prompt = attached || reads_as_prompt(&query);
        if query != self.query || prompt != self.prompt {
            self.query = query;
            self.prompt = prompt;
            self.selected = 0;
            self.scroll.set(0);
            self.settle();
        }
    }

    /// Put the cursor on the `index`th selectable row (the pointer passing over it).
    pub fn select(&mut self, index: usize) {
        if let Some(row) = self.selectable().get(index) {
            self.selected = *row;
        }
    }

    /// What clicking the `index`th selectable row does.
    pub fn outcome_at(&self, index: usize) -> Option<DashOutcome> {
        let row = *self.selectable().get(index)?;
        Some(self.outcome_of(self.entries().get(row)?))
    }

    /// Run the row under the cursor.
    fn activate(&self) -> DashOutcome {
        match self.entries().get(self.selected) {
            Some(entry) => self.outcome_of(entry),
            None => DashOutcome::None,
        }
    }

    fn outcome_of(&self, entry: &Entry) -> DashOutcome {
        match entry {
            Entry::SetUp | Entry::Configuration => DashOutcome::OpenConfig,
            Entry::Resume => match &self.resume {
                Some(resume) => DashOutcome::Resume(resume.id.clone()),
                None => DashOutcome::ResumeLatest,
            },
            Entry::NewSession => DashOutcome::NewSession,
            Entry::Sessions => DashOutcome::OpenPicker(PickerKind::FindSession),
            Entry::Review => DashOutcome::ReviewChanges,
            Entry::Quit => DashOutcome::Quit,
            Entry::Recent(index) => match self.recent.get(*index) {
                Some(session) => DashOutcome::Resume(session.id.clone()),
                None => DashOutcome::None,
            },
            Entry::Session(index, _) => match self.sessions.get(*index) {
                Some(session) => DashOutcome::Resume(session.id.clone()),
                None => DashOutcome::None,
            },
            Entry::Ask => DashOutcome::Ask,
            Entry::Slash(command) => DashOutcome::RunCommand(command.clone()),
            Entry::Grep => DashOutcome::GrepSessions(self.query.trim().to_string()),
            Entry::Command(index, _) => match self.commands.get(*index) {
                Some(command) => DashOutcome::RunCommand(command.clone()),
                None => DashOutcome::None,
            },
            Entry::ProjectContext => DashOutcome::OpenContextFiles,
            Entry::Heading(..) | Entry::Blank => DashOutcome::None,
        }
    }

    fn selectable(&self) -> Vec<usize> {
        selectable_in(&self.entries())
    }

    fn step(&mut self, delta: isize) {
        let selectable = self.selectable();
        let at = selectable.iter().position(|row| *row == self.selected).unwrap_or(0);
        let next = at.saturating_add_signed(delta).min(selectable.len().saturating_sub(1));
        self.selected = selectable.get(next).copied().unwrap_or(0);
    }

    /// Move the cursor onto a selectable row, the first one from where it is.
    fn settle(&mut self) {
        let selectable = self.selectable();
        if !selectable.contains(&self.selected) {
            self.selected = selectable
                .iter()
                .copied()
                .find(|row| *row >= self.selected)
                .or_else(|| selectable.first().copied())
                .unwrap_or(0);
        }
    }

    /// The command a draft would run, for the row and the hint to name.
    pub(crate) fn command(&self, index: usize) -> Option<&Command> {
        self.commands.get(index)
    }
}
