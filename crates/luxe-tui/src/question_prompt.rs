//! Interactive question modal — the picker shown above the composer when the agent asks a question.
//!
//! Mirrors the permission prompt's chrome (rounded panel, composer width, grows up from the input)
//! and the OpenCode `askquestion` UX: an options list with up/down + digit shortcuts, inline `[✓]`
//! checkboxes for multi-select (Space toggles), and a last option for typing your own answer,
//! numbered and checked like the rest so it reads as a choice rather than a hint. Typing anywhere
//! picks it. `Enter` submits, `Esc` cancels. Modal while open — stray keys never leak into the
//! composer.
//!
//! Each option can carry a one-line rationale and a `recommended` marker, so the choice explains
//! itself instead of making the reader infer it from a bare label. The cursor opens on the
//! recommendation, which makes `Enter` the sensible default rather than the first-listed one.

use crate::glyphs::Icon;
use ratatui::text::{Line, Span};

/// Rows one option's rationale may occupy before it is clipped.
const MAX_DESCRIPTION_ROWS: usize = 2;

/// Rows the question itself may occupy — a clipped question is unanswerable.
const MAX_PROMPT_ROWS: usize = 3;

/// Rows a typed answer may occupy before the row starts following the caret.
///
/// It wraps rather than truncating, because an answer you cannot see is one you cannot check — but
/// the growth is bounded twice over: the panel grows upward into the transcript, and a chosen row
/// is one slab, whose cut steps in once per line and so can only carry as many lines as the panel
/// reserved cells for. Past this the window scrolls with the caret instead.
const MAX_CUSTOM_ROWS: usize = MAX_DESCRIPTION_ROWS + 1;

/// The typed-answer option's label until something is typed into it.
const CUSTOM_PLACEHOLDER: &str = "Type your own answer";

/// The cells a panel spends on its border and inner padding, left and right.
const PANEL_FRAME: u16 = 4;

/// Narrowest column count worth wrapping a rationale into. Below this it shreds into fragments
/// that cost more attention than the label alone.
const MIN_DESCRIPTION_WIDTH: usize = 24;

/// Marker on the option the agent would pick, in the house `  word` badge form.
const RECOMMENDED_BADGE: &str = "  recommended";

/// The panel's content, measured against the room it has above the composer.
struct PanelLayout {
    prompt_rows: Vec<String>,
    rows: Vec<LaidOutRow>,
    /// Rows the panel will occupy, chrome included.
    height: u16,
}

/// Cells the panel's content may occupy inside `width`.
fn inner_width(width: u16) -> usize {
    usize::from(width.saturating_sub(PANEL_FRAME))
}

/// Rows the question panel will occupy above the composer.
///
/// The renderer's own measurement, so the rows the transcript stands down from are exactly the
/// rows the panel takes — anything less and the modal covers the output it is asking about.
pub fn height(
    state: &QuestionPromptState,
    theme: &crate::theme::styles::ThemeStyles,
    width: u16,
    available: u16,
) -> u16 {
    if width == 0 {
        return 0;
    }
    plan(state, theme, inner_width(width), available).height
}

/// Lay the panel out for `available` rows, giving up detail in the order a reader can spare it.
///
/// Options come first — an option you cannot see is a modal you cannot answer — so the rationale
/// goes first, then the typed answer's own rows down to the one it cannot do without, and the
/// question itself last (through [`prompt_budget`]). The attempts are a fixed list rather than a
/// loop that decides when to stop, so the worst case is a panel that spends everything it has.
fn plan(
    state: &QuestionPromptState,
    theme: &crate::theme::styles::ThemeStyles,
    inner_width: usize,
    available: u16,
) -> PanelLayout {
    let lay = |with_descriptions, custom_rows| {
        let rows = lay_out_rows(state, theme, inner_width, with_descriptions, custom_rows);
        let prompt_rows = wrapped_rows(&state.prompt, inner_width, prompt_budget(available, &rows));
        let height = desired_height(&prompt_rows, &rows);
        PanelLayout { prompt_rows, rows, height }
    };
    let mut panel = lay(true, MAX_CUSTOM_ROWS);
    for custom_rows in (1..=MAX_CUSTOM_ROWS).rev() {
        if panel.height <= available {
            break;
        }
        panel = lay(false, custom_rows);
    }
    PanelLayout { height: panel.height.min(available), ..panel }
}

/// Render the question modal directly above the input, matching the permission prompt's chrome.
pub fn render_question_prompt(
    f: &mut ratatui::Frame,
    region: ratatui::layout::Rect,
    input_top: u16,
    state: &QuestionPromptState,
    theme: &crate::theme::styles::ThemeStyles,
    hit: &std::cell::RefCell<crate::mouse::HitRegions>,
) {
    use ratatui::{
        layout::Rect,
        style::Modifier,
        widgets::{Clear, Padding, Paragraph},
    };

    let available = input_top.saturating_sub(region.y);
    let inner_width = inner_width(region.width);
    let PanelLayout { prompt_rows, rows, height } = plan(state, theme, inner_width, available);
    if region.width == 0 || height == 0 {
        return;
    }
    let area = Rect::new(region.x, input_top.saturating_sub(height), region.width, height);
    f.render_widget(Clear, area);

    let mut lines: Vec<Line> = Vec::new();
    for row in &prompt_rows {
        lines.push(Line::from(Span::styled(row.clone(), theme.text.add_modifier(Modifier::BOLD))));
    }
    lines.push(Line::from(""));
    for row in &rows {
        lines.extend(row.lines.iter().cloned());
    }
    let hint = if state.multi {
        "enter submit · space toggle · ↑/↓ choose · esc cancel"
    } else {
        "enter answer · ↑/↓ choose · esc cancel"
    };

    let block = crate::widgets::titled_panel(Icon::Question.glyph(), "Question", theme)
        .padding(Padding::new(1, 1, 1, 1));
    let body = crate::widgets::render_footed(f, block, area, hint, None, theme);
    f.render_widget(Paragraph::new(lines), body);

    // Publish each row as a click target over the lines it actually occupies. Above the first:
    // top border, top padding, the wrapped prompt, then a blank.
    let mut regions = hit.borrow_mut();
    let mut y = area.y.saturating_add(2 + prompt_rows.len() as u16 + 1);
    for (i, row) in rows.iter().enumerate() {
        let row_height = row.lines.len() as u16;
        if y < area.y + area.height {
            let visible = row_height.min(area.y + area.height - y);
            regions.push(
                Rect::new(area.x, y, area.width, visible),
                crate::mouse::MouseTarget::QuestionRow(i),
            );
        }
        y = y.saturating_add(row_height);
    }
}

/// Panel height for a laid-out modal: content plus the blanks, borders, and padding around it.
fn desired_height(prompt_rows: &[String], rows: &[LaidOutRow]) -> u16 {
    u16::try_from(chrome_and_rows(rows) + prompt_rows.len()).unwrap_or(u16::MAX)
}

/// Everything the panel owes before the question gets a line: the borders, the top padding, the
/// blank under the prompt, the option rows, and the footer, which takes the bottom padding's row.
fn chrome_and_rows(rows: &[LaidOutRow]) -> usize {
    3 + 1
        + rows.iter().map(|row| row.lines.len()).sum::<usize>()
        + usize::from(crate::widgets::FOOTER_ROWS)
}

/// Rows the question may occupy once the options and chrome are paid for (always at least one).
fn prompt_budget(available: u16, rows: &[LaidOutRow]) -> usize {
    usize::from(available).saturating_sub(chrome_and_rows(rows)).clamp(1, MAX_PROMPT_ROWS)
}

mod rows;
mod state;

use rows::{lay_out_rows, wrapped_rows, LaidOutRow};
pub use state::*;

#[cfg(test)]
mod tests;
