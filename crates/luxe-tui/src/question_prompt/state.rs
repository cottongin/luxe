//! Question prompt state — which row the cursor is on, what is checked, and what a key does.

use crate::editor::Editor;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use luxe_core::question::{default_selection, QuestionOption, QuestionRequestId};
use std::cell::Cell;

/// State for an in-flight question modal (one at a time — tools dispatch sequentially).
#[derive(Clone, Debug)]
pub struct QuestionPromptState {
    /// Correlates the answer with the awaiting engine turn.
    pub id: QuestionRequestId,
    /// The question text.
    pub prompt: String,
    /// Selectable options, each with its optional rationale.
    pub options: Vec<QuestionOption>,
    /// Whether multiple options may be selected.
    pub multi: bool,
    /// Cursor row: `0..options.len()` selects an option; `options.len()` is the free-text row.
    selected: usize,
    /// Per-option checkbox state (multi-select only).
    checked: Vec<bool>,
    /// The user's free-text answer: the composer's editor, so the caret moves and edits where it is.
    custom: Editor,
    /// Cells the answer last wrapped at on screen, so up and down move through the rows drawn.
    answer_width: Cell<u16>,
    /// Whether the free-text row is being edited (keystrokes go to `custom`).
    editing: bool,
    /// Whether the typed answer is checked (multi-select): typing checks it, Space toggles it.
    custom_checked: bool,
}

/// Outcome of a keypress on the question modal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QuestionPromptOutcome {
    /// Answer the question with these values (option labels and/or the free text) and close it.
    Answer(Vec<String>),
    /// Dismiss the question without answering.
    Cancel,
    /// Key consumed; the modal stays open.
    Consumed,
}

impl QuestionPromptState {
    pub fn new(
        id: QuestionRequestId,
        prompt: String,
        options: Vec<QuestionOption>,
        multi: bool,
    ) -> Self {
        let checked = vec![false; options.len()];
        let selected = default_selection(&options);
        Self {
            id,
            prompt,
            options,
            multi,
            selected,
            checked,
            custom: Editor::new(),
            answer_width: Cell::new(u16::MAX),
            editing: false,
            custom_checked: false,
        }
    }

    /// Index of the free-text ("type your own") row.
    pub(super) fn custom_row(&self) -> usize {
        self.options.len()
    }

    /// Total selectable rows (options + the free-text row).
    pub(super) fn row_count(&self) -> usize {
        self.options.len() + 1
    }

    /// Highlighted row index (for rendering / clicks).
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Whether row `i` is checked (multi-select), the typed answer included.
    pub fn is_checked(&self, i: usize) -> bool {
        if i == self.custom_row() {
            return self.custom_checked && !self.custom().trim().is_empty();
        }
        self.checked.get(i).copied().unwrap_or(false)
    }

    /// The current free-text entry.
    pub fn custom(&self) -> &str {
        self.custom.current_input()
    }

    /// The free-text entry's editor, for drawing it with its caret and selection.
    pub(crate) fn answer(&self) -> &Editor {
        &self.custom
    }

    /// Records the width the answer was just drawn at; up and down wrap it the same way.
    pub(super) fn set_answer_width(&self, width: u16) {
        self.answer_width.set(width);
    }

    /// Whether the free-text row is in edit mode.
    pub fn editing(&self) -> bool {
        self.editing
    }

    /// Take pasted text into the free-text row, jumping to it the way typing does.
    pub fn paste(&mut self, text: &str) {
        let text = crate::editor::single_line_paste(text);
        if text.is_empty() {
            return;
        }
        self.start_typing();
        self.custom.insert_text(&text);
    }

    /// Move the cursor to `index` (a hover or a click), clamped to range.
    ///
    /// Landing on the row already selected keeps an answer being typed open: the pointer resting
    /// on it is not a reason to drop the caret.
    pub fn select(&mut self, index: usize) {
        let index = index.min(self.row_count().saturating_sub(1));
        if index != self.selected {
            self.editing = false;
        }
        self.selected = index;
    }

    /// Put the cursor in the typed answer and pick it, as typing anywhere does.
    fn start_typing(&mut self) {
        self.selected = self.custom_row();
        self.editing = true;
        self.custom_checked = true;
    }

    fn move_up(&mut self) {
        let n = self.row_count();
        self.selected = if self.selected == 0 { n - 1 } else { self.selected - 1 };
    }

    pub(super) fn move_down(&mut self) {
        self.selected = (self.selected + 1) % self.row_count();
    }

    /// Collects the final answer: every checked row (multi), or the one chosen (single), where
    /// `None` is the typed answer.
    ///
    /// A single answer is one answer. Text left in the typed row while picking another option used
    /// to ride along with it, which made a choice and an abandoned draft one reply.
    fn collect(&self, option: Option<usize>) -> Vec<String> {
        let custom = self.custom().trim();
        if !self.multi {
            return match option {
                Some(i) => vec![self.options[i].label.clone()],
                None if custom.is_empty() => Vec::new(),
                None => vec![custom.to_string()],
            };
        }
        let mut answers: Vec<String> = (0..self.options.len())
            .filter(|i| self.checked[*i])
            .map(|i| self.options[i].label.clone())
            .collect();
        if self.is_checked(self.custom_row()) {
            answers.push(custom.to_string());
        }
        answers
    }

    /// Route a keypress. Everything not understood is swallowed (the modal is modal), so stray
    /// keystrokes never leak into the composer while an answer is pending.
    pub fn on_key(&mut self, key: &KeyEvent) -> QuestionPromptOutcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if self.editing {
            return self.on_key_editing(key, ctrl);
        }
        match key.code {
            KeyCode::Esc => QuestionPromptOutcome::Cancel,
            KeyCode::Char('c') if ctrl => QuestionPromptOutcome::Cancel,
            KeyCode::Up => {
                self.move_up();
                QuestionPromptOutcome::Consumed
            }
            KeyCode::Down | KeyCode::Tab => {
                self.move_down();
                QuestionPromptOutcome::Consumed
            }
            // Digit shortcut: pick / toggle the nth option, the typed answer's number included.
            KeyCode::Char(c @ '1'..='9') => {
                let i = (c as usize) - ('1' as usize);
                if i == self.custom_row() {
                    self.start_typing();
                    QuestionPromptOutcome::Consumed
                } else if i < self.options.len() {
                    if self.multi {
                        self.checked[i] = !self.checked[i];
                        self.selected = i;
                        QuestionPromptOutcome::Consumed
                    } else {
                        QuestionPromptOutcome::Answer(self.collect(Some(i)))
                    }
                } else {
                    QuestionPromptOutcome::Consumed
                }
            }
            KeyCode::Char(' ') => {
                if self.selected == self.custom_row() {
                    if self.multi && self.is_checked(self.selected) {
                        self.custom_checked = false;
                    } else {
                        self.start_typing();
                    }
                } else if self.multi {
                    self.checked[self.selected] = !self.checked[self.selected];
                }
                QuestionPromptOutcome::Consumed
            }
            KeyCode::Enter => {
                if self.multi {
                    QuestionPromptOutcome::Answer(self.collect(None))
                } else if self.selected == self.custom_row() {
                    if self.custom().trim().is_empty() {
                        self.editing = true;
                        QuestionPromptOutcome::Consumed
                    } else {
                        QuestionPromptOutcome::Answer(self.collect(None))
                    }
                } else {
                    QuestionPromptOutcome::Answer(self.collect(Some(self.selected)))
                }
            }
            // Start typing anywhere jumps into the free-text row.
            KeyCode::Char(_) if !ctrl => {
                self.start_typing();
                self.custom.handle_key(*key);
                QuestionPromptOutcome::Consumed
            }
            _ => QuestionPromptOutcome::Consumed,
        }
    }

    fn on_key_editing(&mut self, key: &KeyEvent, ctrl: bool) -> QuestionPromptOutcome {
        match key.code {
            KeyCode::Esc => {
                // Leave edit mode but keep the modal open (Esc again cancels it).
                self.editing = false;
                QuestionPromptOutcome::Consumed
            }
            KeyCode::Char('c') if ctrl => {
                self.custom.select_all();
                self.custom.delete_selection();
                QuestionPromptOutcome::Consumed
            }
            KeyCode::Up | KeyCode::Down => self.vertical_in_answer(key),
            // The answer is one reply on one line, and Tab means nothing in it
            KeyCode::Tab | KeyCode::BackTab => QuestionPromptOutcome::Consumed,
            // Every Enter, Shift+Enter's newline included, is the question's to answer
            KeyCode::Enter => {
                if self.multi {
                    // Commit the text and submit everything checked plus the free text.
                    self.editing = false;
                    QuestionPromptOutcome::Answer(self.collect(None))
                } else if self.custom().trim().is_empty() {
                    self.editing = false;
                    QuestionPromptOutcome::Consumed
                } else {
                    QuestionPromptOutcome::Answer(self.collect(None))
                }
            }
            // Typing, deleting and every caret motion are the composer's own
            _ => {
                let before = self.custom().len();
                self.custom.handle_key(*key);
                if self.custom().len() > before {
                    self.custom_checked = true;
                }
                QuestionPromptOutcome::Consumed
            }
        }
    }

    /// Up and down move through the rows the answer wraps to; from its first row up, or its last
    /// row down, they leave it for the options, as the launcher hands its arrows to the list.
    /// Shift always stays in the answer, where it is selecting.
    fn vertical_in_answer(&mut self, key: &KeyEvent) -> QuestionPromptOutcome {
        self.custom.set_viewport_width(self.answer_width.get());
        let up = key.code == KeyCode::Up;
        let edge =
            if up { self.custom.cursor_on_first_row() } else { self.custom.cursor_on_last_row() };
        if edge && !key.modifiers.contains(KeyModifiers::SHIFT) {
            self.editing = false;
            if up {
                self.move_up();
            } else {
                self.move_down();
            }
        } else {
            self.custom.handle_key(*key);
        }
        QuestionPromptOutcome::Consumed
    }
}
