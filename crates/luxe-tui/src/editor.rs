//! Editor — multiline prompt composition with compact paste parts.
//!
//! The display buffer stays small while tracked paste parts retain the exact text or image
//! attachment submitted to the engine. Cursor movement treats those parts as atomic tokens.

mod layout;
mod motion;
mod parts;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
pub use layout::VisualCursor;
pub(crate) use layout::{TextLayout, VisualSegment};
use motion::text_command;
pub use parts::AttachedImage;
use parts::{shift_offset, PromptPart, PromptPartKind};

const DEFAULT_VIEWPORT_WIDTH: u16 = 80;

/// Submitted messages history keeps; past it the oldest go, pastes and all.
const HISTORY_LIMIT: usize = 500;
/// What a one-line field takes from a paste.
///
/// Every surface with a single-line box — an API key prompt, a picker's search, a question's
/// free-text row — needs the same answer, and four copies of it is four chances to disagree. Line
/// breaks and control characters are dropped rather than turned into spaces: a key that wrapped in
/// a console is still one key, and a stray newline that became Enter would submit it half-read.
pub fn single_line_paste(text: &str) -> String {
    text.chars().filter(|character| !character.is_control()).collect::<String>().trim().to_string()
}

/// Editor state for multiline prompt input.
#[derive(Debug, Clone)]
pub struct Editor {
    /// Compact display buffer
    buffer: String,
    /// Cursor position as a byte offset in the display buffer
    cursor: usize,
    /// Previous submitted prompts, newest last, at most [`HISTORY_LIMIT`]
    history: Vec<String>,
    /// How many steps back into `history` the draft is showing; `None` is the reader's own draft
    history_index: Option<usize>,
    parts: Vec<PromptPart>,
    viewport_width: u16,
    preferred_column: Option<usize>,
    /// Byte offset the active selection started at; the cursor is its other end.
    selection_anchor: Option<usize>,
}

impl Default for Editor {
    fn default() -> Self {
        Self {
            buffer: String::new(),
            cursor: 0,
            history: Vec::new(),
            history_index: None,
            parts: Vec::new(),
            viewport_width: DEFAULT_VIEWPORT_WIDTH,
            preferred_column: None,
            selection_anchor: None,
        }
    }
}

impl Editor {
    /// Creates an empty editor.
    pub fn new() -> Self {
        Self::default()
    }

    /// An editor holding `text` as typed, with the cursor at byte `cursor`.
    #[cfg(test)]
    pub(crate) fn with_text(text: &str, cursor: usize) -> Self {
        let mut editor = Self::new();
        editor.insert_text(text);
        editor.cursor = cursor;
        editor
    }

    /// Cursor position as a byte offset in [`Self::current_input`].
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// The draft as it would be sent, expanded text and image attachments, leaving it in place.
    pub fn draft(&self) -> (String, Vec<AttachedImage>) {
        let images = self
            .parts
            .iter()
            .filter_map(|part| match &part.kind {
                PromptPartKind::Image { image } => Some(image.clone()),
                PromptPartKind::PastedText { .. } | PromptPartKind::Reference => None,
            })
            .collect();
        (self.expanded_input(), images)
    }

    /// Submits expanded text and image attachments, then clears the draft.
    pub fn submit(&mut self) -> Option<(String, Vec<AttachedImage>)> {
        let (text, images) = self.draft();
        // Whitespace is nothing to send: a stray space and Enter must not submit it
        if text.trim().is_empty() && images.is_empty() {
            return None;
        }

        self.history.push(text.clone());
        if self.history.len() > HISTORY_LIMIT {
            self.history.remove(0);
        }
        self.clear_draft();
        Some((text, images))
    }

    /// Clears the current draft and attachments.
    pub fn cancel(&mut self) {
        self.clear_draft();
        self.history_index = None;
    }

    /// Handles editing, cursor movement, and history navigation for one key event.
    ///
    /// Plain Enter does nothing here: what it means (send, accept, run) is the caller's to decide,
    /// and [`Self::submit`] is how a caller that sends takes the draft.
    pub fn handle_key(&mut self, key: KeyEvent) {
        self.clamp_cursor();
        if let Some(command) = text_command(&key) {
            self.run_text_command(command, key.modifiers.contains(KeyModifiers::SHIFT));
            return;
        }
        // Shift + a movement key extends the selection from wherever it started; anything else
        // drops it, and an edit replaces it (the usual editor contract).
        let extending = key.modifiers.contains(KeyModifiers::SHIFT)
            && matches!(
                key.code,
                KeyCode::Left
                    | KeyCode::Right
                    | KeyCode::Up
                    | KeyCode::Down
                    | KeyCode::Home
                    | KeyCode::End
            );
        if extending {
            self.selection_anchor.get_or_insert(self.cursor);
        } else if matches!(key.code, KeyCode::Backspace | KeyCode::Delete)
            || matches!(key.code, KeyCode::Char(_) if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT))
            || (key.code == KeyCode::Enter && key.modifiers.contains(KeyModifiers::SHIFT))
        {
            if self.delete_selection() && matches!(key.code, KeyCode::Backspace | KeyCode::Delete) {
                return;
            }
        } else {
            self.selection_anchor = None;
        }
        match key.code {
            KeyCode::Enter if key.modifiers.contains(KeyModifiers::SHIFT) => self.insert_newline(),
            KeyCode::Esc => self.cancel(),
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => self.delete(),
            KeyCode::Left => self.move_left(),
            KeyCode::Right => self.move_right(),
            KeyCode::Up => self.move_up(),
            KeyCode::Down => self.move_down(),
            KeyCode::Home if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cursor = 0;
                self.preferred_column = None;
            }
            KeyCode::End if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cursor = self.buffer.len();
                self.preferred_column = None;
            }
            KeyCode::Home => self.move_to_logical_line_start(),
            KeyCode::End => self.move_to_logical_line_end(),
            KeyCode::Char(character)
                if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.insert_text(&character.to_string());
            }
            _ => {}
        }
    }

    /// Inserts a newline at the cursor.
    pub fn insert_newline(&mut self) {
        self.insert_text("\n");
    }

    /// Inserts literal text at the cursor.
    pub fn insert_text(&mut self, text: &str) {
        self.replace_range(self.cursor..self.cursor, text);
    }

    /// Replaces a display-buffer byte range and keeps tracked parts aligned.
    pub fn replace_range(&mut self, range: std::ops::Range<usize>, text: &str) {
        if range.start > range.end
            || range.end > self.buffer.len()
            || !self.buffer.is_char_boundary(range.start)
            || !self.buffer.is_char_boundary(range.end)
        {
            return;
        }

        let removed = range.end - range.start;
        let inserted = text.len();
        self.parts.retain(|part| range.end <= part.start || range.start >= part.end);
        for part in &mut self.parts {
            if part.start >= range.end {
                part.start = shift_offset(part.start, inserted, removed);
                part.end = shift_offset(part.end, inserted, removed);
            }
        }
        let start = range.start;
        self.buffer.replace_range(range, text);
        self.cursor = start + inserted;
        self.preferred_column = None;
    }

    /// Returns the compact text shown in the composer.
    pub fn current_input(&self) -> &str {
        &self.buffer
    }

    /// Returns prompt text with tracked paste tokens expanded and image tokens removed.
    pub fn expanded_input(&self) -> String {
        self.expand_parts(0..self.buffer.len())
    }

    /// Starts a selection at the current cursor (a mouse press or the first shift-move).
    pub fn begin_selection(&mut self) {
        self.selection_anchor = Some(self.cursor);
    }

    /// Drops any active selection.
    pub fn clear_selection(&mut self) {
        self.selection_anchor = None;
    }

    /// The selected byte range, or `None` when nothing is selected.
    pub fn selection(&self) -> Option<std::ops::Range<usize>> {
        let anchor = self.selection_anchor?;
        let (start, end) =
            if anchor <= self.cursor { (anchor, self.cursor) } else { (self.cursor, anchor) };
        (start < end).then_some(start..end)
    }

    /// The selected text, with paste tokens expanded so the clipboard carries what was pasted
    /// rather than the `[Pasted N lines]` placeholder.
    pub fn selected_text(&self) -> Option<String> {
        self.selection().map(|range| self.expand_parts(range))
    }

    /// Removes the selection, if any. Returns whether anything was deleted.
    pub fn delete_selection(&mut self) -> bool {
        let Some(range) = self.selection() else {
            self.selection_anchor = None;
            return false;
        };
        self.replace_range(range, "");
        self.selection_anchor = None;
        true
    }

    /// Selects the whole draft.
    pub fn select_all(&mut self) {
        self.selection_anchor = Some(0);
        self.cursor = self.buffer.len();
    }

    fn clear_draft(&mut self) {
        self.buffer.clear();
        self.cursor = 0;
        self.parts.clear();
        self.preferred_column = None;
    }

    fn clamp_cursor(&mut self) {
        self.cursor = self.cursor.min(self.buffer.len());
        while self.cursor > 0 && !self.buffer.is_char_boundary(self.cursor) {
            self.cursor -= 1;
        }
        self.cursor = self.snap_out_of_part(self.cursor, false);
    }

    fn backspace(&mut self) {
        if let Some(part) = self.parts.iter().find(|part| part.end == self.cursor) {
            self.replace_range(part.start..part.end, "");
            return;
        }
        if self.cursor == 0 {
            return;
        }
        if let Some((start, _)) = self.buffer[..self.cursor].char_indices().next_back() {
            self.replace_range(start..self.cursor, "");
        }
    }

    fn delete(&mut self) {
        if let Some(part) = self.parts.iter().find(|part| part.start == self.cursor) {
            self.replace_range(part.start..part.end, "");
            return;
        }
        if let Some(character) = self.buffer[self.cursor..].chars().next() {
            self.replace_range(self.cursor..self.cursor + character.len_utf8(), "");
        }
    }

    fn history_previous(&mut self) {
        let back = self.history_index.map_or(0, |back| back + 1);
        if back >= self.history.len() {
            return;
        }
        self.history_index = Some(back);
        self.buffer.clone_from(&self.history[self.history.len() - 1 - back]);
        self.parts.clear();
        self.cursor = 0;
        self.preferred_column = None;
    }

    fn history_next(&mut self) {
        match self.history_index {
            Some(back) if back > 0 => {
                self.history_index = Some(back - 1);
                self.buffer.clone_from(&self.history[self.history.len() - back]);
                self.parts.clear();
                self.cursor = self.buffer.len();
            }
            Some(_) => {
                self.history_index = None;
                self.buffer.clear();
                self.parts.clear();
                self.cursor = 0;
            }
            None => {}
        }
        self.preferred_column = None;
    }
}

#[cfg(test)]
mod tests;
