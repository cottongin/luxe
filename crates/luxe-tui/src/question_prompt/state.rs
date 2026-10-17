//! Question prompt state — which row the cursor is on, what is checked, and what a key does.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use luxe_core::question::{default_selection, QuestionOption, QuestionRequestId};

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
    /// The user's free-text answer.
    custom: String,
    /// Whether the free-text row is being edited (keystrokes append to `custom`).
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
            custom: String::new(),
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
            return self.custom_checked && !self.custom.trim().is_empty();
        }
        self.checked.get(i).copied().unwrap_or(false)
    }

    /// The current free-text entry.
    pub fn custom(&self) -> &str {
        &self.custom
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
        self.custom.push_str(&text);
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
        let custom = self.custom.trim();
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
                    if self.custom.trim().is_empty() {
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
            KeyCode::Char(c) if !ctrl => {
                self.start_typing();
                self.custom.push(c);
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
                self.custom.clear();
                QuestionPromptOutcome::Consumed
            }
            KeyCode::Backspace => {
                self.custom.pop();
                QuestionPromptOutcome::Consumed
            }
            KeyCode::Enter => {
                if self.multi {
                    // Commit the text and submit everything checked plus the free text.
                    self.editing = false;
                    QuestionPromptOutcome::Answer(self.collect(None))
                } else if self.custom.trim().is_empty() {
                    self.editing = false;
                    QuestionPromptOutcome::Consumed
                } else {
                    QuestionPromptOutcome::Answer(self.collect(None))
                }
            }
            KeyCode::Char(c) if !ctrl => {
                self.custom.push(c);
                self.custom_checked = true;
                QuestionPromptOutcome::Consumed
            }
            _ => QuestionPromptOutcome::Consumed,
        }
    }
}
