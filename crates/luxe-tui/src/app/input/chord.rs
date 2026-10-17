//! Chords — a terminal key event in the keymap's terms, and the action it is bound to.

use luxe_core::keymap::{KeyMap, KeyName, UiAction};

/// The chord a key event is, in the keymap's terms, or `None` for a key the keymap has no name for.
pub(crate) fn chord_of(key: &crossterm::event::KeyEvent) -> Option<luxe_core::keymap::KeyChord> {
    use crossterm::event::{KeyCode, KeyModifiers};

    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let mut shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let alt = key.modifiers.contains(KeyModifiers::ALT);

    let key_name = match key.code {
        KeyCode::Enter => KeyName::Enter,
        KeyCode::Esc => KeyName::Esc,
        KeyCode::Tab => KeyName::Tab,
        KeyCode::BackTab => {
            shift = true;
            KeyName::Tab
        }
        KeyCode::Backspace => KeyName::Backspace,
        KeyCode::Delete => KeyName::Delete,
        KeyCode::Insert => KeyName::Insert,
        KeyCode::Left => KeyName::Left,
        KeyCode::Right => KeyName::Right,
        KeyCode::Up => KeyName::Up,
        KeyCode::Down => KeyName::Down,
        KeyCode::Home => KeyName::Home,
        KeyCode::End => KeyName::End,
        KeyCode::PageUp => KeyName::PageUp,
        KeyCode::PageDown => KeyName::PageDown,
        // A capital is the shifted letter. Terminals report `E` as itself, with or without the
        // shift flag, while a chord written `shift+e` parses to the lower case — so without this a
        // binding to a shifted letter could never be pressed
        KeyCode::Char(character) if character.is_ascii_uppercase() => {
            shift = true;
            KeyName::Char(character.to_ascii_lowercase())
        }
        KeyCode::Char(character) => KeyName::Char(character),
        KeyCode::F(number) => match number {
            1 => KeyName::F1,
            2 => KeyName::F2,
            3 => KeyName::F3,
            4 => KeyName::F4,
            5 => KeyName::F5,
            6 => KeyName::F6,
            7 => KeyName::F7,
            8 => KeyName::F8,
            9 => KeyName::F9,
            10 => KeyName::F10,
            11 => KeyName::F11,
            12 => KeyName::F12,
            _ => return None,
        },
        _ => return None,
    };

    Some(luxe_core::keymap::KeyChord { ctrl, shift, alt, key: key_name })
}

/// Resolves a terminal key event through the configured key map.
pub(crate) fn resolve_key_event(
    keymap: &KeyMap,
    key: crossterm::event::KeyEvent,
) -> Option<UiAction> {
    let chord = chord_of(&key)?;
    keymap.resolve(chord.ctrl, chord.shift, chord.alt, &chord.key)
}
