//! What a click or a wheel notch means — the target under the pointer resolved to an action.

use super::{RouteAction, ScrollAction};
use crate::app::{App, Msg};
use crate::mouse::MouseTarget;
use crossterm::event::KeyEvent;

/// A synthetic Enter, used to replay an overlay's existing "accept selected row" path after a
/// click has moved its selection to the clicked row.
pub(super) fn synthetic_enter() -> RouteAction {
    use crossterm::event::{KeyCode, KeyModifiers};
    RouteAction::msg(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)))
}

/// Routes a left-click to the clickable target under the cursor.
pub(super) fn click(app: &mut App, col: u16, row: u16) -> RouteAction {
    let target = app.hit.borrow().at(col, row).cloned();
    // The scope switch is a menu, and a menu opens where it was asked for
    if target == Some(crate::mouse::MouseTarget::DockFilesScope)
        && app.current_view != crate::app::ViewMode::GuidedReview
    {
        app.overlays.context_menu = Some(crate::context_menu::ContextMenu::for_scope(
            (col, row),
            app.files.changed_only,
            app.files.show_ignored,
        ));
        return RouteAction::None;
    }
    match target {
        Some(target) => clicked_target(app, target),
        None => RouteAction::None,
    }
}

/// Maps one resolved click target to the action it stands for.
pub(super) fn clicked_target(app: &mut App, target: MouseTarget) -> RouteAction {
    // Overlay list rows: move the selection to the clicked row, then reuse the overlay's
    // existing Enter/accept path so click and keyboard behave identically.
    if let Some(open) = select_overlay_row(app, &target) {
        return if open { synthetic_enter() } else { RouteAction::None };
    }
    match target {
        // A dashboard menu row activates only while the dashboard itself has the input, so a
        // click never leaks through a surface drawn on top of it.
        MouseTarget::DashboardItem(index) if app.dashboard_owns_input() => {
            dashboard_row(app, index)
        }
        // Same rule for the Configuration menu: a click moves the cursor and then runs the row it
        // moved to, so pointer and keyboard reach identical state.
        MouseTarget::ConfigRow(index)
            if app.current_view == crate::app::ViewMode::Configuration
                && app.overlays.auth_wizard.is_none() =>
        {
            configuration_row(app, index)
        }
        MouseTarget::ConfigPage(index)
            if app.current_view == crate::app::ViewMode::Configuration =>
        {
            configuration_page(app, index)
        }
        MouseTarget::ReviewFindingRow(finding_id) => {
            RouteAction::msg(Msg::GuidedReviewSelectFinding(finding_id))
        }
        // Clicking a level the model can't honor is a no-op rather than a silent clamp.
        MouseTarget::ThinkingRow(index)
            if app.overlays.thinking_picker.as_mut().is_some_and(|p| p.select(index)) =>
        {
            synthetic_enter()
        }
        MouseTarget::ThemeRow(index) => theme_row(app, index),
        MouseTarget::AuthCopyCode => copy_device_code(app),
        // Session/project pickers are route_key-owned (dashboard flow), so a click resolves
        // straight to the selection RouteAction rather than a synthetic key.
        MouseTarget::SessionRow(index)
            if app.current_view == crate::app::ViewMode::SessionPicker =>
        {
            session_row(app, index)
        }
        MouseTarget::ReviewDiffRow(row)
            if app.current_view == crate::app::ViewMode::GuidedReview =>
        {
            RouteAction::msg(Msg::GuidedReviewSelectDiffRow(row))
        }
        MouseTarget::ReviewFoldedRun(file_id)
            if app.current_view == crate::app::ViewMode::GuidedReview =>
        {
            RouteAction::msg(Msg::GuidedReviewToggleFileFold(file_id))
        }
        MouseTarget::ReviewGuideFile(file_id)
            if app.current_view == crate::app::ViewMode::GuidedReview =>
        {
            RouteAction::msg(Msg::GuidedReviewSelectFile(file_id))
        }
        MouseTarget::ReviewGuideChapter(chapter_id)
            if app.current_view == crate::app::ViewMode::GuidedReview =>
        {
            RouteAction::msg(Msg::GuidedReviewOpenChapter(chapter_id))
        }
        // The other half of the same hint row: a glance rather than a review.
        MouseTarget::DockPeekChanges if app.current_view != crate::app::ViewMode::GuidedReview => {
            RouteAction::msg(Msg::DispatchUiAction(luxe_core::keymap::UiAction::ToggleChanges))
        }
        // The dock is drawn in Chat's wide layout; the affordance opens review from there.
        MouseTarget::DockReviewChanges
            if app.current_view != crate::app::ViewMode::GuidedReview =>
        {
            RouteAction::msg(Msg::DispatchUiAction(luxe_core::keymap::UiAction::ReviewOpen))
        }
        // A file row under ELSEWHERE: show that file's diff. Clicking the one already open closes it,
        // so the same gesture that opened the pane puts it away.
        MouseTarget::DockChangeFile(path)
            if app.current_view != crate::app::ViewMode::GuidedReview =>
        {
            toggle_file(app, &path, Msg::OpenChanges)
        }
        // A place under ELSEWHERE: fold what has been dealt with, unfold what has not.
        MouseTarget::DockExternalPlace(place)
            if app.current_view != crate::app::ViewMode::GuidedReview =>
        {
            RouteAction::msg(Msg::ToggleExternalPlace(place.clone()))
        }
        // The start screen has no preview to open the changes in, so the bar's changes open the
        // review the list's own row would
        MouseTarget::StatuslineChanges if app.dashboard_owns_input() => {
            RouteAction::Dashboard(crate::dashboard::DashOutcome::ReviewChanges)
        }
        MouseTarget::StatuslineChanges
            if app.current_view != crate::app::ViewMode::GuidedReview =>
        {
            RouteAction::msg(Msg::DispatchUiAction(luxe_core::keymap::UiAction::ToggleChanges))
        }
        MouseTarget::ChangesFooter(action) if app.changes.is_some() => {
            changes_footer_message(action).map_or(RouteAction::None, RouteAction::msg)
        }
        // A file in the tree: show it, or put it away when it is the one showing
        MouseTarget::DockFile(path) if app.current_view != crate::app::ViewMode::GuidedReview => {
            toggle_file(app, &path, Msg::OpenProjectFile)
        }
        MouseTarget::DockFolder(path) if app.current_view != crate::app::ViewMode::GuidedReview => {
            RouteAction::msg(Msg::ToggleFolder(path.clone()))
        }
        MouseTarget::DockFilesScope if app.current_view != crate::app::ViewMode::GuidedReview => {
            RouteAction::msg(Msg::ToggleFilesScope)
        }
        // Every place the mode is shown switches it, the start screen's composer included
        MouseTarget::ComposerMode | MouseTarget::DockModeRow
            if app.current_view.shows_composer() =>
        {
            RouteAction::msg(Msg::DispatchUiAction(luxe_core::keymap::UiAction::CycleMode))
        }
        MouseTarget::DockFocusFiles if app.current_view != crate::app::ViewMode::GuidedReview => {
            RouteAction::msg(Msg::FocusFiles)
        }
        // A press on a line of the File view starts picking lines; the drag that follows extends it
        MouseTarget::PeekLine(line) if app.changes.is_some() => {
            app.pointer.peek_dragging = true;
            RouteAction::msg(Msg::StartPeekPick(line))
        }
        MouseTarget::PeekSelection(action) if app.changes.is_some() => {
            RouteAction::msg(Msg::PeekSelectionAction(action))
        }
        MouseTarget::DockModelRow if app.current_view != crate::app::ViewMode::GuidedReview => {
            RouteAction::msg(Msg::OpenModelPicker)
        }
        // A click on a message opens whatever it is holding back — a card's output, a collapsed run
        // of calls, a previewed paste, a reasoning block — and does nothing at all on plain prose,
        // which is there to be read and selected rather than clicked.
        MouseTarget::ConversationBlock(index) if app.current_view == crate::app::ViewMode::Chat => {
            RouteAction::msg(Msg::ToggleConversationBlockDetails(index))
        }
        MouseTarget::CodeBlockCopy(code) if app.current_view == crate::app::ViewMode::Chat => {
            RouteAction::msg(Msg::CopyText(code))
        }
        MouseTarget::OpenImage(key) if app.current_view == crate::app::ViewMode::Chat => {
            RouteAction::msg(Msg::OpenImageExternally(key))
        }
        MouseTarget::ReviewFooter(action)
            if app.current_view == crate::app::ViewMode::GuidedReview =>
        {
            RouteAction::msg(review_footer_message(action))
        }
        _ => RouteAction::None,
    }
}

/// A dashboard row's outcome, if the row is still there.
fn dashboard_row(app: &App, index: usize) -> RouteAction {
    match app.dashboard.as_ref().and_then(|dashboard| dashboard.outcome_at(index)) {
        Some(outcome) => RouteAction::Dashboard(outcome),
        None => RouteAction::None,
    }
}

/// Moves the Configuration cursor to a row and runs it.
fn configuration_row(app: &mut App, index: usize) -> RouteAction {
    match app.overlays.configuration.as_mut() {
        Some(surface) => {
            surface.select(index);
            RouteAction::Configuration(surface.activate())
        }
        None => RouteAction::None,
    }
}

/// Switches the settings page a tab names.
fn configuration_page(app: &mut App, index: usize) -> RouteAction {
    match app.overlays.configuration.as_mut().and_then(|surface| surface.settings_mut()) {
        Some(settings) => {
            settings.select_page(index);
            RouteAction::Configuration(crate::configuration::ConfigOutcome::Consumed)
        }
        None => RouteAction::None,
    }
}

/// Previews the clicked theme, then accepts it the way Enter would.
/// Copies the sign-in wizard's device code, when it is showing one.
fn copy_device_code(app: &mut App) -> RouteAction {
    let code = app.overlays.auth_wizard.as_ref().and_then(|w| w.device_code().map(String::from));
    if let Some(code) = code {
        app.record_copy(code);
    }
    RouteAction::None
}

/// Opens the session on the clicked picker row.
fn session_row(app: &App, index: usize) -> RouteAction {
    match app.overlays.session_picker.as_ref().and_then(|p| p.session_id_at(index)) {
        Some(id) => RouteAction::SessionSelected(id),
        None => RouteAction::None,
    }
}

fn theme_row(app: &mut App, index: usize) -> RouteAction {
    if let Some(name) = app.overlays.theme_picker.as_mut().and_then(|p| p.select(index)) {
        app.preview_theme(&name);
    }
    synthetic_enter()
}

/// Shows a file, or puts it away when it is the one already showing.
///
/// Args: `(app, path, open)`, where `open` builds the message that shows it.
fn toggle_file(app: &App, path: &str, open: fn(String) -> Msg) -> RouteAction {
    let showing = app.changes.as_ref().is_some_and(|view| view.path == path);
    RouteAction::msg(if showing { Msg::CloseChanges } else { open(path.to_owned()) })
}

/// What a click on the peek's footer does; the hint is not a button.
fn changes_footer_message(action: crate::mouse::ChangesFooterAction) -> Option<Msg> {
    use crate::mouse::ChangesFooterAction as A;
    Some(match action {
        A::FilePrev => Msg::ChangesStepFile(false),
        A::FileNext => Msg::ChangesStepFile(true),
        A::ToggleWordWrap => Msg::ToggleChangesWordWrap,
        A::ToggleSplit => Msg::ToggleChangesSplit,
        A::OpenReview => Msg::ChangesOpenReview,
        A::OpenInEditor => Msg::DispatchUiAction(luxe_core::keymap::UiAction::ChangesOpenInEditor),
        A::Close => Msg::CloseChanges,
        A::ToggleView => Msg::TogglePeekView,
        A::Focus => Msg::FocusPeek,
        A::Hint => return None,
    })
}

/// What a click on the review's footer does.
fn review_footer_message(action: crate::mouse::ReviewFooterAction) -> Msg {
    use crate::mouse::ReviewFooterAction as A;
    match action {
        A::HunkNext => Msg::GuidedReviewHunkNext,
        A::ToggleNarration => Msg::GuidedReviewToggleNarration,
        A::ToggleNoise => Msg::GuidedReviewToggleNoise,
        A::ToggleNoiseDetails => Msg::GuidedReviewToggleNoiseDetails,
        A::ToggleSplit => Msg::GuidedReviewToggleSplit,
        A::ToggleWordWrap => Msg::GuidedReviewToggleWordWrap,
        A::ToggleGuide => Msg::GuidedReviewToggleGuide,
        A::ChapterNext => Msg::GuidedReviewChapterNext,
        A::ChapterPrev => Msg::GuidedReviewChapterPrevious,
        A::AskQuestion => Msg::GuidedReviewBeginQuestion,
        A::AddComment => Msg::GuidedReviewBeginFinding,
        A::Feedback => Msg::GuidedReviewToggleFindings,
        A::Finish => Msg::DispatchUiAction(luxe_core::keymap::UiAction::ReviewFinish),
        A::ToggleSource => Msg::GuidedReviewToggleSourcePicker,
        A::ToggleHelp => Msg::GuidedReviewToggleHelp,
        A::Exit => Msg::ExitGuidedReview,
    }
}

/// Routes a wheel notch to the scrollable region under the cursor, falling back to the current
/// view's primary surface when the cursor is over unregistered space.
pub(super) fn wheel(app: &App, col: u16, row: u16, up: bool) -> RouteAction {
    use crate::mouse::ScrollRegionId;
    match app.hit.borrow().scroll_at(col, row) {
        Some(ScrollRegionId::Conversation) => conversation_wheel(app, up),
        Some(ScrollRegionId::ReviewDiff) => RouteAction::msg(if up {
            Msg::GuidedReviewLinePrevious
        } else {
            Msg::GuidedReviewLineNext
        }),
        // Scrolls the prose rather than moving the cursor, so a reader can read the section they
        // are *not* standing in without every notch dragging the review's position along
        Some(ScrollRegionId::ReviewGuide) => RouteAction::msg(Msg::GuidedReviewScrollGuide { up }),
        Some(ScrollRegionId::Dock) => RouteAction::msg(Msg::ScrollDock { up }),
        Some(ScrollRegionId::DockList(list)) => RouteAction::msg(Msg::ScrollDockList { list, up }),
        Some(ScrollRegionId::Changes) => {
            RouteAction::msg(Msg::ScrollChanges { up, lines: crate::changes::SCROLL_STEP })
        }
        Some(ScrollRegionId::ContextFiles) => RouteAction::msg(Msg::ScrollContextFiles { up }),
        Some(ScrollRegionId::OverlayList(list)) => {
            RouteAction::msg(Msg::ScrollOverlayList { list, up })
        }
        // Their bars steer them, but their wheel is still the view's
        Some(ScrollRegionId::ReviewFeedback | ScrollRegionId::PullRequestBody) | None => {
            scroll_for_view(app, up)
        }
    }
}

/// Fallback when the cursor isn't over a registered scroll region: scroll the current view's
/// primary surface.
pub(super) fn scroll_for_view(app: &App, up: bool) -> RouteAction {
    match app.current_view {
        crate::app::ViewMode::Chat if app.changes.is_some() => {
            RouteAction::msg(Msg::ScrollChanges { up, lines: crate::changes::SCROLL_STEP })
        }
        crate::app::ViewMode::Chat => conversation_wheel(app, up),
        crate::app::ViewMode::GuidedReview => RouteAction::msg(if up {
            Msg::GuidedReviewLinePrevious
        } else {
            Msg::GuidedReviewLineNext
        }),
        _ => RouteAction::None,
    }
}

/// One wheel notch over the transcript, worth `conversation.scroll_lines` (3 by default).
pub(super) fn conversation_wheel(app: &App, up: bool) -> RouteAction {
    let lines = app.config.conversation.scroll_lines();
    RouteAction::Scroll(if up { ScrollAction::Up(lines) } else { ScrollAction::Down(lines) })
}

/// Moves an open overlay list's selection to the row `target` names: the context files, the
/// palette, the `/` and `@` flyouts, the permission and question prompts, the model picker and the
/// login wizard's two lists.
///
/// `None` when `target` is none of those rows; otherwise whether its overlay is open. Hover and
/// click both go through it, so they always agree on which rows a pointer can move.
pub(super) fn select_overlay_row(app: &mut App, target: &MouseTarget) -> Option<bool> {
    let open = match *target {
        MouseTarget::ContextFileRow(index) => {
            app.overlays.context_files.as_mut().map(|files| files.select(index)).is_some()
        }
        MouseTarget::PaletteRow(index) => {
            app.overlays.palette.as_mut().map(|palette| palette.select(index)).is_some()
        }
        MouseTarget::SlashRow(index) => {
            app.overlays.slash_flyout.as_mut().map(|flyout| flyout.select(index)).is_some()
        }
        MouseTarget::PermissionRow(index) => {
            app.overlays.permission.as_mut().map(|prompt| prompt.select(index)).is_some()
        }
        MouseTarget::QuestionRow(index) => {
            app.overlays.question.as_mut().map(|prompt| prompt.select(index)).is_some()
        }
        MouseTarget::MentionRow(index) => {
            app.overlays.mention.as_mut().map(|mention| mention.select(index)).is_some()
        }
        MouseTarget::ModelRow(index) => {
            app.overlays.model_picker.as_mut().map(|picker| picker.select(index)).is_some()
        }
        MouseTarget::AuthProviderRow(index) => {
            app.overlays.auth_wizard.as_mut().map(|wizard| wizard.select_provider(index)).is_some()
        }
        MouseTarget::AuthMethodRow(index) => {
            app.overlays.auth_wizard.as_mut().map(|wizard| wizard.select_method(index)).is_some()
        }
        _ => return None,
    };
    Some(open)
}
