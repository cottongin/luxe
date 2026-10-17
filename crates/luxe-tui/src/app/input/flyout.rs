//! Composer flyouts — the `/` and `@` completions that grow out of the input.
//!
//! Unlike the pickers these are *part of* typing: an `Edit` outcome hands the key back to the
//! editor and re-syncs, which is what keeps the flyout tracking the word being typed.

use crossterm::event::KeyEvent;

use super::super::{commands, App};
use super::{active_mention_query, active_slash_query, sync_completion_flyouts};

/// Answers whether this layer took the key.
pub(super) fn flyout_took(app: &mut App, key: KeyEvent) -> bool {
    if app.overlays.slash_flyout.is_some() {
        slash_flyout_key(app, key);
        return true;
    }
    if app.overlays.mention.is_some() {
        mention_key(app, key);
        return true;
    }
    false
}

/// The `/` flyout's answer to a key: run, complete or fill a command, or hand the key back to
/// typing.
fn slash_flyout_key(app: &mut App, key: KeyEvent) {
    let Some(flyout) = app.overlays.slash_flyout.as_mut() else {
        return;
    };
    use crate::slash_flyout::SlashFlyoutOutcome;
    match flyout.on_key(&key) {
        SlashFlyoutOutcome::Dispatch(command) => {
            app.overlays.slash_flyout = None;
            app.editor.cancel();
            // On the start screen a command either runs over it or gets a session first,
            // which is the launcher's call, not the session's
            match app.current_view {
                super::super::ViewMode::Dashboard => {
                    app.handle_dashboard_outcome(crate::dashboard::DashOutcome::RunCommand(command))
                }
                _ => commands::dispatch(app, command),
            }
        }
        SlashFlyoutOutcome::Complete(command) => {
            if let Some((range, _)) = active_slash_query(&app.editor) {
                let completion =
                    format!("{} ", command.slash_name().unwrap_or(command.name.as_ref()));
                app.editor.replace_range(range, &completion);
            }
            app.overlays.slash_flyout = None;
            // Straight on to its forms, for a command that takes arguments
            sync_completion_flyouts(app);
        }
        SlashFlyoutOutcome::Fill(draft) => {
            if let Some(range) = super::slash_command_range(&app.editor) {
                app.editor.replace_range(range, &draft);
            }
            sync_completion_flyouts(app);
        }
        SlashFlyoutOutcome::Close => {
            // Put away, the forms stay away for this command: they would otherwise come back
            // on the next keystroke of whatever is being typed after it
            let gone = app.overlays.slash_flyout.take();
            app.slash_arguments_dismissed = gone.and_then(|f| f.arguments_of().map(String::from));
        }
        SlashFlyoutOutcome::Consumed => {}
        SlashFlyoutOutcome::Edit => {
            app.editor.handle_key(key);
            sync_completion_flyouts(app);
        }
    }
}

/// The `@` picker's answer to a key: place a reference, fetch a tab, or hand the key back to
/// typing.
fn mention_key(app: &mut App, key: KeyEvent) {
    let Some(mention) = app.overlays.mention.as_mut() else {
        return;
    };
    use crate::mention_flyout::MentionFlyoutOutcome;
    match mention.on_key(&key) {
        MentionFlyoutOutcome::Insert(reference) => {
            place_reference(app, &format!("@[file:{}]", reference.target));
            app.overlays.mention = None;
        }
        MentionFlyoutOutcome::InsertGithub(item) => {
            place_reference(app, &crate::mentions::github_token(&item));
            app.github_rows.insert(crate::mentions::github_target(&item), *item);
            app.overlays.mention = None;
        }
        MentionFlyoutOutcome::InsertSkill(name) => {
            place_reference(app, &format!("@[skill:{name}]"));
            app.overlays.mention = None;
        }
        MentionFlyoutOutcome::InsertMcp(id) => {
            place_reference(app, &format!("@[mcp:{id}]"));
            app.overlays.mention = None;
        }
        MentionFlyoutOutcome::InsertRow(picked) => {
            let (source, row) = *picked;
            let kind = source.kind();
            place_reference(app, &format!("@[{kind}:{}]", row.id));
            app.mention_rows.insert(format!("{kind}:{}", row.id), row.text);
            app.overlays.mention = None;
        }
        MentionFlyoutOutcome::Fetch(tab) => {
            use crate::mention_flyout::MentionTab;
            // Listed already this session: the rows are kept, and `r` in `/github` refreshes
            match tab {
                // Files, skills and servers are listed when the picker opens
                MentionTab::Files | MentionTab::Skills | MentionTab::Mcp => {}
                MentionTab::GitHub => match app.github_listing.clone() {
                    Some(listing) => mention.set_github(listing, None),
                    None => {
                        super::super::commands::list_github(app);
                    }
                },
                MentionTab::Source(source) => match app.mention_listings.get(&source.kind()) {
                    Some(rows) => mention.set_source(&source.kind(), Ok(rows.clone())),
                    None => app.outbox.mention_source = Some(source),
                },
            }
        }
        MentionFlyoutOutcome::RefreshFiles => {
            app.file_listing.request();
        }
        MentionFlyoutOutcome::Close => {
            app.overlays.mention = None;
        }
        MentionFlyoutOutcome::Consumed => {}
        MentionFlyoutOutcome::Edit => {
            app.editor.handle_key(key);
            sync_completion_flyouts(app);
        }
    }
}

/// Puts `token` where the `@` query being typed is, as one chip, a space after it unless one is
/// there already.
fn place_reference(app: &mut App, token: &str) {
    let Some((range, _)) = active_mention_query(&app.editor) else {
        return;
    };
    let needs_space = app
        .editor
        .current_input()
        .get(range.end..)
        .and_then(|suffix| suffix.chars().next())
        .is_none_or(|character| !character.is_whitespace());
    app.editor.replace_range(range, "");
    app.editor.insert_reference(token);
    if needs_space {
        app.editor.insert_text(" ");
    }
}
