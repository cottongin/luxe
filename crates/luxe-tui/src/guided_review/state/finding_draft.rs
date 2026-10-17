//! The finding draft — one anchor and the comment written against it, and what saving makes of it.

use luxe_core::guided_review::{
    FindingAnchor, FindingId, FindingSeverity, FindingState, ReviewFinding,
    ReviewSessionValidationError,
};

/// Unsaved reviewer input associated with one immutable diff anchor.
///
/// One anchor and one piece of prose. The composer briefly asked for a title, a proposed fix and a
/// severity around it; all three asked the reviewer to split a thought that arrives whole, and none
/// told the agent anything the comment had not already said. Severity survives as a field because
/// the verb sets it — `c` means the agent should act, `a` means it must answer and not edit — but
/// nothing picks it once the box is open.
#[derive(Debug, Clone)]
pub(crate) struct FindingDraft {
    /// The source location captured when the draft opens.
    pub(crate) anchor: FindingAnchor,
    /// What this asks of the agent, fixed by the verb that opened the draft
    pub(crate) severity: FindingSeverity,
    /// A remediation an *earlier* draft carried, preserved untouched through a reopen.
    ///
    /// Nothing writes this any more. It exists so reopening a draft persisted by a build that
    /// still had the field does not silently throw the reviewer's suggestion away on the next save.
    carried_fix: Option<String>,
    /// The real editor backing the comment.
    ///
    /// This is the one place a reviewer writes prose, so it reuses Luxe's composer wholesale and
    /// behaves like every other text input: a cursor, arrow keys, word operations.
    editor: crate::editor::Editor,
    /// Why the last save was rejected, cleared by the next edit. Shown on screen, or `Ctrl+Enter`
    /// would look like a dead key.
    pub(crate) error: Option<String>,
    /// Send this straight to the agent on save instead of holding it as a draft.
    ///
    /// A question left sitting in a draft answers nothing, so `a` dispatches immediately while `c`
    /// batches for a later send.
    pub(crate) dispatch_on_save: bool,
    /// Existing finding this draft replies to, when it is a thread reply rather than a new comment.
    pub(crate) reply_to: Option<FindingId>,
}

impl FindingDraft {
    /// Builds an empty draft against one anchor.
    pub(crate) fn new(
        anchor: FindingAnchor,
        severity: FindingSeverity,
        dispatch_on_save: bool,
        reply_to: Option<FindingId>,
    ) -> Self {
        Self {
            anchor,
            severity,
            carried_fix: None,
            editor: crate::editor::Editor::new(),
            error: None,
            dispatch_on_save,
            reply_to,
        }
    }

    /// The reviewer's comment, read live off the editor.
    pub(crate) fn body(&self) -> &str {
        self.editor.current_input()
    }

    /// Replaces the comment, loading it into the editor.
    pub(crate) fn set_body(&mut self, text: impl Into<String>) {
        let text = text.into();
        self.editor.cancel();
        self.editor.insert_text(&text);
        self.error = None;
    }

    /// Keeps a remediation an earlier build authored, so a reopen does not drop it.
    pub(crate) fn carry_fix(&mut self, fix: Option<String>) {
        self.carried_fix = fix.filter(|fix| !fix.trim().is_empty());
    }

    /// The live editor, so the overlay can draw a real cursor.
    pub(crate) fn editor(&self) -> &crate::editor::Editor {
        &self.editor
    }

    /// The width the field is drawn at, so `↑/↓` moves by the rows the reader sees.
    pub(crate) fn set_width(&mut self, width: u16) {
        self.editor.set_viewport_width(width);
    }

    /// Routes one key to the focused field's editor.
    ///
    /// `Enter` inserts a newline instead of submitting: the composer's save is `Ctrl+Enter`, and a
    /// stray `Enter` must never fire the chat submit behind the review.
    pub(crate) fn handle_key(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::KeyCode;
        self.error = None;
        match key.code {
            KeyCode::Enter => self.editor.insert_newline(),
            _ => self.editor.handle_key(key),
        }
    }

    /// Inserts pasted text into the focused field.
    ///
    /// Without this, a paste while the composer was open reached the *chat* editor behind the
    /// review — from the reviewer's point of view the text simply vanished.
    pub(crate) fn paste(&mut self, text: &str) {
        self.error = None;
        self.editor.paste_text(text);
    }

    /// The title the comment will carry: its own opening line.
    ///
    /// There is no title field. Requiring one made the common case — a sentence of feedback — fail
    /// to save, and once it was optional it became a box that echoed what you were already typing
    /// into the box above it, one line behind. The durable record still has a title because the
    /// drawer, the send preview and the agent's message all need something short to name a comment
    /// by; deriving it costs the reviewer nothing.
    pub(crate) fn resolved_title(&self) -> String {
        luxe_core::guided_review::title_from_body(self.body())
    }

    /// Builds a validated core finding without consuming the local draft.
    pub(crate) fn to_review_finding(
        &self,
        id: FindingId,
    ) -> Result<ReviewFinding, ReviewSessionValidationError> {
        let finding = ReviewFinding {
            posted: None,
            github_thread: None,
            id,
            // The session assigns the next `R<n>` on insert; an edit keeps the one it already has.
            label: String::new(),
            anchor: self.anchor.clone(),
            severity: self.severity,
            title: self.resolved_title(),
            body: self.body().to_owned(),
            proposed_fix: self.carried_fix.clone(),
            state: FindingState::Draft,
            thread: Vec::new(),
        };
        finding.validate()?;
        Ok(finding)
    }
}
