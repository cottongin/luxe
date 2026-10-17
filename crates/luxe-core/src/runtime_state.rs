//! Runtime state — the choices a session made, remembered for the next one.
//!
//! Picking a model or a reasoning level is a decision, and re-making it on every launch is a tax on
//! the person who already decided. This is the small, auto-managed record of the last such choice:
//! not configuration (the user never edits it), not session data (it outlives any one session).
//!
//! Deliberately **not** written into `models.json` or `auth.json`. Those are user-authored and hold
//! secrets, and a serde round-trip through our own structs would redact API keys and drop fields we
//! do not model. A separate state file can be deleted at any time with no consequence beyond
//! forgetting.

use crate::provider::ThinkingLevel;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Schema version for the durable record.
pub const RUNTIME_STATE_SCHEMA_VERSION: u32 = 1;

/// How many recently used command ids the record keeps.
///
/// More than a front-end shows, so a command that cannot run where you are now is skipped rather
/// than forgotten, and comes back when you are somewhere it works.
pub const RECENT_COMMANDS_KEPT: usize = 20;

/// A model the user chose, in the form settings resolution needs it back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RememberedModel {
    /// Provider name (`anthropic`, `openai_compat`, …).
    pub provider: String,
    /// Wire model id, exactly as the provider expects it.
    pub id: String,
}

/// What the last session chose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RuntimeState {
    /// Schema version, so a future format can be ignored rather than misread.
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    /// The last model chosen in a session.
    pub model: Option<RememberedModel>,
    /// The last reasoning level chosen in a session.
    ///
    /// The *requested* level, not the one a model clamped it to: a session with a model that cannot
    /// think must not quietly forget that you asked for `high`.
    pub thinking: Option<ThinkingLevel>,
    /// Command ids the palette and `/` ran, most recent first, at most [`RECENT_COMMANDS_KEPT`].
    ///
    /// Absent from a record written before it existed, which reads as nothing used yet.
    pub recent_commands: Vec<String>,
    /// Pane widths the reader set by dragging a divider. Left out while none has been set.
    #[serde(skip_serializing_if = "PaneWidths::is_default")]
    pub panes: PaneWidths,
}

/// How wide the reader dragged each resizable pane, in terminal cells; `None` keeps the pane's own
/// default.
///
/// Columns rather than a share of the terminal, the way a multiplexer keeps a sidebar: a pane sized
/// to its content should stay that size when the window around it changes. A front-end clamps each
/// one to what the terminal in front of it can seat.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PaneWidths {
    /// The Context Dock beside the conversation
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dock: Option<u16>,
    /// The guide beside a Guided Review's diff
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review_guide: Option<u16>,
    /// A peeked diff beside the conversation
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peek: Option<u16>,
}

impl PaneWidths {
    /// Whether every pane keeps its default.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

fn default_schema_version() -> u32 {
    RUNTIME_STATE_SCHEMA_VERSION
}

/// Hand-written so a fresh record carries the current schema version.
///
/// A derived `Default` would give `0`, which every reader rejects as a foreign schema: the first
/// write would make the record permanently unreadable.
impl Default for RuntimeState {
    fn default() -> Self {
        Self {
            schema_version: RUNTIME_STATE_SCHEMA_VERSION,
            model: None,
            thinking: None,
            recent_commands: Vec::new(),
            panes: PaneWidths::default(),
        }
    }
}

impl RuntimeState {
    /// Reads the record, or a default when it is absent, unreadable, or from another schema.
    ///
    /// Never an error: forgetting is the correct failure mode for a convenience.
    pub fn load() -> Self {
        Self::load_from(&Self::path())
    }

    /// Reads the record from an explicit path (tests, and callers with their own state dir).
    pub fn load_from(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        match serde_json::from_str::<Self>(&text) {
            Ok(state) if state.schema_version == RUNTIME_STATE_SCHEMA_VERSION => state,
            Ok(_) => Self::default(),
            Err(error) => {
                tracing::debug!(%error, "runtime state is unreadable; starting from defaults");
                Self::default()
            }
        }
    }

    /// Writes the record atomically, owner-readable only.
    pub fn save(&self) -> std::io::Result<()> {
        self.save_to(&Self::path())
    }

    /// Writes the record to an explicit path.
    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        let body = serde_json::to_string_pretty(self)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        crate::fs_atomic::replace_private(path, body.as_bytes())
    }

    /// Reads the record at `path`, applies `change`, and writes it back when `change` says it
    /// changed something.
    ///
    /// Every remembering is this: read, change one field, write. Two Luxe processes racing lose
    /// each other's update, which is acceptable for a convenience record; a failed write is logged
    /// and otherwise ignored, because forgetting across a relaunch is the right failure for one.
    fn update_at(path: &Path, what: &str, change: impl FnOnce(&mut Self) -> bool) -> Self {
        let mut state = Self::load_from(path);
        if change(&mut state) {
            if let Err(error) = state.save_to(path) {
                tracing::debug!(%error, what, "could not remember");
            }
        }
        state
    }

    /// Records a chosen model, leaving everything else as it was.
    pub fn remember_model(provider: &str, id: &str) {
        let remembered = RememberedModel { provider: provider.to_owned(), id: id.to_owned() };
        Self::update_at(&Self::path(), "the chosen model", |state| {
            let changed = state.model.as_ref() != Some(&remembered);
            if changed {
                state.model = Some(remembered);
            }
            changed
        });
    }

    /// Records a chosen reasoning level, leaving everything else as it was.
    pub fn remember_thinking(level: ThinkingLevel) {
        Self::update_at(&Self::path(), "the chosen thinking level", |state| {
            let changed = state.thinking != Some(level);
            if changed {
                state.thinking = Some(level);
            }
            changed
        });
    }

    /// Records a command as the most recently used, at an explicit path, and returns the list.
    ///
    /// A write that fails still returns the list the caller should hold: forgetting across a
    /// relaunch is the right failure for a convenience, forgetting mid-session is not.
    pub fn remember_command_at(path: &Path, id: &str) -> Vec<String> {
        Self::update_at(path, "the command", |state| {
            let before = state.recent_commands.clone();
            push_recent(&mut state.recent_commands, id);
            state.recent_commands != before
        })
        .recent_commands
    }

    /// Records the pane widths at an explicit path, leaving everything else as it was.
    pub fn remember_panes_at(path: &Path, panes: PaneWidths) {
        Self::update_at(path, "the pane widths", |state| {
            let changed = state.panes != panes;
            if changed {
                state.panes = panes;
            }
            changed
        });
    }

    /// Where the record lives: `state.json` in [`crate::user_dirs::luxe_state_dir`].
    ///
    /// The state directory, not the config directory: this is machine-written data about what
    /// happened, and it must never look like something to hand-edit.
    pub fn path() -> PathBuf {
        crate::user_dirs::luxe_state_dir().join("state.json")
    }
}

/// Moves `id` to the front of `recent`, once, and keeps the list to [`RECENT_COMMANDS_KEPT`].
///
/// Public so a front-end holding its own copy orders it exactly the way the record does.
pub fn push_recent(recent: &mut Vec<String>, id: &str) {
    recent.retain(|kept| kept != id);
    recent.insert(0, id.to_owned());
    recent.truncate(RECENT_COMMANDS_KEPT);
}

#[cfg(test)]
mod tests;
