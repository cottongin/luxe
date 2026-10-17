//! Guided Review key routing — which keys the review surface claims, and when.
//!
//! Review is full-screen, so it owns plain letters that would otherwise reach the composer. What
//! it must *not* own is anything typed into one of its own overlays — the help sheet, the source
//! reference field, the search box, the source picker, the comment composer — because those are
//! text input, and a letter bound to a review action would be swallowed instead of typed.
//!
//! This is the *navigation* half of the surface's input. The other half — the finding composer, the
//! findings drawer, the finish dialog — is `super::review`, and both hang off
//! `super::handle_key`. Keys must reach it through `App`, not around it in the event loop, or no
//! `App`-level test and no scripted recording could drive any of it.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, Msg};

/// The review's own overlay that takes typing, when one is open.
///
/// In the order they take it: a search outranks the source reference field, which outranks the
/// picker it was opened from. The help sheet takes no typing, only Esc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LocalOverlay {
    Search,
    SourceReference,
    SourcePicker,
    Help,
    None,
}

impl LocalOverlay {
    fn of(workspace: &crate::guided_review::GuidedReviewWorkspace) -> Self {
        if workspace.search.is_some() {
            LocalOverlay::Search
        } else if workspace.source_reference_open() {
            LocalOverlay::SourceReference
        } else if workspace.source_picker_open() {
            LocalOverlay::SourcePicker
        } else if workspace.help_visible {
            LocalOverlay::Help
        } else {
            LocalOverlay::None
        }
    }
}

/// The message a key produces inside the Guided Review surface, or `None` when the surface does not
/// claim it and the ordinary rungs should have it.
///
/// `None` is the whole reason this is not a `RouteAction`: two arms mean *"not mine"* — a key typed
/// into the finding composer, and a chord the keymap has already bound to a `tui.review.*` action.
pub fn claim(app: &mut App, key: &KeyEvent) -> Option<Msg> {
    let workspace = &app.guided_review_workspace;
    let overlay = LocalOverlay::of(workspace);
    // The composer owns everything, including the chords the review binds to letters.
    // `input::handle_key` already routes discard, severity, field and text; nothing here
    // needs to know which is which.
    if workspace.finding_draft.is_some() {
        return None;
    }
    let action = crate::app::input::resolve_key_event(&app.keymap, *key);
    let failed =
        matches!(workspace.load_state, crate::guided_review::GuidedReviewLoadState::Failed { .. });
    if overlay == LocalOverlay::None && !failed && action.is_some_and(|action| action.is_review()) {
        return None;
    }
    // The palette opens over anything, a review included; everything else unclaimed below is
    // swallowed so it cannot reach the composer hidden behind the surface
    if overlay == LocalOverlay::None && action == Some(luxe_core::keymap::UiAction::OpenPalette) {
        return None;
    }
    if key.code == KeyCode::Esc {
        return Some(escape(workspace));
    }
    let typed = overlay_key(overlay, workspace.source_picker_open(), key);
    Some(typed.unwrap_or_else(|| surface_key(workspace, failed, key)))
}

/// Esc closes what is drawn on top — the help sheet first — and with nothing open leaves the review.
fn escape(workspace: &crate::guided_review::GuidedReviewWorkspace) -> Msg {
    if workspace.help_visible {
        Msg::GuidedReviewToggleHelp
    } else if workspace.source_reference_open() {
        Msg::GuidedReviewCancelSourceReference
    } else if workspace.source_picker_open() {
        Msg::GuidedReviewToggleSourcePicker
    } else if workspace.search.is_some() {
        Msg::GuidedReviewCancelSearch
    } else {
        Msg::ExitGuidedReview
    }
}

/// A key the open overlay takes: while a field is open, letters, spaces and `/` are text.
///
/// A space in the source reference field is text too, though a reference is one token: typed, it
/// is refused at submit with a reason, where passed through it stepped the walk behind the field.
fn overlay_key(overlay: LocalOverlay, picker_open: bool, key: &KeyEvent) -> Option<Msg> {
    let plain = !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
    let msg = match (overlay, key.code) {
        (LocalOverlay::Search, KeyCode::Enter | KeyCode::Down) => Msg::GuidedReviewSearchNext,
        (LocalOverlay::Search, KeyCode::Up) => Msg::GuidedReviewSearchPrevious,
        (LocalOverlay::Search, KeyCode::Backspace) => Msg::GuidedReviewSearchBackspace,
        (LocalOverlay::Search, KeyCode::Char(character)) if plain => {
            Msg::GuidedReviewSearchInput(character)
        }
        (LocalOverlay::SourceReference, KeyCode::Char(character)) if plain => {
            Msg::GuidedReviewSourceReferenceInsert(character)
        }
        (LocalOverlay::SourceReference, KeyCode::Backspace) => {
            Msg::GuidedReviewSourceReferenceBackspace
        }
        (LocalOverlay::SourceReference, KeyCode::Enter) => Msg::GuidedReviewSubmitSourceReference,
        (LocalOverlay::SourcePicker, KeyCode::Enter) => Msg::GuidedReviewSelectSourcePicker,
        // The picker stays open under the reference field, and its list still moves
        (LocalOverlay::SourceReference | LocalOverlay::SourcePicker, KeyCode::Down)
            if picker_open =>
        {
            Msg::GuidedReviewSourcePickerNext
        }
        (LocalOverlay::SourceReference | LocalOverlay::SourcePicker, KeyCode::Up)
            if picker_open =>
        {
            Msg::GuidedReviewSourcePickerPrevious
        }
        _ => return None,
    };
    Some(msg)
}

/// The walkthrough's own keys, and a swallowed tick for anything else.
fn surface_key(
    workspace: &crate::guided_review::GuidedReviewWorkspace,
    failed: bool,
    key: &KeyEvent,
) -> Msg {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let picking = workspace.source_reference_open() || workspace.source_picker_open();
    match key.code {
        KeyCode::Char('/') if !picking => Msg::GuidedReviewBeginSearch,
        // Space advances the walkthrough: the universal "keep reading" key, and with the
        // reviewed checklist gone it has no better job.
        KeyCode::Char(' ') => Msg::GuidedReviewHunkNext,
        KeyCode::Char('?') if !picking => Msg::GuidedReviewToggleHelp,
        // vim's half-page moves; before these arms, so Ctrl+D is not taken for a bare `d`
        KeyCode::Char('d') if ctrl => Msg::GuidedReviewHalfPage(true),
        KeyCode::Char('u') if ctrl => Msg::GuidedReviewHalfPage(false),
        KeyCode::Char('f') => Msg::GuidedReviewToggleNoise,
        KeyCode::Char('z') => Msg::GuidedReviewToggleNarration,
        KeyCode::Char('d') => Msg::GuidedReviewToggleNoiseDetails,
        KeyCode::Char('s') => Msg::GuidedReviewToggleSourcePicker,
        KeyCode::Char('r') if failed => Msg::GuidedReviewRetry,
        // ↑/↓ scroll what is being read and never anything else, whichever pane has focus
        KeyCode::Down if ctrl => Msg::GuidedReviewFileNext,
        KeyCode::Up if ctrl => Msg::GuidedReviewFilePrevious,
        KeyCode::Down => Msg::GuidedReviewLineNext,
        KeyCode::Up => Msg::GuidedReviewLinePrevious,
        KeyCode::PageDown => Msg::GuidedReviewPageNext,
        KeyCode::PageUp => Msg::GuidedReviewPagePrevious,
        KeyCode::Home => Msg::GuidedReviewLineStart,
        KeyCode::Char('b') if ctrl => Msg::GuidedReviewToggleSplit,
        KeyCode::Right => Msg::GuidedReviewColumnNext,
        KeyCode::Left => Msg::GuidedReviewColumnPrevious,
        KeyCode::Char(']') => Msg::GuidedReviewChapterNext,
        KeyCode::Char('[') => Msg::GuidedReviewChapterPrevious,
        KeyCode::Char('n') => Msg::GuidedReviewHunkNext,
        KeyCode::Char('N') => Msg::GuidedReviewHunkPrevious,
        // Guided Review owns plain keys inside this full-screen surface; never leak them into composer.
        _ => Msg::Tick,
    }
}

#[cfg(test)]
mod tests;
