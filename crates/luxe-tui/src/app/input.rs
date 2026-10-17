//! Composer input — key routing, completion, and key-map resolution.
//!
//! The editor buffer remains the source of truth for completion overlays. Routing preserves the
//! precedence of the palette, slash completion, mention completion, configured shortcuts, then
//! ordinary composer input.

mod chord;
mod files;
mod flyout;
mod modal;
mod peek;
mod picker;
mod review;
pub mod review_keys;
mod submit;

pub(crate) use chord::{chord_of, resolve_key_event};
pub(crate) use modal::modal_took_paste;
pub(super) use submit::{submit_prompt, submit_prompt_with};

use super::{commands, App, ConvoBlock, MidTurnDelivery};
use crate::editor::Editor;
use luxe_core::keymap::UiAction;

/// Finds the current-source failed submission that owns the selected finding.
pub(crate) fn retryable_submission_for_selected_finding(
    app: &App,
) -> Option<luxe_core::guided_review::SubmissionId> {
    let session = app.review.session()?;
    let finding_id = app.guided_review_workspace.selected_finding_id.as_ref()?;
    session
        .submissions
        .iter()
        .rev()
        .find(|submission| {
            submission.dispatch == luxe_core::guided_review::SubmissionDispatch::FailedRetryable
                && submission.revision_id == session.source.content_digest
                && submission.findings.iter().any(|finding| &finding.finding_id == finding_id)
        })
        .map(|submission| submission.id.clone())
}

/// Finds the current-source queued submission that owns the selected finding.
pub(crate) fn queued_submission_for_selected_finding(
    app: &App,
) -> Option<luxe_core::guided_review::SubmissionId> {
    let session = app.review.session()?;
    let finding_id = app.guided_review_workspace.selected_finding_id.as_ref()?;
    session
        .submissions
        .iter()
        .rev()
        .find(|submission| {
            submission.dispatch == luxe_core::guided_review::SubmissionDispatch::Queued
                && submission.revision_id == session.source.content_digest
                && submission.findings.iter().any(|finding| &finding.finding_id == finding_id)
        })
        .map(|submission| submission.id.clone())
}

/// Every comment not yet delivered to the agent, in review order.
///
/// A submission carries all of them regardless of severity. A severity that routed comments would
/// be a hidden switch: `Request fixes` taking only Issue/Blocking would leave Suggestions
/// unsendable and silently discard a Note beside an Issue.
pub(crate) fn unsent_finding_ids(app: &App) -> Vec<luxe_core::guided_review::FindingId> {
    app.review
        .session()
        .map(luxe_core::guided_review::ReviewSession::unsent_finding_ids)
        .unwrap_or_default()
}

/// Whether any unsent comment asks the agent for work, so `Request fixes` has something to ask.
pub(crate) fn has_actionable_findings(app: &App) -> bool {
    use luxe_core::guided_review::{FindingSeverity, FindingState};
    app.review.session().is_some_and(|session| {
        session.findings.iter().any(|finding| {
            matches!(finding.state, FindingState::Draft | FindingState::Open)
                && matches!(
                    finding.severity,
                    FindingSeverity::Suggestion
                        | FindingSeverity::Issue
                        | FindingSeverity::Blocking
                )
        })
    })
}

/// Routes a key through overlays, configured actions, then the composer.
pub(super) fn handle_key(app: &mut App, key: crossterm::event::KeyEvent) {
    use crossterm::event::{KeyCode, KeyModifiers};

    // Precedence, top to bottom: whatever is drawn nearest the reader owns the key. Each rung
    // answers whether it took it; the composer at the bottom takes whatever is left.
    if modal::modal_took(app, key)
        || picker::picker_took(app, key)
        || flyout::flyout_took(app, key)
        || review::review_layer_took(app, key)
        || peek::peek_took(app, key)
        || files::files_took(app, key)
    {
        return;
    }
    // Guided Review is full-screen and owns plain letters, so its navigation gets the key before
    // the composer does — but *after* the rungs above, because its own overlays are drawn nearer
    // the reader than it is. `None` means the surface did not claim it.
    if app.current_view == super::ViewMode::GuidedReview {
        if let Some(message) = review_keys::claim(app, &key) {
            app.update(message);
            return;
        }
    }
    // Esc drops a live text selection before anything else claims it (without a selection it
    // still falls through to the composer, where it clears the draft).
    if key.code == KeyCode::Esc && key.modifiers.is_empty() && super::selection::is_active(app) {
        app.update(super::Msg::ClearTextSelection);
        return;
    }
    // Then it puts the Changes pane away and gives the column back to the transcript. Ahead of
    // the composer's own Esc, because a visible surface is what the key most obviously means.
    if key.code == KeyCode::Esc && key.modifiers.is_empty() && app.changes.is_some() {
        app.update(super::Msg::CloseChanges);
        return;
    }

    let toggles_selected_tool = matches!(key.code, KeyCode::Enter | KeyCode::Char(' '))
        && !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT);
    if toggles_selected_tool && app.toggle_selected_tool_details() {
        return;
    }

    let is_text_input = matches!(key.code, KeyCode::Char(_))
        && !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
    if app.current_view == super::ViewMode::GuidedReview
        && key.code == KeyCode::Char('r')
        && key.modifiers == KeyModifiers::CONTROL
    {
        app.dispatch_action(UiAction::ReviewRefresh);
        return;
    }
    // A launcher takes its key before any action does: it is the reader's own binding
    if let Some(launcher) = crate::launcher::bound(&app.config.launchers, &key) {
        app.outbox.launch = Some(launcher.clone());
        return;
    }
    if let Some(action) = resolve_key_event(&app.keymap, key) {
        let is_review_action = action.is_review();
        if (!is_text_input || is_review_action) && app.dispatch_action(action) {
            return;
        }
    }

    // Alt+Enter queues a follow-up turn instead of steering the running one. Handled here rather
    // than in the keymap because Alt+Enter is chat-only (Guided Review binds it to Finish).
    if key.code == KeyCode::Enter
        && key.modifiers == KeyModifiers::ALT
        && app.current_view == super::ViewMode::Chat
    {
        submit_prompt_with(app, MidTurnDelivery::Queue);
        return;
    }

    let is_submit =
        matches!(key.code, KeyCode::Enter) && !key.modifiers.contains(KeyModifiers::SHIFT);
    if !is_submit {
        app.editor.handle_key(key);
        sync_completion_flyouts(app);
        return;
    }

    let compact_input = app.editor.current_input().trim().to_string();
    if compact_input.starts_with('/') {
        let registry = crate::commands::CommandRegistry::all();
        if let Some(command) = registry.parse_slash(&compact_input) {
            app.editor.cancel();
            commands::dispatch(app, command);
        } else {
            app.push_convo(ConvoBlock::System(format!("Unknown command: {compact_input}")));
            app.editor.cancel();
        }
        return;
    }
    if compact_input.is_empty() && !app.editor.has_images() {
        return;
    }

    submit_prompt(app);
}

pub(super) fn sync_completion_flyouts(app: &mut App) {
    if let Some((_, query)) = active_slash_query(&app.editor) {
        // Back in the trigger: whatever is typed next is a different command's arguments
        app.slash_arguments_dismissed = None;
        if app.overlays.slash_flyout.as_ref().is_none_or(|flyout| flyout.arguments_of().is_some()) {
            let menu = commands::command_menu(app);
            app.overlays.slash_flyout = Some(crate::slash_flyout::SlashFlyoutState::new(menu));
        }
        if let Some(flyout) = app.overlays.slash_flyout.as_mut() {
            flyout.set_query(&query);
        }
        app.overlays.mention = None;
        return;
    }
    if sync_argument_forms(app) {
        app.overlays.mention = None;
        return;
    }
    app.overlays.slash_flyout = None;

    if let Some((_, query)) = active_mention_query(&app.editor) {
        let repo_root = std::env::current_dir().unwrap_or_default();
        // As the picker opens: which files are being worked on, and a fresh read of the disk,
        // which lands a moment later without holding up the keystroke that opened it
        if app.overlays.mention.is_none() {
            if let Some(snapshot) = app.workspace.as_ref() {
                app.file_provider.set_working(&snapshot.changes.files);
            }
            app.file_listing.request();
        }
        let provider = app.file_provider.clone();
        let sources = &app.config.mentions;
        let servers = app.workspace.as_ref().map_or(&[][..], |w| &w.tooling.mcp_servers[..]);
        let mention = app.overlays.mention.get_or_insert_with(|| {
            let skills = luxe_core::skills::discover(&repo_root);
            crate::mention_flyout::MentionFlyoutState::new(
                provider, repo_root, sources, skills, servers,
            )
        });
        mention.set_query(&query);
    } else {
        app.overlays.mention = None;
    }
}

/// Lists the argument forms of the command being typed, once the draft is past its trigger.
/// Returns whether the flyout is showing them.
///
/// Only for a command that can run here and takes arguments, only on the command's own line, and
/// not after the reader put the list away for this command.
fn sync_argument_forms(app: &mut App) -> bool {
    let Some((trigger, typed)) = slash_arguments(&app.editor) else {
        app.slash_arguments_dismissed = None;
        return false;
    };
    if app.slash_arguments_dismissed.as_deref() == Some(trigger.as_str()) {
        app.overlays.slash_flyout = None;
        return false;
    }
    let same = app
        .overlays
        .slash_flyout
        .as_ref()
        .is_some_and(|flyout| flyout.arguments_of() == Some(trigger.as_str()));
    if same {
        let fits =
            app.overlays.slash_flyout.as_mut().is_some_and(|flyout| flyout.set_arguments(&typed));
        if !fits {
            app.overlays.slash_flyout = None;
        }
        return fits;
    }
    let command = commands::command_menu(app)
        .entries
        .into_iter()
        .map(|entry| entry.command)
        .find(|command| command.slash_name() == Some(trigger.as_str()));
    app.overlays.slash_flyout = command
        .and_then(|command| crate::slash_flyout::SlashFlyoutState::arguments(command, &typed));
    app.overlays.slash_flyout.is_some()
}

/// The trigger and what follows it up to the cursor, while the cursor is past the trigger of a
/// draft that starts with one, on its first line.
pub(super) fn slash_arguments(editor: &Editor) -> Option<(String, String)> {
    let range = slash_command_range(editor)?;
    let line = &editor.current_input()[range];
    let (trigger, typed) = line.split_once(char::is_whitespace)?;
    Some((trigger.to_string(), typed.to_string()))
}

/// The stretch of the draft a slash command and its arguments occupy, up to the cursor: what a
/// filled-in form replaces.
pub(super) fn slash_command_range(editor: &Editor) -> Option<std::ops::Range<usize>> {
    let cursor = editor.cursor();
    if !editor.current_input().starts_with('/') || !editor.current_input().is_char_boundary(cursor)
    {
        return None;
    }
    let before = editor.current_input().get(..cursor)?;
    (!before.contains('\n')).then_some(0..cursor)
}

pub(super) fn active_slash_query(editor: &Editor) -> Option<(std::ops::Range<usize>, String)> {
    let cursor = editor.cursor();
    if cursor == 0
        || cursor > editor.current_input().len()
        || !editor.current_input().is_char_boundary(cursor)
        || !editor.current_input().starts_with('/')
    {
        return None;
    }
    let token = editor.current_input().get(1..cursor)?;
    if token.chars().any(char::is_whitespace) {
        return None;
    }
    Some((0..cursor, token.to_string()))
}

pub(super) fn active_mention_query(editor: &Editor) -> Option<(std::ops::Range<usize>, String)> {
    let cursor = editor.cursor();
    if cursor == 0
        || cursor > editor.current_input().len()
        || !editor.current_input().is_char_boundary(cursor)
    {
        return None;
    }
    let before = editor.current_input().get(..cursor)?;
    let start = before
        .char_indices()
        .rev()
        .find(|(_, character)| character.is_whitespace())
        .map(|(index, character)| index + character.len_utf8())
        .unwrap_or(0);
    let token = before.get(start..)?;
    let query = token.strip_prefix('@')?;
    if query.starts_with('[') {
        return None;
    }
    Some((start..cursor, query.to_string()))
}
