//! Keybinding registry — maps named actions to key chords.
//!
//! Pure data: no crossterm or ratatui deps in core. The TUI layer converts
//! `crossterm::event::KeyEvent` → a `(ctrl, shift, alt, KeyName)` tuple and feeds
//! it to [`KeyMap::resolve`].
//!
//! Action ids are namespaced (`tui.editor.submit`, `tui.app.open_palette`, …) so
//! both front-ends speak the same vocabulary.

mod actions;
mod chord;
mod defaults;
mod overrides;

pub use actions::*;
pub use chord::*;
pub use overrides::{keybindings_path, load_keybindings};

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// A keybinding map: named action → list of chords.
///
/// Built from [`KeyMap::defaults()`] plus optional user overrides (loaded from
/// `keybindings.json`). Unknown action ids or unparseable chords are skipped
/// with a warning — never fatal.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KeyMap {
    /// action → chords. Populated by [`KeyMap::defaults()`], then merged with
    /// user overrides via [`KeyMap::with_overrides()`].
    #[serde(skip)]
    bindings: BTreeMap<UiAction, Vec<KeyChord>>,
}

impl KeyMap {
    /// The default keymap; users override it through `keybindings.json`.
    pub fn defaults() -> Self {
        Self { bindings: defaults::bindings() }
    }

    /// Merge user overrides on top of defaults.
    ///
    /// - Known action ids: chords are **replaced** (not appended) — a user binding
    ///   for `tui.editor.submit` replaces the default, not adds to it.
    /// - Explicit bindings evict matching chords from every other action, so user
    ///   overrides always win default collisions.
    /// - Unknown action ids: skipped with a warning (forward compatibility).
    /// - A retired id beside the action's current one: dropped, the current entry applies.
    /// - Unparseable chords: skipped with a warning (never fatal).
    pub fn with_overrides(mut self, overrides: HashMap<String, Vec<String>>) -> Self {
        let mut parsed = Vec::new();
        // one entry per action, so a retired id beside the current one neither wins nor evicts
        for (id_str, chord_strs) in overrides::canonical(overrides) {
            let action = match UiAction::from_id(&id_str) {
                Some(action) => action,
                None => {
                    tracing::warn!(action_id = %id_str, "unknown action id in keybindings; skipping");
                    continue;
                }
            };

            let mut chords = Vec::new();
            for chord_str in chord_strs {
                match KeyChord::parse(&chord_str) {
                    Ok(chord) => chords.push(chord),
                    Err(error) => {
                        tracing::warn!(chord = %chord_str, %error, "unparseable key chord; skipping");
                    }
                }
            }

            if chords.is_empty() {
                tracing::warn!(action_id = %id_str, "all chords invalid for action; removing binding");
                self.bindings.remove(&action);
            } else {
                parsed.push((action, chords));
            }
        }

        // HashMap input order is unstable. Sorting makes collisions between two explicit
        // overrides deterministic; the lexically later action owns the shared chord.
        parsed.sort_by_key(|(action, _)| action.id());
        for (action, chords) in parsed {
            self.bindings.insert(action, chords.clone());
            self.bindings.retain(|other_action, other_chords| {
                *other_action == action || {
                    other_chords.retain(|chord| !chords.contains(chord));
                    !other_chords.is_empty()
                }
            });
        }
        self
    }

    /// Resolve a key event to a UiAction, if any default chord matches.
    ///
    /// Returns `None` when no chord matches — the TUI should then feed the key
    /// to the editor (for bare printable keys) or ignore it. A chord bound to two actions resolves
    /// to the one declared first in `keymap/actions.rs`.
    pub fn resolve(&self, ctrl: bool, shift: bool, alt: bool, key: &KeyName) -> Option<UiAction> {
        self.bindings
            .iter()
            .find(|(_, chords)| {
                chords
                    .iter()
                    .any(|c| c.ctrl == ctrl && c.shift == shift && c.alt == alt && c.key == *key)
            })
            .map(|(action, _)| *action)
    }

    /// Get the chords bound to an action (for palette/help display).
    pub fn chords_for(&self, action: UiAction) -> Option<&Vec<KeyChord>> {
        self.bindings.get(&action)
    }

    /// All bound actions (for palette listing).
    pub fn actions(&self) -> impl Iterator<Item = UiAction> + '_ {
        self.bindings.keys().copied()
    }

    /// Serialize to a config-friendly map (action id → chord strings).
    pub fn to_config_map(&self) -> HashMap<String, Vec<String>> {
        self.bindings
            .iter()
            .map(|(action, chords)| {
                let id = action.id().to_string();
                let strs: Vec<String> = chords.iter().map(|c| c.display()).collect();
                (id, strs)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests;
