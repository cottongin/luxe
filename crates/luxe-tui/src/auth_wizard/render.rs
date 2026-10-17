//! Login-wizard rendering — the centered panel for `/login` and the Configuration surface.
//!
//! Same chrome as the other pickers (rounded titled panel, accent cursor, muted hint row) so the
//! wizard reads as part of the same product. Four stage layouts share one frame: provider list,
//! method list, running transcript, and the failure. Success is not drawn here — it closes the
//! wizard and opens the model picker.

mod lists;
mod running;

use crate::glyphs::Icon;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::Modifier,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Padding, Paragraph, Wrap},
    Frame,
};
use unicode_width::UnicodeWidthStr;

use luxe_core::auth::{AuthPromptKind, CredentialStatus, LoginScope};

use super::{AuthWizardState, Stage, WizardLine};
use crate::mouse::{HitRegions, MouseTarget};
use crate::theme::styles::ThemeStyles;

/// Widest the modal may get, so it stays a dialog rather than a page.
const MAX_WIDTH: u16 = 76;

/// Narrowest usable modal; below this the terminal is too small to draw one.
const MIN_WIDTH: u16 = 34;

/// Rows of transcript kept on screen.
const MAX_LINES: usize = 10;

/// Rows a method occupies: what it is, and what it does to you.
const METHOD_ROWS: u16 = 2;

impl AuthWizardState {
    /// Draw the wizard as a modal, centered in `area` with a margin around it.
    ///
    /// What `/login` gets: the conversation is still there, and the panel has to read as something
    /// laid over it.
    pub fn view(
        &self,
        frame: &mut Frame,
        area: Rect,
        theme: &ThemeStyles,
        hit: &std::cell::RefCell<HitRegions>,
    ) {
        if area.width < MIN_WIDTH || area.height < 10 {
            return;
        }
        let width = MAX_WIDTH.min(area.width.saturating_sub(4)).max(MIN_WIDTH);
        let height = self.desired_height().min(area.height.saturating_sub(2));
        self.view_in(frame, crate::widgets::centered_rect(width, height, area), theme, hit);
    }

    /// Draw the wizard's panel at exactly `panel`.
    ///
    /// The Configuration surface has already measured where the panel goes — under the logotype,
    /// where its own menu was — so it places the panel rather than handing over a region to be
    /// centered in, which would leave the two surfaces disagreeing by a row.
    pub(crate) fn view_in(
        &self,
        frame: &mut Frame,
        panel: Rect,
        theme: &ThemeStyles,
        hit: &std::cell::RefCell<HitRegions>,
    ) {
        frame.render_widget(Clear, panel);

        // One column of breathing room each side, so no row is flush against the border.
        let block = crate::widgets::titled_panel(Icon::Key.glyph(), self.title(), theme)
            .padding(Padding::horizontal(1));
        let inner = crate::widgets::render_footed(frame, block, panel, self.hint(), None, theme);

        match &self.stage {
            Stage::Provider { query, .. } => self.render_providers(frame, inner, query, theme, hit),
            Stage::Method { .. } => self.render_methods(frame, inner, theme, hit),
            Stage::Running { provider, lines } => {
                self.render_running(frame, inner, provider, lines, theme)
            }
            Stage::Failed(message) => self.render_failed(frame, inner, message, theme),
        }
    }

    /// The keys the stage answers to, for the footer.
    fn hint(&self) -> &'static str {
        match &self.stage {
            Stage::Provider { .. } => "enter open · ↑/↓ choose · esc close",
            Stage::Method { .. } => "enter start · ↑/↓ choose · esc back",
            Stage::Running { .. } => match self.pending_prompt().map(|(prompt, _)| &prompt.kind) {
                Some(AuthPromptKind::Select(_)) => "enter confirm · ↑/↓ choose · esc cancel",
                Some(_) => "enter confirm · esc cancel",
                None => "Working… · esc cancel",
            },
            Stage::Failed(_) => "r try again · esc close",
        }
    }

    /// Panel title — the scope names what this list is, since the reader chose it by method.
    fn title(&self) -> &'static str {
        match (&self.stage, self.scope()) {
            (Stage::Failed(_), _) => "Sign-in failed",
            (_, LoginScope::Subscription) => "Sign in to a subscription",
            (_, LoginScope::ApiKey) => "Add an API key",
            (_, LoginScope::LocalOrCustom) => "Local or custom model",
            (_, LoginScope::Any) => "Sign in to a provider",
        }
    }

    /// The line above the list: what this set of providers has in common.
    ///
    /// Absent for the whole catalog, which needs no explaining to someone who typed `/login`.
    fn framing(&self) -> Option<&'static str> {
        match self.scope() {
            LoginScope::Any => None,
            LoginScope::Subscription => {
                Some("Signs in to a plan you already pay for. Luxe never sees your password.")
            }
            LoginScope::ApiKey => {
                Some("Paste a key from the provider's console. Stored 0600 in auth.json.")
            }
            LoginScope::LocalOrCustom => {
                Some("A local server needs no key; anything else asks for its base URL.")
            }
        }
    }

    /// Rows the panel needs, counted from what each stage actually draws.
    ///
    /// Guessing high leaves dead space; guessing low silently drops the last row of a list, which is
    /// how a provider goes missing. The Configuration surface reads it too, to decide how much room
    /// is left for the logotype above the panel.
    pub(crate) fn desired_height(&self) -> u16 {
        // Borders (2) + the search row and its rule (2) + the footer (2).
        const PROVIDER_CHROME: u16 = 6;
        match &self.stage {
            Stage::Provider { filtered, .. } => {
                let framing = if self.framing().is_some() { 2 } else { 0 };
                (filtered.len().min(12) as u16) + PROVIDER_CHROME + framing
            }
            Stage::Method { provider, .. } => {
                let option = self.providers().get(*provider);
                let methods = option.map(|o| o.methods.len()).unwrap_or(0) as u16;
                let note = if option.is_some_and(|o| o.note.is_some()) { 3 } else { 0 };
                // Borders (2) + the provider name and a blank line (2) + two rows per method +
                // the footer (2).
                methods * 2 + note + 6
            }
            Stage::Running { lines, .. } => {
                // Two rows per transcript line, because a URL or an explanation usually wraps.
                // The transcript pane never shrinks below its own `Min(2)` constraint, so counting
                // less than that here silently steals a row from the prompt.
                let transcript = (lines.len().min(MAX_LINES) as u16 * 2).max(2);
                // Borders (2) + the "signing in to …" header (1) + the footer (2).
                5 + transcript + self.prompt_height()
            }
            // Borders (2) + heading, blank (2) + the message + the footer (2).
            Stage::Failed(message) => 6 + wrapped_rows(message),
        }
    }
}

/// Rows a line of prose takes at the panel's usable width.
///
/// The height is decided before the modal is laid out, so this assumes the widest case (the modal is
/// capped at [`MAX_WIDTH`]); over-reserving costs a blank row, under-reserving clips content.
fn wrapped_rows(text: &str) -> u16 {
    const USABLE: usize = MAX_WIDTH as usize - 4;
    ((UnicodeWidthStr::width(text) / USABLE) + 1) as u16
}

/// Tuck the terminal cursor into the panel's bottom-right corner.
///
/// Something has to claim it every frame — the composer sets it before the overlay draws, so leaving
/// it alone leaves a cursor blinking outside the dialog the user is reading. The corner of the hint
/// row is the least distracting place to keep it when no field has focus.
fn park_cursor(frame: &mut Frame, inner: Rect) {
    // One row below the content is the panel's bottom border: a cursor sitting on a box-drawing
    // glyph reads as part of the frame instead of a stray block on an empty line.
    frame.set_cursor_position((inner.right(), inner.bottom()));
}

/// Rows a prompt's choice occupies: its label, and the line under it when the flow wrote one.
///
/// One owner, because the panel is measured before it is drawn and a choice that draws taller than
/// it was counted pushes the hint row out through the bottom border.
fn choice_rows(choice: &luxe_core::auth::AuthChoice) -> u16 {
    1 + u16::from(choice.detail.is_some())
}

/// The badge shown beside a provider that's already usable.
fn status_badge(status: &CredentialStatus) -> Option<&'static str> {
    match status {
        CredentialStatus::Missing => None,
        CredentialStatus::Env(_) => Some("env"),
        CredentialStatus::Config(_) => Some("config"),
        CredentialStatus::Local(_) => Some("connected"),
        CredentialStatus::Stored(_) => Some("configured"),
    }
}

#[cfg(test)]
mod tests;
