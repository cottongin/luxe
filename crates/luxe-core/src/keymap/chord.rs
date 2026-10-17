//! Key chords — modifiers + key name, parsing, and display (`ctrl+k`, `shift+enter`).

use serde::{Deserialize, Serialize};

/// A single key chord: modifiers + a key.
///
/// Parses from strings like `"ctrl+k"`, `"shift+enter"`, `"alt+t"`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct KeyChord {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub key: KeyName,
}

/// Normalized key names (covers the crossterm special keys + letters/digits).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyName {
    // Letters/digits (kept lowercase for parsing convenience)
    Char(char),
    // Named specials (crossterm equivalents in parens)
    Enter,     // KeyCode::Enter
    Esc,       // KeyCode::Esc
    Tab,       // KeyCode::Tab
    Backspace, // KeyCode::Backspace
    Delete,    // KeyCode::Delete
    Insert,    // KeyCode::Insert
    Left,      // KeyCode::Left
    Right,     // KeyCode::Right
    Up,        // KeyCode::Up
    Down,      // KeyCode::Down
    Home,      // KeyCode::Home
    End,       // KeyCode::End
    PageUp,    // KeyCode::PageUp
    PageDown,  // KeyCode::PageDown
    Space,     // KeyCode::Char(' ')
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
}

impl KeyName {
    /// Format for display (e.g. "enter", "k", "f1"): lower case, so a capital only ever means
    /// Shift.
    pub fn display(&self) -> String {
        match self {
            Self::Char(c) => c.to_ascii_lowercase().to_string(),
            Self::Enter => "enter".to_string(),
            Self::Esc => "esc".to_string(),
            Self::Tab => "tab".to_string(),
            Self::Backspace => "backspace".to_string(),
            Self::Delete => "delete".to_string(),
            Self::Insert => "insert".to_string(),
            Self::Left => "left".to_string(),
            Self::Right => "right".to_string(),
            Self::Up => "up".to_string(),
            Self::Down => "down".to_string(),
            Self::Home => "home".to_string(),
            Self::End => "end".to_string(),
            Self::PageUp => "pgup".to_string(),
            Self::PageDown => "pgdn".to_string(),
            Self::Space => "space".to_string(),
            Self::F1 => "f1".to_string(),
            Self::F2 => "f2".to_string(),
            Self::F3 => "f3".to_string(),
            Self::F4 => "f4".to_string(),
            Self::F5 => "f5".to_string(),
            Self::F6 => "f6".to_string(),
            Self::F7 => "f7".to_string(),
            Self::F8 => "f8".to_string(),
            Self::F9 => "f9".to_string(),
            Self::F10 => "f10".to_string(),
            Self::F11 => "f11".to_string(),
            Self::F12 => "f12".to_string(),
        }
    }
}

/// Keys read the way they are written in a keybindings file, all lower case: `ctrl+p`, `alt+l`,
/// `enter`. So `ctrl+p` can never be read as Ctrl+Shift+P, which `Ctrl+P` could, and a capital in
/// a hint (`G`, `J K`) means Shift, as vim writes it. A shifted letter keeps its `shift+` here,
/// since a keybindings file is written from this and must read back to the same key.
impl std::fmt::Display for KeyChord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("ctrl");
        }
        if self.shift {
            parts.push("shift");
        }
        if self.alt {
            parts.push("alt");
        }
        let key_display = self.key.display();
        parts.push(&key_display);
        write!(f, "{}", parts.join("+"))
    }
}

/// Errors from parsing a key chord string.
#[derive(Debug, thiserror::Error)]
pub enum KeymapError {
    #[error("empty chord string")]
    Empty,
    #[error("unknown modifier '{0}' (expected ctrl, shift, or alt)")]
    UnknownModifier(String),
    #[error("unknown key '{0}'")]
    UnknownKey(String),
    #[error("chord must have exactly one key at the end")]
    MissingKey,
}

impl KeyChord {
    /// Parse a chord string like `"ctrl+k"`, `"shift+enter"`, `"ctrl+alt+t"`.
    ///
    /// Modifiers are case-insensitive; the final key is case-insensitive for
    /// letters (so "Ctrl+K" and "Ctrl+k" both parse to the same chord).
    pub fn parse(s: &str) -> Result<Self, KeymapError> {
        let s = s.trim();
        if s.is_empty() {
            return Err(KeymapError::Empty);
        }

        let mut ctrl = false;
        let mut shift = false;
        let mut alt = false;
        let mut key: Option<KeyName> = None;

        for part in s.split('+') {
            let part = part.trim().to_ascii_lowercase();
            match part.as_str() {
                "ctrl" => ctrl = true,
                "shift" => shift = true,
                "alt" => alt = true,
                other => {
                    // Last segment must be the key.
                    let k = parse_key_name(other)?;
                    key = Some(k);
                }
            }
        }

        let key = key.ok_or(KeymapError::MissingKey)?;
        Ok(Self { ctrl, shift, alt, key })
    }

    /// Format back to a string (round-trips parse).
    pub fn display(&self) -> String {
        format!("{self}")
    }
}

/// Parse a single key name from a string.
fn parse_key_name(s: &str) -> Result<KeyName, KeymapError> {
    match s.to_ascii_lowercase().as_str() {
        "enter" => Ok(KeyName::Enter),
        "esc" | "escape" => Ok(KeyName::Esc),
        "tab" => Ok(KeyName::Tab),
        "backspace" => Ok(KeyName::Backspace),
        "delete" | "del" => Ok(KeyName::Delete),
        "insert" | "ins" => Ok(KeyName::Insert),
        "left" => Ok(KeyName::Left),
        "right" => Ok(KeyName::Right),
        "up" => Ok(KeyName::Up),
        "down" => Ok(KeyName::Down),
        "home" => Ok(KeyName::Home),
        "end" => Ok(KeyName::End),
        "pageup" | "pgup" => Ok(KeyName::PageUp),
        "pagedown" | "pgdn" => Ok(KeyName::PageDown),
        "space" => Ok(KeyName::Space),
        "f1" => Ok(KeyName::F1),
        "f2" => Ok(KeyName::F2),
        "f3" => Ok(KeyName::F3),
        "f4" => Ok(KeyName::F4),
        "f5" => Ok(KeyName::F5),
        "f6" => Ok(KeyName::F6),
        "f7" => Ok(KeyName::F7),
        "f8" => Ok(KeyName::F8),
        "f9" => Ok(KeyName::F9),
        "f10" => Ok(KeyName::F10),
        "f11" => Ok(KeyName::F11),
        "f12" => Ok(KeyName::F12),
        // Single character → Char variant.
        other if other.len() == 1 => other
            .chars()
            .next()
            .map(KeyName::Char)
            .ok_or_else(|| KeymapError::UnknownKey(s.to_string())),
        _ => Err(KeymapError::UnknownKey(s.to_string())),
    }
}
