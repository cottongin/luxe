//! Motion — word and line jumps, and the deletions across them, in every encoding a terminal
//! sends.

use super::Editor;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// A word or line jump, or a deletion across one, as every text field's keyboard expects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TextCommand {
    reach: Reach,
    delete: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reach {
    WordBefore,
    WordAfter,
    LineStart,
    LineEnd,
}

/// The jump or deletion a key asks for, in every encoding a terminal sends it in.
///
/// On macOS, ghostty (like Terminal.app and iTerm's "natural text editing") sends Option+←/→ as
/// readline's `Esc b`/`Esc f`, and Cmd+←/→/⌫ as `Ctrl+A`/`Ctrl+E`/`Ctrl+U`; elsewhere the same keys
/// arrive as modified arrows. Alt+D and Ctrl+K are taken by the dock and the palette, so the
/// forward deletions are Ctrl/Alt+Delete only.
pub(super) fn text_command(key: &KeyEvent) -> Option<TextCommand> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let command = key.modifiers.contains(KeyModifiers::SUPER);
    let jump = |reach| Some(TextCommand { reach, delete: false });
    let delete = |reach| Some(TextCommand { reach, delete: true });
    match key.code {
        KeyCode::Left if command => jump(Reach::LineStart),
        KeyCode::Right if command => jump(Reach::LineEnd),
        KeyCode::Left if ctrl || alt => jump(Reach::WordBefore),
        KeyCode::Right if ctrl || alt => jump(Reach::WordAfter),
        KeyCode::Char('b') if alt && !ctrl => jump(Reach::WordBefore),
        KeyCode::Char('f') if alt && !ctrl => jump(Reach::WordAfter),
        KeyCode::Char('a') if ctrl && !alt => jump(Reach::LineStart),
        KeyCode::Char('e') if ctrl && !alt => jump(Reach::LineEnd),
        KeyCode::Backspace if command => delete(Reach::LineStart),
        KeyCode::Backspace if ctrl || alt => delete(Reach::WordBefore),
        KeyCode::Char('u') if ctrl && !alt => delete(Reach::LineStart),
        KeyCode::Delete if ctrl || alt => delete(Reach::WordAfter),
        _ => None,
    }
}

#[derive(PartialEq, Eq)]
enum CharClass {
    Word,
    Punctuation,
    Space,
}

fn char_class(character: char) -> CharClass {
    if character.is_alphanumeric() || character == '_' {
        CharClass::Word
    } else if character.is_whitespace() {
        CharClass::Space
    } else {
        CharClass::Punctuation
    }
}

impl Editor {
    pub(super) fn move_left(&mut self) {
        if let Some(part) = self.parts.iter().find(|part| part.end == self.cursor) {
            self.cursor = part.start;
        } else if let Some((start, _)) = self.buffer[..self.cursor].char_indices().next_back() {
            self.cursor = self.snap_out_of_part(start, false);
        }
        self.preferred_column = None;
    }

    pub(super) fn move_right(&mut self) {
        if let Some(part) = self.parts.iter().find(|part| part.start == self.cursor) {
            self.cursor = part.end;
        } else if let Some(character) = self.buffer[self.cursor..].chars().next() {
            self.cursor = self.snap_out_of_part(self.cursor + character.len_utf8(), true);
        }
        self.preferred_column = None;
    }

    pub(super) fn move_to_logical_line_start(&mut self) {
        self.cursor = self.line_start(self.cursor);
        self.preferred_column = None;
    }

    pub(super) fn move_to_logical_line_end(&mut self) {
        self.cursor = self.line_end(self.cursor);
        self.preferred_column = None;
    }

    fn line_start(&self, offset: usize) -> usize {
        let start = self.buffer[..offset].rfind('\n').map_or(0, |index| index + 1);
        self.snap_out_of_part(start, false)
    }

    fn line_end(&self, offset: usize) -> usize {
        let end =
            self.buffer[offset..].find('\n').map_or(self.buffer.len(), |index| offset + index);
        self.snap_out_of_part(end, true)
    }

    /// A word jump or deletion. Shift extends the selection with a jump; a deletion takes the
    /// selection instead when there is one.
    pub(super) fn run_text_command(&mut self, command: TextCommand, extending: bool) {
        let target = match command.reach {
            Reach::WordBefore => self.word_start_before(self.cursor),
            Reach::WordAfter => self.word_end_after(self.cursor),
            Reach::LineStart => self.line_start(self.cursor),
            Reach::LineEnd => self.line_end(self.cursor),
        };
        if command.delete {
            if self.delete_selection() {
                return;
            }
            self.selection_anchor = None;
            match command.reach {
                // Already at the start of a line: join it to the one above, as Backspace would
                Reach::LineStart if target == self.cursor => self.backspace(),
                _ => self.replace_range(target.min(self.cursor)..target.max(self.cursor), ""),
            }
            return;
        }
        if extending {
            self.selection_anchor.get_or_insert(self.cursor);
        } else {
            self.selection_anchor = None;
        }
        self.cursor = target;
        self.preferred_column = None;
    }

    /// Where the word before `offset` starts: past any whitespace (line breaks included), then a
    /// run of word characters or a run of punctuation. A paste placeholder is one word.
    fn word_start_before(&self, offset: usize) -> usize {
        let part_ending = |at: usize| self.parts.iter().find(|part| part.end == at);
        let before = |at: usize| self.buffer[..at].chars().next_back();
        let mut at = offset;
        while let Some(character) = before(at) {
            if part_ending(at).is_some() || !character.is_whitespace() {
                break;
            }
            at -= character.len_utf8();
        }
        if let Some(part) = part_ending(at) {
            return part.start;
        }
        let Some(class) = before(at).map(char_class) else {
            return at;
        };
        while let Some(character) = before(at) {
            if part_ending(at).is_some() || char_class(character) != class {
                break;
            }
            at -= character.len_utf8();
        }
        at
    }

    /// Where the word after `offset` ends, the mirror of [`Self::word_start_before`].
    fn word_end_after(&self, offset: usize) -> usize {
        let part_starting = |at: usize| self.parts.iter().find(|part| part.start == at);
        let after = |at: usize| self.buffer[at..].chars().next();
        let mut at = offset;
        while let Some(character) = after(at) {
            if part_starting(at).is_some() || !character.is_whitespace() {
                break;
            }
            at += character.len_utf8();
        }
        if let Some(part) = part_starting(at) {
            return part.end;
        }
        let Some(class) = after(at).map(char_class) else {
            return at;
        };
        while let Some(character) = after(at) {
            if part_starting(at).is_some() || char_class(character) != class {
                break;
            }
            at += character.len_utf8();
        }
        at
    }
}
