//! Command dispatch — palette and slash commands into TUI state.
//!
//! Input routing remains in [`super::App`]; this module owns the command outcomes so
//! command behavior stays separate from key precedence.

use super::conversation::first_code_block;
use super::{App, ConvoBlock};
#[cfg(test)]
use luxe_core::config::Config;
use luxe_core::engine::Action;
use luxe_core::event::{GuidedReviewFailure, GuidedReviewFailureKind};
use luxe_core::guided_review::ReviewTarget;
use luxe_core::keymap::UiAction;
use luxe_core::ReviewLoadRequestId;

mod cancel;
pub(in crate::app) mod editor;
mod github;
mod menu;
mod review;
mod slash;

pub(in crate::app) use github::{
    act_on_pull_request, composing_detail, confirm_change, draft_reply, edit_draft_externally,
    open_pull_request_card, post_comment, read_conversation, read_overview,
    refresh_pull_request_card, reread_overview,
};
pub(super) use menu::{command_context, command_menu};
use review::dispatch_review_navigation_action;
pub(in crate::app) use review::{
    flush_queued_review, open_or_start_guided_review, parse_review_target, refresh_guided_review,
    start_guided_review, toggle_changes, worktree_removal_checked,
};
#[cfg(test)]
use review::{LAST_TURN_UNAVAILABLE, REVIEW_TARGET_USAGE};
pub(crate) use slash::reload_config;
use slash::run_slash;

/// Runs a command from the palette, the slash flyout, or typed at the start of the composer.
///
/// The one door every command comes through, so all three behave the same. A command that cannot
/// run here says what it needs instead of closing on nothing; one whose action refuses says so too.
/// Silence would leave *Next review hunk* from a fresh session closing the palette to no effect.
///
/// Clearing the composer is the *caller's* job: the typed and flyout paths strip the `/cmd` text
/// first; the palette leaves the draft alone, which is what lets it run mid-prompt without eating
/// a half-written message.
pub(super) fn dispatch(app: &mut App, command: crate::commands::Command) {
    let context = command_context(app);
    if let Some(need) = command.unmet(&context) {
        app.status.flash_toast(format!("{} needs {}", command.name, need.describe()));
        return;
    }
    let ran = match command.kind {
        crate::commands::CommandKind::Action(action) => dispatch_action(app, action),
        crate::commands::CommandKind::Slash => run_slash(app, &command),
    };
    match ran {
        true => app.recent_commands.record(&command.id),
        false => app.status.flash_toast(format!("{} did nothing here", command.name)),
    }
}

/// Opens the key card for what is on screen: the review's in a review, the chat's otherwise.
fn toggle_keys(app: &mut App) {
    if app.current_view == super::ViewMode::GuidedReview {
        app.update(super::Msg::GuidedReviewToggleHelp);
    } else {
        app.overlays.keys_card = !app.overlays.keys_card;
    }
}

/// Moves the session to the mode after the one it is in.
///
/// The engine answers with `StateChanged`, which is what retitles the composer: drawing the new mode
/// here as well would show a switch that had not happened yet.
pub(super) fn cycle_mode(app: &mut App) {
    let next = luxe_core::mode::Modes::default().after(&app.status.mode).name.clone();
    app.send_action(Action::SetMode(next), "change the mode");
}

/// Applies a resolved UI action and reports whether it was handled.
pub(super) fn dispatch_action(app: &mut App, action: UiAction) -> bool {
    match action {
        UiAction::Quit => {
            app.should_quit = true;
            app.editor.cancel();
            true
        }
        UiAction::NewLine => {
            app.editor.insert_newline();
            true
        }
        UiAction::Submit => {
            app.submit_prompt();
            true
        }
        UiAction::Reload => {
            let repo_root = std::env::current_dir().unwrap_or_default();
            reload_config(app, &repo_root);
            true
        }
        UiAction::OpenPalette => {
            app.overlays.palette = Some(crate::palette::PaletteState::open(command_menu(app)));
            true
        }
        UiAction::ToggleHelp => {
            toggle_keys(app);
            true
        }
        UiAction::ToggleDock => {
            app.dock.toggle(app.workspace_layout);
            true
        }
        UiAction::ShowDock => {
            app.dock.show();
            true
        }
        UiAction::HideDock => {
            app.dock.hide();
            true
        }
        UiAction::ResetDockAuto => {
            app.dock.reset_auto();
            true
        }
        UiAction::ToggleDockPosition => {
            app.dock.toggle_position();
            true
        }
        UiAction::ToggleChanges if app.current_view != super::ViewMode::GuidedReview => {
            toggle_changes(app);
            true
        }
        UiAction::ChangesFileNext if app.changes.is_some() => {
            app.update(super::Msg::ChangesStepFile(true));
            true
        }
        UiAction::ChangesFilePrev if app.changes.is_some() => {
            app.update(super::Msg::ChangesStepFile(false));
            true
        }
        UiAction::ChangesToggleWordWrap if app.changes.is_some() => {
            app.update(super::Msg::ToggleChangesWordWrap);
            true
        }
        UiAction::ChangesToggleView if app.changes.as_ref().is_some_and(|view| view.changed()) => {
            app.update(super::Msg::TogglePeekView);
            true
        }
        UiAction::AskAboutSelection if app.changes.is_some() => {
            match app.changes.as_ref().is_some_and(|view| view.selection.is_some()) {
                true => app.update(super::Msg::PeekSelectionAction(
                    crate::mouse::PeekSelectionAction::Ask,
                )),
                false => app.status.flash_toast(
                    "Pick lines first: drag across them in the file, or right-click one",
                ),
            }
            true
        }
        UiAction::FocusFiles if app.current_view == super::ViewMode::Chat => {
            app.update(super::Msg::FocusFiles);
            true
        }
        UiAction::ChangesOpenInEditor
            if app.changes.is_some() && app.current_view != super::ViewMode::GuidedReview =>
        {
            editor::open_peeked_file(app);
            true
        }
        UiAction::ReviewOpenInEditor if app.current_view == super::ViewMode::GuidedReview => {
            editor::open_reviewed_file(app);
            true
        }
        // The same key in both diff surfaces: it names a layout, not a surface.
        UiAction::ReviewToggleSplit if app.changes.is_some() => {
            app.update(super::Msg::ToggleChangesSplit);
            true
        }
        UiAction::CycleMode if app.current_view.shows_composer() => {
            cycle_mode(app);
            true
        }
        UiAction::ToggleStatusline => {
            let presentation = app.dock.presentation(app.workspace_layout);
            app.statusline_state.toggle(presentation);
            true
        }
        UiAction::ShowStatusline => {
            app.statusline_state.show();
            true
        }
        UiAction::HideStatusline => {
            app.statusline_state.hide();
            true
        }
        UiAction::ResetStatuslineAuto => {
            app.statusline_state.reset_auto();
            true
        }
        UiAction::ToggleMouse => {
            app.mouse_enabled = !app.mouse_enabled;
            let state = if app.mouse_enabled {
                "Mouse on — the wheel scrolls, a drag selects and copies, a click opens a card. \
                 Hold Shift to hand a drag back to the terminal."
            } else {
                "Mouse off — native terminal text selection restored."
            };
            app.push_convo(ConvoBlock::System(state.to_string()));
            true
        }
        // The pane stands where the transcript does, so it answers the transcript's scroll keys.
        // That is the whole reason it lives in this column: the composer keeps focus, and the
        // reader needs no new vocabulary to move around what replaced the conversation.
        UiAction::ScrollUp | UiAction::ScrollDown if app.changes.is_some() => {
            app.update(super::Msg::ScrollChanges { up: action == UiAction::ScrollUp, lines: 1 });
            true
        }
        UiAction::PageUp | UiAction::PageDown if app.changes.is_some() => {
            app.update(super::Msg::PageChanges(action == UiAction::PageUp));
            true
        }
        UiAction::ScrollUp => {
            app.scroll_state.line_up();
            true
        }
        UiAction::ScrollDown => {
            app.scroll_state.line_down();
            true
        }
        UiAction::PageUp => {
            app.scroll_state.page_up();
            true
        }
        UiAction::PageDown => {
            app.scroll_state.page_down();
            true
        }
        UiAction::SearchConvo => {
            app.search_state.enter();
            true
        }
        // The only thing that sets the block selection: a click opens what it hit rather than
        // selecting it, so this is how a reader picks a block to copy or open.
        UiAction::SelectBlockUp => {
            app.select_block_up();
            true
        }
        UiAction::SelectBlockDown => {
            app.select_block_down();
            true
        }
        UiAction::ToggleToolOutput => {
            app.tool_output_expanded = !app.tool_output_expanded;
            // Reflowing the transcript under a pinned viewport is what the semantic anchor is for,
            // so the row you were reading stays where it was.
            true
        }
        UiAction::OpenImage => {
            match app.newest_visible_image() {
                Some(key) => app.update(super::Msg::OpenImageViewer(key)),
                // Nothing to look at is not a failure, but silence would read as a dead key.
                None => app.push_convo(ConvoBlock::System(
                    "No image in this conversation yet. `screenshot` takes one, and `read` on a \
                     PNG, JPEG, GIF or WebP shows the file itself."
                        .to_string(),
                )),
            }
            true
        }
        UiAction::CopyMessage => {
            // A live text selection is the more specific ask, so it wins over the whole message.
            if super::selection::is_active(app) {
                app.update(super::Msg::CopySelection);
                return true;
            }
            if app.copy_selected_block() {
                let copied = app.outbox.copy.as_ref().map_or(0, String::len);
                tracing::debug!("copy message: {copied} bytes");
            }
            true
        }
        UiAction::CopySelection => {
            // A live selection is the point of the key; with none, fall back to the selected
            // message so a copy key always copies *something* sensible.
            let picked = app.changes.as_ref().is_some_and(|view| view.selection.is_some());
            if picked && !super::selection::is_active(app) {
                super::messages::changes::selection_action(
                    app,
                    crate::mouse::PeekSelectionAction::Copy,
                );
            } else if super::selection::is_active(app) {
                app.update(super::Msg::CopySelection);
            } else {
                app.copy_selected_block();
            }
            true
        }
        UiAction::CopyCode => {
            let code = app.selected_block_text().and_then(|text| first_code_block(&text));
            if let Some(code) = code {
                tracing::debug!("copy code: {} bytes", code.len());
                app.record_copy(code);
            }
            true
        }
        UiAction::EditLastMessage => {
            if let Some(ConvoBlock::Assistant(text)) = app.convo.pop().map(|item| item.into_block())
            {
                app.editor.insert_text(&text);
                tracing::debug!("edit last: {} chars", text.len());
            }
            true
        }
        // Each of these rewrites the end of the conversation, which a running turn is still
        // writing, so they wait for it rather than racing it
        UiAction::Retry | UiAction::Regenerate | UiAction::EditResend if app.turn_in_flight() => {
            app.status.flash_toast("Stop the turn first, or wait for it to finish".to_string());
            true
        }
        UiAction::Retry => {
            let Some(index) = app.last_user_message_index() else {
                app.status.flash_toast("There is no message to send again".to_string());
                return true;
            };
            if !app.send_action(Action::Retry, "retry") {
                return true;
            }
            // After an answer the question goes in again below it; with none it is asked again in
            // place, which is how the engine sends it too
            let answered = app.convo[index + 1..]
                .iter()
                .any(|item| matches!(item.block(), ConvoBlock::Assistant(_)));
            if answered {
                let question = app.convo[index].block().clone();
                app.push_convo(question);
            }
            app.begin_own_turn();
            true
        }
        UiAction::Regenerate => {
            let Some(index) = app.last_user_message_index() else {
                app.status.flash_toast("There is no answer to replace".to_string());
                return true;
            };
            if !app.send_action(Action::Regenerate, "regenerate") {
                return true;
            }
            app.drop_transcript_from(index + 1);
            app.begin_own_turn();
            true
        }
        UiAction::EditResend => {
            if let Some(text) = app.convo.iter().rev().find_map(|item| match item.block() {
                ConvoBlock::User { text, .. } => Some(text.clone()),
                _ => None,
            }) {
                app.editor.insert_text(&text);
                app.edit_resend_mode = true;
                tracing::debug!("edit resend: {} chars", text.len());
            }
            true
        }
        UiAction::PasteClipboard => {
            app.outbox.clipboard_paste = true;
            true
        }
        UiAction::QueueMessage => {
            super::input::submit_prompt_with(app, super::MidTurnDelivery::Queue);
            true
        }
        UiAction::DequeueMessages => {
            let restored = app.restore_pending(|_| true);
            // Said out loud either way: the tray emptying is easy to miss mid-stream, and a key
            // that appears to do nothing is worse than one that says there was nothing to do.
            app.status.flash_toast(match restored {
                0 => "Nothing waiting to restore".to_string(),
                1 => "Restored 1 message to the composer".to_string(),
                n => format!("Restored {n} messages to the composer"),
            });
            true
        }
        UiAction::SelectAll => {
            app.editor.select_all();
            true
        }
        // From the Changes pane this keeps its meaning and gains a destination: the reader has
        // already said which file they care about, so the review opens on it.
        UiAction::ReviewOpen if app.changes.is_some() => {
            app.update(super::Msg::ChangesOpenReview);
            true
        }
        UiAction::ReviewOpen if app.current_view != super::ViewMode::GuidedReview => {
            open_or_start_guided_review(app);
            true
        }
        UiAction::ReviewExit | UiAction::ReviewReturnChat
            if app.current_view == super::ViewMode::GuidedReview =>
        {
            super::navigation::exit_guided_review(app);
            true
        }
        UiAction::ReviewFinish if app.current_view == super::ViewMode::GuidedReview => {
            // A pull request's review finishes on GitHub, with GitHub's verdicts
            if super::messages::github_post::is_pull_request(app) {
                app.update(super::Msg::GuidedReviewOpenGithubPost);
            } else {
                let unsent = !super::input::unsent_finding_ids(app).is_empty();
                app.guided_review_workspace.open_finish_review(unsent);
                // Your own pull request keeps the local choices — committing the fixes is one of
                // them — and adds answering its threads
                app.guided_review_workspace.finish_offers_answers =
                    super::messages::thread_answers::is_own_pull_request(app);
                app.guided_review_workspace.finish_offers_opening =
                    super::messages::pull_request_opening::offered(app);
                app.guided_review_workspace.finish_offers_stack =
                    super::messages::stack_opening::offered(app);
            }
            true
        }
        UiAction::ReviewRefresh if app.current_view == super::ViewMode::GuidedReview => {
            refresh_guided_review(app);
            true
        }
        UiAction::Cancel => cancel::cancel(app),
        UiAction::ReviewChapterNext
        | UiAction::ReviewChapterPrev
        | UiAction::ReviewFileNext
        | UiAction::ReviewFilePrev
        | UiAction::ReviewHunkNext
        | UiAction::ReviewHunkPrev
        | UiAction::ReviewLineNext
        | UiAction::ReviewLinePrev
        | UiAction::ReviewPageNext
        | UiAction::ReviewPagePrev
        | UiAction::ReviewScrollLeft
        | UiAction::ReviewScrollRight
        | UiAction::ReviewLineStart
        | UiAction::ReviewToggleGuide
        | UiAction::ReviewAddFinding
        | UiAction::ReviewEditFinding
        | UiAction::ReviewDeleteFinding
        | UiAction::ReviewReanchorFinding
        | UiAction::ReviewFindings
        | UiAction::ReviewResolveFinding
        | UiAction::ReviewToggleNoise
        | UiAction::ReviewToggleSplit
        | UiAction::ReviewToggleWordWrap
        | UiAction::ReviewAskQuestion
        | UiAction::ReviewOpenInBrowser
        | UiAction::ReviewSend
        | UiAction::ReviewSource
        | UiAction::ReviewOpenSelection
        | UiAction::ReviewHelp => dispatch_review_navigation_action(app, action),
        _ => false,
    }
}

#[cfg(test)]
mod tests;

/// Asks the engine for `/github`'s searches, which it and the `@` picker's GitHub tab both show.
/// Answers whether it was asked.
pub(crate) fn list_github(app: &mut App) -> bool {
    let sections = github_searches(app);
    app.send_action(Action::ListGithub { sections }, "list GitHub")
}

/// `/github`'s tabs as they search now: each configured one, or what the reader changed it to
/// this session.
pub(crate) fn github_searches(app: &App) -> Vec<luxe_core::config::GithubSection> {
    let mut sections = app.config.github.sections();
    for section in &mut sections {
        if let Some(query) = app.github_searches.get(&section.query) {
            section.query = query.clone();
        }
    }
    sections
}
