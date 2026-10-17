//! Permission ask prompt — the yellow picker shown above the composer when a tool needs approval.
//!
//! Same chrome as the `/` and `@` flyouts (rounded panel, composer width, grows up from the
//! input) and the same styling: standard border + text colors, an accent cursor, and a
//! selection-highlighted focused row. Only the *action in question* (the summary line, e.g.
//! `Run: …`) is painted in the theme's warning color, so attention lands on what's being approved
//! rather than the whole panel. It captures input while open so the answer is deliberate; `Esc`
//! always rejects. Keyboard-complete (arrows + Enter, and digits by position) and mouse-clickable
//! (rows publish hit regions).
//!
//! # Type-through
//!
//! The card opens under the reader's hands, often mid-sentence in the composer. A letter that
//! answered it — `a`, `s`, `w`, `r` — would be pressed by typing "Now…" as it appeared, writing an
//! allow rule into the global permissions file. So there are no letter accelerators, and for
//! [`GRACE`] after it opens the card swallows everything except a reject.

use crate::glyphs::Icon;
use crate::widgets::truncate_end as truncate;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use luxe_core::permission::{PermissionReply, PermissionRequestId};

/// The answers, in display order: label + one-line hint.
///
/// Built once at first use rather than written down, because the persist hint names a real file on
/// the machine running this and `~/.config/luxe` is right on two platforms out of three.
static OPTIONS: std::sync::LazyLock<[(PermissionReply, &'static str, String); 4]> =
    std::sync::LazyLock::new(|| {
        [
            (PermissionReply::Once, "Allow once", "run just this call".to_string()),
            (
                PermissionReply::Always,
                "Allow for this session",
                "remember matching calls".to_string(),
            ),
            (
                PermissionReply::AlwaysPersist,
                "Allow always",
                format!(
                    "save to {}",
                    luxe_core::user_dirs::config_path_display("permissions.json")
                ),
            ),
            (PermissionReply::Reject, "Reject", "deny this call".to_string()),
        ]
    });

/// How long after opening the card ignores everything but a reject.
///
/// Long enough to outlast a burst of typing that was aimed at the composer, short enough that a
/// reader who saw the card and meant to answer never notices it.
pub const GRACE: std::time::Duration = std::time::Duration::from_millis(400);

/// Rows around the options: the summary line and a blank under it.
const HEADER_LINES: u16 = 2;

/// Rows of chrome around that content: the border, a row of inner padding on top, and the footer,
/// which takes the bottom padding's row.
const CHROME_ROWS: u16 = 3 + crate::widgets::FOOTER_ROWS;

/// Rows the prompt will occupy above the composer.
///
/// The renderer's own measurement, so the rows the transcript stands down from are exactly the
/// rows the panel takes — anything less and the prompt covers the work it is asking about.
pub fn height(state: &PermissionPromptState, available: u16) -> u16 {
    (HEADER_LINES + state.options().len() as u16 + CHROME_ROWS).min(available)
}

/// State for an in-flight permission prompt (one at a time — tools dispatch sequentially).
#[derive(Clone, Debug)]
pub struct PermissionPromptState {
    /// Correlates the reply with the awaiting engine turn.
    pub id: PermissionRequestId,
    /// Tool being invoked (e.g. `bash`).
    pub tool: String,
    /// One-line description of the action (e.g. `Run: rm -rf build`).
    pub summary: String,
    /// Patterns an "always" reply would approve (for the informed-consent hint).
    ///
    /// Empty means there is nothing to remember — a hook's escalation, say — and the two
    /// remembering rows are not offered at all rather than offered and quietly meaning "once".
    pub always_patterns: Vec<String>,
    selected: usize,
    opened_at: std::time::Instant,
}

impl PermissionPromptState {
    pub fn new(
        id: PermissionRequestId,
        tool: String,
        summary: String,
        always_patterns: Vec<String>,
    ) -> Self {
        Self {
            id,
            tool,
            summary,
            always_patterns,
            selected: 0,
            opened_at: std::time::Instant::now(),
        }
    }

    /// The same prompt, opened at `at` rather than now — the app's frame clock, which a test pins.
    pub fn opened(mut self, at: std::time::Instant) -> Self {
        self.opened_at = at;
        self
    }

    /// The options this prompt offers (reply, label, hint), in display order.
    pub fn options(&self) -> Vec<&'static (PermissionReply, &'static str, String)> {
        let remembers = !self.always_patterns.is_empty();
        OPTIONS
            .iter()
            .filter(|(reply, ..)| {
                remembers
                    || !matches!(reply, PermissionReply::Always | PermissionReply::AlwaysPersist)
            })
            .collect()
    }

    /// Index of the highlighted option.
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Move the selection to `index` (a mouse click), clamped to range.
    pub fn select(&mut self, index: usize) {
        self.selected = index.min(self.options().len() - 1);
    }

    fn move_up(&mut self) {
        let count = self.options().len();
        self.selected = if self.selected == 0 { count - 1 } else { self.selected - 1 };
    }

    fn move_down(&mut self) {
        self.selected = (self.selected + 1) % self.options().len();
    }

    /// Route a keypress arriving at `now`. Everything not understood is swallowed (the prompt is
    /// modal), so stray keystrokes never leak into the composer while a decision is pending.
    ///
    /// `now` is the app's frame clock rather than the wall clock, so a test can pin the grace
    /// period instead of sleeping through it.
    pub fn on_key_at(
        &mut self,
        key: &KeyEvent,
        now: std::time::Instant,
    ) -> PermissionPromptOutcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let rejects = key.code == KeyCode::Esc || (ctrl && key.code == KeyCode::Char('c'));
        if rejects {
            return PermissionPromptOutcome::Respond(PermissionReply::Reject);
        }
        if now.saturating_duration_since(self.opened_at) < GRACE {
            return PermissionPromptOutcome::Consumed;
        }
        match key.code {
            KeyCode::Enter => PermissionPromptOutcome::Respond(self.options()[self.selected].0),
            KeyCode::Up => {
                self.move_up();
                PermissionPromptOutcome::Consumed
            }
            KeyCode::Down => {
                self.move_down();
                PermissionPromptOutcome::Consumed
            }
            KeyCode::Char('p') if ctrl => {
                self.move_up();
                PermissionPromptOutcome::Consumed
            }
            KeyCode::Char('n') if ctrl => {
                self.move_down();
                PermissionPromptOutcome::Consumed
            }
            // Digits by displayed position, never letters: a word typed at the composer is made of
            // letters, and one of them would mean "save this approval forever"
            KeyCode::Char(digit @ '1'..='9') => {
                let index = digit as usize - '1' as usize;
                match self.options().get(index) {
                    Some((reply, ..)) => PermissionPromptOutcome::Respond(*reply),
                    None => PermissionPromptOutcome::Consumed,
                }
            }
            _ => PermissionPromptOutcome::Consumed,
        }
    }
}

/// Outcome of a keypress on the permission prompt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PermissionPromptOutcome {
    /// Answer the prompt with this reply and close it.
    Respond(PermissionReply),
    /// Key consumed; the prompt stays open.
    Consumed,
}

/// Render the prompt directly above the input, styled in the theme's warning (yellow) color.
pub fn render_permission_prompt(
    f: &mut ratatui::Frame,
    region: ratatui::layout::Rect,
    input_top: u16,
    state: &PermissionPromptState,
    theme: &crate::theme::styles::ThemeStyles,
    hit: &std::cell::RefCell<crate::mouse::HitRegions>,
) {
    use ratatui::{
        layout::Rect,
        style::Modifier,
        text::{Line, Span},
        widgets::{Clear, Padding, Paragraph},
    };

    let available = input_top.saturating_sub(region.y);
    let height = height(state, available);
    let options = state.options();
    if region.width == 0 || height == 0 {
        return;
    }
    let area = Rect::new(region.x, input_top.saturating_sub(height), region.width, height);
    f.render_widget(Clear, area);

    // Border (2) + one column of inner padding each side (2).
    let inner_width = area.width.saturating_sub(4) as usize;
    let mut lines: Vec<Line> = Vec::with_capacity(HEADER_LINES as usize + options.len());
    lines.push(Line::from(Span::styled(
        truncate(&state.summary, inner_width.saturating_sub(1)),
        theme.warning.add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));
    for (i, (reply, label, hint)) in options.iter().enumerate() {
        let selected = i == state.selected;
        let cursor = crate::glyphs::Mark::Cursor.gutter(selected);
        // Standard picker styling (matches the `/` and `@` flyouts): accent cursor, bold label on
        // the focused row, muted hint. The warning color is reserved for the summary above.
        let remembers = matches!(reply, PermissionReply::Always | PermissionReply::AlwaysPersist);
        let hint_text = if remembers && !state.always_patterns.is_empty() {
            let scope = if *reply == PermissionReply::AlwaysPersist { "save" } else { "remember" };
            format!("  {scope}: {}", state.always_patterns.join(", "))
        } else {
            format!("  {hint}")
        };
        let hint_style = if selected { theme.muted.patch(theme.selected_bg) } else { theme.muted };
        let room = inner_width.saturating_sub(label.len() + 2 + crate::widgets::SELECTION_FRAME);
        lines.push(crate::widgets::selection_row(
            vec![
                Span::styled(cursor, theme.accent_on(selected)),
                Span::styled(*label, theme.label_on(selected)),
                Span::styled(truncate(&hint_text, room), hint_style),
            ],
            inner_width,
            selected,
            theme,
        ));
    }

    let block = crate::widgets::titled_panel(
        Icon::Permission.glyph(),
        "Permission required",
        theme,
    )
    // One row/column of breathing room so text isn't flush against the border on any side.
    .padding(Padding::new(1, 1, 1, 1));
    let digits = format!("1–{} pick", options.len());
    let hint = format!("enter answer · {digits} · ↑/↓ choose · esc deny");
    let body = crate::widgets::render_footed(f, block, area, &hint, None, theme);
    f.render_widget(Paragraph::new(lines), body);

    // Publish the option rows as click targets. Rows above the first option: top border, top
    // padding, summary, blank — so options start four rows in.
    let mut regions = hit.borrow_mut();
    let first_option_y = area.y.saturating_add(4);
    for i in 0..options.len() {
        let y = first_option_y.saturating_add(i as u16);
        if y < area.y + area.height {
            regions.push(
                Rect::new(area.x, y, area.width, 1),
                crate::mouse::MouseTarget::PermissionRow(i),
            );
        }
    }
}

/// Truncate to `max` display columns, adding an ellipsis when shortened.
#[cfg(test)]
mod tests;
