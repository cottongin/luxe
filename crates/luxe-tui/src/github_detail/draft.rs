//! The draft card — a reply, comment or review summary read in full before it is sent.
//!
//! The agent writes and you send, or you write and send: a draft arrives here rendered as it will
//! read on GitHub, under a line saying where it goes and who wrote it, with what it answers quoted
//! above it. Nothing posts without Enter. The same shape as *Open pull request* — `e` edits in
//! place, Ctrl+Enter keeps it, `E` hands it to your editor — so an edited reply and an edited pull
//! request description are one gesture.

use std::cell::Cell;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Padding, Paragraph};
use ratatui::Frame;

use crate::editor::Editor;
use crate::github_detail::Draft;
use crate::glyphs::Icon;
use crate::theme::styles::ThemeStyles;

/// Lines of the comment being answered that the card quotes.
const QUOTE_LINES: usize = 3;

/// A draft on screen: what it says, what it answers, and whether it is being written.
#[derive(Debug, Clone)]
pub struct DraftCard {
    /// The draft
    pub draft: Draft,
    /// Who wrote what it answers, and what they said, raw
    pub answering: Option<(String, String)>,
    /// The editor, while it is being written
    pub editor: Option<Editor>,
    /// The editor's width when last drawn, which moving the cursor up and down needs
    pub field_width: Cell<u16>,
    /// Whether it has been sent and the answer is still coming
    pub posting: bool,
    /// Why the last attempt did not land, as a sentence
    pub error: Option<String>,
    /// What it answers, which asking the agent to draft it needs
    pub target: Option<luxe_core::engine::DraftTarget>,
    /// The request the agent is drafting it under, while it is
    pub drafting: Option<String>,
    /// What should change, while it is being written for a redraft
    pub note: Option<Editor>,
    /// Rows of the draft scrolled past while it is read
    pub scroll: usize,
    /// The farthest the draft can scroll and the rows it shows, as last drawn
    pub reading_room: Cell<(usize, usize)>,
}

/// What a key asks of whoever holds the card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DraftKey {
    /// The card handled it
    Held,
    /// Put it away with nothing written
    Close,
    /// Put it away, keeping what was written
    Keep,
    /// Send it
    Post,
    /// Hand the text to the reader's editor
    EditExternally,
    /// Ask the agent to draft it, again when there is text, with what should change
    Redraft(Option<String>),
}

impl DraftCard {
    /// A draft read before it is sent, not being written.
    pub fn reading(draft: Draft, answering: Option<(String, String)>) -> Self {
        Self {
            draft,
            answering,
            editor: None,
            field_width: Cell::new(0),
            posting: false,
            error: None,
            target: None,
            drafting: None,
            note: None,
            scroll: 0,
            reading_room: Cell::new((0, 0)),
        }
    }

    /// A draft the agent is writing under `request_id`, answering `target`.
    pub fn drafting(
        draft: Draft,
        answering: Option<(String, String)>,
        target: luxe_core::engine::DraftTarget,
        request_id: String,
    ) -> Self {
        Self { target: Some(target), drafting: Some(request_id), ..Self::reading(draft, answering) }
    }

    /// Lands the agent's draft, or why there is none. What the reader had written is replaced only
    /// by a draft that arrived.
    ///
    /// Args: `(body, sources, error, note)`.
    pub fn drafted(
        &mut self,
        body: Option<String>,
        sources: Vec<String>,
        error: Option<String>,
        note: Option<String>,
    ) {
        self.drafting = None;
        match body {
            Some(body) => {
                self.draft.body = body;
                self.scroll = 0;
                self.draft.by_agent = true;
                self.draft.sources = sources;
                self.draft.note = note;
                self.editor = None;
                self.error = None;
            }
            None => self.error = Some(error.unwrap_or_else(|| "No draft returned".into())),
        }
    }

    /// A reply in your own words, the editor open on nothing.
    pub fn writing(draft: Draft, answering: Option<(String, String)>) -> Self {
        Self { editor: Some(Editor::new()), ..Self::reading(draft, answering) }
    }

    /// What `key` does. While writing, keys type; Ctrl+Enter keeps the text and Esc stops writing.
    pub fn on_key(&mut self, key: &KeyEvent) -> DraftKey {
        if self.posting {
            return DraftKey::Held;
        }
        // Being drafted: Esc puts it away, and the draft is kept under what it answers when it lands
        if self.drafting.is_some() {
            return match key.code {
                KeyCode::Esc => DraftKey::Close,
                _ => DraftKey::Held,
            };
        }
        if let Some(note) = self.note.as_mut() {
            match key.code {
                KeyCode::Enter => {
                    let said = note.current_input().trim().to_string();
                    self.note = None;
                    return DraftKey::Redraft((!said.is_empty()).then_some(said));
                }
                KeyCode::Esc => self.note = None,
                _ => note.handle_key(*key),
            }
            return DraftKey::Held;
        }
        if let Some(editor) = self.editor.as_mut() {
            editor.set_viewport_width(self.field_width.get());
            match key.code {
                KeyCode::Enter if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.draft.body = editor.current_input().trim_end().to_string();
                    self.editor = None;
                    self.scroll = 0;
                }
                // Backing out of a reply never started puts the card away; one with words keeps them
                KeyCode::Esc if self.draft.body.trim().is_empty() => return DraftKey::Close,
                KeyCode::Esc => self.editor = None,
                KeyCode::Enter => editor.insert_newline(),
                _ => editor.handle_key(*key),
            }
            return DraftKey::Held;
        }
        if self.read(key) {
            return DraftKey::Held;
        }
        match key.code {
            KeyCode::Enter if !self.draft.body.trim().is_empty() => {
                self.error = None;
                self.posting = true;
                DraftKey::Post
            }
            KeyCode::Char('e') if key.modifiers.is_empty() => {
                let mut editor = Editor::new();
                editor.insert_text(&self.draft.body);
                self.editor = Some(editor);
                DraftKey::Held
            }
            KeyCode::Char('E') => DraftKey::EditExternally,
            // With nothing written, the agent drafts it from scratch; with text, it asks what to change
            KeyCode::Char('d') if key.modifiers.is_empty() && self.target.is_some() => {
                if self.draft.body.trim().is_empty() {
                    return DraftKey::Redraft(None);
                }
                self.note = Some(Editor::new());
                DraftKey::Held
            }
            KeyCode::Esc if self.draft.body.trim().is_empty() => DraftKey::Close,
            KeyCode::Esc => DraftKey::Keep,
            _ => DraftKey::Held,
        }
    }

    /// Text pasted into the card: into the editor when it is open, nowhere otherwise.
    pub fn paste(&mut self, text: &str) {
        if let Some(note) = self.note.as_mut() {
            note.insert_text(&crate::editor::single_line_paste(text));
        } else if let Some(editor) = self.editor.as_mut() {
            editor.insert_text(text);
        }
    }

    /// Text back from the reader's editor, which replaces the draft.
    pub fn edited(&mut self, text: String) {
        self.draft.body = text.trim_end().to_string();
        self.editor = None;
        self.scroll = 0;
    }

    /// Moves the reading window for a reading key, and says whether `key` was one. Bounded by what
    /// was last drawn, so the end of a long draft is in reach and nothing past it.
    fn read(&mut self, key: &KeyEvent) -> bool {
        let (room, shown) = self.reading_room.get();
        let page = shown.saturating_sub(1).max(1);
        self.scroll = match key.code {
            KeyCode::Up => self.scroll.saturating_sub(1),
            KeyCode::Down => self.scroll + 1,
            KeyCode::PageUp => self.scroll.saturating_sub(page),
            KeyCode::PageDown => self.scroll + page,
            KeyCode::Home => 0,
            KeyCode::End => room,
            _ => return false,
        }
        .min(room);
        true
    }

    fn hint(&self) -> &'static str {
        let agent = self.target.is_some();
        match (self.posting, &self.editor) {
            (true, _) => "posting…",
            _ if self.drafting.is_some() => "drafting… · esc keep it for later",
            _ if self.note.is_some() => "enter redraft · esc cancel",
            (false, Some(_)) => "ctrl+enter keep · esc stop writing",
            // `E` works on an empty card too, and writing a long reply is where an editor helps
            (false, None) if self.draft.body.trim().is_empty() && agent => {
                "e write · E in your editor · d draft with agent · esc close"
            }
            (false, None) if self.draft.body.trim().is_empty() => {
                "e write · E in your editor · esc close"
            }
            (false, None) if agent => {
                "enter post · e edit · E in your editor · d redraft · esc keep"
            }
            (false, None) => "enter post · e edit · E in your editor · esc keep",
        }
    }
}

/// What the agent said beside a draft, wrapped to `width`: a caveat, usually, so it is set apart
/// from the draft and read before sending rather than in the conversation behind it.
pub(crate) fn note_lines(note: &str, width: usize, styles: &ThemeStyles) -> Vec<Line<'static>> {
    let said = note.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut spans = vec![Span::styled("Agent's note · ", styles.warning)];
    spans.extend(crate::widgets::code_spans(&said, styles.muted, styles));
    crate::widgets::wrap_line(&Line::from(spans), width)
}

/// What sits above the body: who drafted it and for what, where it came from, and the comment
/// being answered.
fn head_lines(card: &DraftCard, inner: usize, styles: &ThemeStyles) -> Vec<Line<'static>> {
    let mut head = Vec::new();
    let who = if card.draft.by_agent {
        " · drafted by the agent in this session"
    } else {
        " · in your words"
    };
    let mut target = crate::widgets::code_spans(&card.draft.target, styles.accent, styles);
    target.push(Span::styled(who, styles.muted));
    head.extend(crate::widgets::wrap_line(&Line::from(target), inner));
    if !card.draft.sources.is_empty() {
        let from = Line::from(vec![
            Span::styled("From ", styles.muted),
            Span::styled(card.draft.sources.join(" · "), styles.text),
        ]);
        head.extend(crate::widgets::wrap_line(&from, inner));
    }
    if let Some(note) = card.draft.note.as_deref() {
        head.extend(note_lines(note, inner, styles));
    }
    if let Some((author, said)) = &card.answering {
        head.push(Line::default());
        let said = luxe_core::github::untrusted(said, 2_000);
        let said = said.split_whitespace().collect::<Vec<_>>().join(" ");
        let quote = Line::from(vec![
            Span::styled(format!("{author}: "), styles.info),
            Span::styled(said, styles.muted),
        ]);
        let quoted = crate::widgets::wrap_line(&quote, inner.saturating_sub(2));
        let cut = quoted.len() > QUOTE_LINES;
        for (index, line) in quoted.into_iter().take(QUOTE_LINES).enumerate() {
            let mut spans = vec![Span::styled("│ ", styles.border_muted.patch(styles.dim))];
            spans.extend(line.spans);
            if cut && index + 1 == QUOTE_LINES {
                spans.push(Span::styled(crate::glyphs::Mark::Elision.glyph(), styles.muted));
            }
            head.push(Line::from(spans));
        }
    }
    head.push(Line::default());
    head
}

/// What sits under the body: the error, and whatever is still in flight.
fn tail_lines(card: &DraftCard, frame: usize, styles: &ThemeStyles) -> Vec<Line<'static>> {
    let mut tail = Vec::new();
    if let Some(error) = &card.error {
        tail.push(Line::from(crate::widgets::code_spans(error, styles.error, styles)));
    }
    if card.posting {
        let spinner = crate::statusline::spinner(frame);
        tail.push(Line::from(Span::styled(format!("{spinner} Posting to GitHub…"), styles.muted)));
    }
    if card.drafting.is_some() {
        let spinner = crate::statusline::spinner(frame);
        tail.push(Line::from(Span::styled(
            format!("{spinner} The agent is drafting it in this session…"),
            styles.muted,
        )));
    }
    tail
}

/// Draws the card over `area`, the surface it was opened from.
///
/// Args: `(frame, area, card, spinner frame, styles)`.
pub(super) fn render(
    f: &mut Frame,
    area: Rect,
    card: &DraftCard,
    frame: usize,
    styles: &ThemeStyles,
) {
    // Wide enough for a paragraph at a reading measure, and most of the height: what an agent
    // drafts runs long, and reading it in full is the point of the card
    let width = area.width.saturating_sub(12).clamp(40, 112).min(area.width);
    let inner = usize::from(width.saturating_sub(4)).max(1);

    let head = head_lines(card, inner, styles);

    let tail = tail_lines(card, frame, styles);
    let note_rows: u16 = if card.note.is_some() { 2 } else { 0 };

    let body = match &card.editor {
        Some(_) => Vec::new(),
        None if card.draft.body.trim().is_empty() && card.drafting.is_some() => Vec::new(),
        None if card.draft.body.trim().is_empty() => {
            vec![Line::from(Span::styled("Empty", styles.muted))]
        }
        None => crate::widgets::wrap_lines(
            &crate::markdown::render_markdown(&card.draft.body, styles),
            inner,
        ),
    };
    let fits = head.len() + body.len().max(3) + tail.len() + 2;
    let fits = u16::try_from(fits + usize::from(crate::widgets::FOOTER_ROWS)).unwrap_or(u16::MAX);
    let height = fits.max(area.height * 3 / 4).min(area.height.saturating_sub(4));
    let popup = crate::widgets::centered_rect(width, height, area);
    let title = if card.answering.is_some() { "Reply" } else { "Comment" };
    let block = crate::widgets::titled_panel(Icon::GitHub.glyph(), title, styles)
        .padding(Padding::horizontal(1));
    f.render_widget(Clear, popup);
    let inside = crate::widgets::render_footed(f, block, popup, card.hint(), None, styles);

    let head_rows = u16::try_from(head.len()).unwrap_or(u16::MAX).min(inside.height);
    let tail_rows = u16::try_from(tail.len()).unwrap_or(u16::MAX);
    f.render_widget(Paragraph::new(head), Rect { height: head_rows, ..inside });
    let text = Rect {
        y: inside.y + head_rows,
        height: inside.height.saturating_sub(head_rows + tail_rows + note_rows),
        ..inside
    };
    match &card.editor {
        Some(editor) => {
            card.field_width.set(text.width);
            crate::widgets::text_field::render(f, text, editor, styles);
        }
        None => render_reading(f, text, (body, inner), card, styles),
    }
    f.render_widget(
        Paragraph::new(tail),
        Rect { y: text.bottom(), height: tail_rows.min(inside.height), ..inside },
    );
    if let Some(note) = &card.note {
        let at = text.bottom() + tail_rows;
        let label = Rect { y: at, height: 1, ..inside };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled("Redraft note", styles.accent))),
            label,
        );
        let field = Rect { y: at + 1, height: 1, ..inside };
        crate::widgets::text_field::render(f, field, note, styles);
    }
}

/// The draft as it is read: the window the reader scrolled to, and a scrollbar saying where it is.
fn render_reading(
    f: &mut Frame,
    text: Rect,
    (body, inner): (Vec<Line<'static>>, usize),
    card: &DraftCard,
    styles: &ThemeStyles,
) {
    let shown = usize::from(text.height);
    // Too long for the card, it gives the scrollbar a column of its own: drawn over the text, the
    // bar ate the last letter of every line
    let body = if body.len() > shown {
        crate::widgets::wrap_lines(
            &crate::markdown::render_markdown(&card.draft.body, styles),
            inner.saturating_sub(1).max(1),
        )
    } else {
        body
    };
    let total = body.len();
    let room = total.saturating_sub(shown);
    card.reading_room.set((room, shown));
    let offset = card.scroll.min(room);
    let lines = Rect { width: text.width.saturating_sub(u16::from(total > shown)), ..text };
    let scrolled = u16::try_from(offset).unwrap_or(u16::MAX);
    f.render_widget(Paragraph::new(body).scroll((scrolled, 0)), lines);
    if total > shown {
        crate::widgets::scrollbar(f, text, offset, total, shown, styles);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(body: &str) -> Draft {
        Draft {
            target: "Reply to @jon on `src/dispatch/router.rs:88`".into(),
            thread: Some("PRRT_1".into()),
            body: body.into(),
            by_agent: false,
            sources: Vec::new(),
            note: None,
        }
    }

    fn press(card: &mut DraftCard, code: KeyCode, modifiers: KeyModifiers) -> DraftKey {
        card.on_key(&KeyEvent::new(code, modifiers))
    }

    /// Writing types into the editor; Ctrl+Enter keeps it; Enter then posts it, once; nothing more
    /// happens until the answer comes.
    #[test]
    fn a_reply_is_written_kept_and_posted() {
        let mut card = DraftCard::writing(draft(""), Some(("jon".into(), "Why twice?".into())));
        for character in "Fixed.".chars() {
            press(&mut card, KeyCode::Char(character), KeyModifiers::NONE);
        }
        assert_eq!(press(&mut card, KeyCode::Enter, KeyModifiers::NONE), DraftKey::Held);
        press(&mut card, KeyCode::Char('x'), KeyModifiers::NONE);
        assert_eq!(press(&mut card, KeyCode::Enter, KeyModifiers::CONTROL), DraftKey::Held);
        assert_eq!(card.draft.body, "Fixed.\nx");
        assert!(card.editor.is_none());
        assert_eq!(press(&mut card, KeyCode::Enter, KeyModifiers::NONE), DraftKey::Post);
        assert!(card.posting);
        assert_eq!(press(&mut card, KeyCode::Enter, KeyModifiers::NONE), DraftKey::Held, "once");
        assert_eq!(card.hint(), "posting…");
    }

    /// Esc on a reply never written puts it away; on one with words it keeps them, first leaving
    /// the editor and then the card.
    #[test]
    fn esc_keeps_what_was_written_and_drops_what_was_not() {
        let mut empty = DraftCard::writing(draft(""), None);
        assert_eq!(press(&mut empty, KeyCode::Esc, KeyModifiers::NONE), DraftKey::Close);

        let mut written = DraftCard::reading(draft("Fixed."), None);
        assert_eq!(press(&mut written, KeyCode::Char('e'), KeyModifiers::NONE), DraftKey::Held);
        assert!(written.editor.is_some(), "e opens the editor on the text");
        assert_eq!(press(&mut written, KeyCode::Esc, KeyModifiers::NONE), DraftKey::Held);
        assert!(written.editor.is_none());
        assert_eq!(press(&mut written, KeyCode::Esc, KeyModifiers::NONE), DraftKey::Keep);
        assert_eq!(
            press(&mut written, KeyCode::Char('E'), KeyModifiers::SHIFT),
            DraftKey::EditExternally
        );
        written.edited("Fixed in 4c1e2a0.\n\n".into());
        assert_eq!(written.draft.body, "Fixed in 4c1e2a0.");
    }

    /// Nothing written is nothing to post.
    #[test]
    fn an_empty_draft_cannot_be_posted() {
        let mut card = DraftCard::reading(draft("   "), None);
        assert_eq!(press(&mut card, KeyCode::Enter, KeyModifiers::NONE), DraftKey::Held);
        assert!(!card.posting);
    }

    /// A draft longer than the card scrolls to its end and back, and posting still sends all of it.
    ///
    /// The break this catches: the body was drawn from row zero with the scrollbar pinned there, so
    /// the end of a long agent-written reply could not be read before Enter posted it.
    #[test]
    fn a_long_draft_scrolls_to_its_end() {
        use ratatui::{backend::TestBackend, Terminal};
        let mut body = String::from("FIRST-MARK\n\n");
        for line in 0..60 {
            body.push_str(&format!("line {line}\n\n"));
            if line == 30 {
                body.push_str("MIDDLE-MARK\n\n");
            }
        }
        body.push_str("LAST-MARK");
        let mut card = DraftCard::reading(draft(&body), None);
        let styles = crate::theme::styles::ThemeStyles::from_resolved(
            &luxe_core::theme::ThemeRegistry::new().resolve("nord").expect("nord"),
            "nord",
        );
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).expect("terminal");
        let mut screen = |card: &DraftCard| {
            terminal.draw(|f| render(f, f.area(), card, 0, &styles)).expect("drawn");
            let buffer = terminal.backend().buffer();
            buffer.content.iter().map(|cell| cell.symbol()).collect::<String>()
        };
        let first = screen(&card);
        assert!(first.contains("FIRST-MARK") && !first.contains("LAST-MARK"));
        press(&mut card, KeyCode::End, KeyModifiers::NONE);
        let end = screen(&card);
        assert!(end.contains("LAST-MARK") && !end.contains("FIRST-MARK"), "End reaches the end");
        press(&mut card, KeyCode::Home, KeyModifiers::NONE);
        let home = screen(&card);
        assert!(home.contains("FIRST-MARK") && !home.contains("LAST-MARK"), "Home comes back");
        for _ in 0..20 {
            press(&mut card, KeyCode::PageDown, KeyModifiers::NONE);
        }
        assert!(screen(&card).contains("LAST-MARK"), "and paging gets there too");

        assert_eq!(press(&mut card, KeyCode::Enter, KeyModifiers::NONE), DraftKey::Post);
        for mark in ["FIRST-MARK", "MIDDLE-MARK", "LAST-MARK"] {
            assert!(card.draft.body.contains(mark), "{mark} is posted");
        }
    }

    /// Up and PageUp come back from the end, never past the top, and a new body is read from its
    /// start however it arrived: drafted, kept from the editor, or back from an outside editor.
    #[test]
    fn reading_comes_back_up_and_a_new_body_starts_at_the_top() {
        let body = (0..80).map(|line| format!("line {line}\n")).collect::<String>();
        let mut card = DraftCard::reading(draft(&body), None);
        card.reading_room.set((60, 20));
        press(&mut card, KeyCode::End, KeyModifiers::NONE);
        press(&mut card, KeyCode::PageUp, KeyModifiers::NONE);
        assert_eq!(card.scroll, 41, "a page is what was shown, less a line of overlap");
        press(&mut card, KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(card.scroll, 40);
        press(&mut card, KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(card.scroll, 41);
        for _ in 0..5 {
            press(&mut card, KeyCode::PageUp, KeyModifiers::NONE);
        }
        assert_eq!(card.scroll, 0, "no further than the top");
        press(&mut card, KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(card.scroll, 0);

        let scrolled = |card: &mut DraftCard| {
            press(card, KeyCode::End, KeyModifiers::NONE);
            assert_eq!(card.scroll, 60);
        };
        scrolled(&mut card);
        card.drafted(Some(body.clone()), Vec::new(), None, None);
        assert_eq!(card.scroll, 0, "a drafted body");
        scrolled(&mut card);
        card.edited(body.clone());
        assert_eq!(card.scroll, 0, "a body back from the outside editor");
        scrolled(&mut card);
        press(&mut card, KeyCode::Char('e'), KeyModifiers::NONE);
        press(&mut card, KeyCode::Enter, KeyModifiers::CONTROL);
        assert_eq!(card.scroll, 0, "a body kept from the editor");
    }
}
