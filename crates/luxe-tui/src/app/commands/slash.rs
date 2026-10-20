//! Slash commands — the commands run by id, and what each one does.

use super::super::{App, ConvoBlock};
use super::{
    cycle_mode, dispatch_action, editor, github_searches, list_github, open_pull_request_card,
    parse_review_target, refresh_guided_review, start_guided_review, toggle_changes, toggle_keys,
};
use crate::commands::id;
use crate::theme::styles::ThemeStyles;
use luxe_core::config::{Config, DockPosition};
use luxe_core::engine::Action;
use luxe_core::keymap::{load_keybindings, UiAction};

/// Runs a command handled by id, with whatever arguments were typed after its trigger, and answers
/// whether its id is one this knows.
pub(super) fn run_slash(app: &mut App, cmd: &crate::commands::Command) -> bool {
    match cmd.id.as_ref() {
        id::NEW => {
            if app.leave_for_session(crate::app::NextSession::New) {
                tracing::info!("/new — starting a new session");
            }
        }
        id::SESSIONS => app.open_sessions(),
        id::UPDATE => app.outbox.update = true,
        id::MODEL => {
            // Opens the searchable model picker (also reachable from the palette). Selecting a
            // model sends `Action::SetModel`, switching the active model in-session.
            app.open_model_picker();
        }
        id::THINKING => app.open_thinking_picker(),
        id::THEME => app.open_theme_picker(),
        id::LOGIN => {
            // `/login openai` skips the provider list; a bare `/login` shows it.
            app.open_auth_wizard(cmd.args.as_deref().map(str::trim).filter(|a| !a.is_empty()));
        }
        id::CONFIG => {
            app.open_configuration();
        }
        id::SETTINGS => {
            app.open_settings();
        }
        id::LOGOUT => {
            app.logout_provider(cmd.args.as_deref().map(str::trim).filter(|a| !a.is_empty()));
        }
        id::AUTH => {
            app.report_auth_status();
        }
        id::HELP => toggle_keys(app),
        // What the workspace last heard, at once, and a fresh look behind it: the snapshot that
        // lands afterwards brings the statusline and the dock up to date
        id::GITHUB => {
            if list_github(app) {
                let configured = app.config.github.sections();
                let searches = github_searches(app);
                let panel = crate::github_panel::GithubPanel::with_searches(&configured, searches);
                app.overlays.github = Some(panel);
            }
        }
        id::STACK => {
            if app.send_action(
                Action::PlanRestack { request_id: luxe_core::restack::RestackId::new() },
                "plan a restack",
            ) {
                app.overlays.restack = Some(crate::restack_panel::RestackPanel::Planning);
            }
        }
        id::PR => {
            let presence = app
                .workspace
                .as_ref()
                .map(|workspace| workspace.pull_request.clone())
                .unwrap_or_default();
            app.send_quietly(Action::RefreshWorkspace, "refresh the workspace");
            // A pull request opens in its card; without one, the sentence saying why there is none
            match &presence {
                luxe_core::workspace::PullRequestPresence::Found(pr) => {
                    open_pull_request_card(app, pr.clone());
                }
                _ => app.push_convo(ConvoBlock::System(crate::pull_request::report(&presence))),
            }
        }
        id::WORKTREE => worktree_command(app, cmd.args.as_deref()),
        id::EDITOR => editor::editor_command(app, cmd.args.as_deref()),
        id::COST => {
            let report = crate::cost_report::render_with_plan(
                &app.status.session,
                app.status.plan_usage.as_ref(),
            );
            app.push_convo(ConvoBlock::System(report));
        }
        // The engine owns the measurement — it is the only place the assembled prompt, the tool
        // block and the active transcript all exist at once — so the report arrives as an event.
        id::CONTEXT => {
            app.send_action(Action::RequestContextComposition, "show the context");
        }
        // Same reasoning as `/context`: only the engine can see the transcript, and the summary
        // costs a provider call, so the answer comes back as an event rather than inline.
        id::COMPACT => {
            let focus = cmd.args.as_deref().map(str::trim).filter(|args| !args.is_empty());
            let action = Action::CompactNow { focus: focus.map(str::to_string) };
            app.send_action(action, "compact");
        }
        id::LSP => {
            let text = match app.workspace.as_ref() {
                Some(workspace) => crate::tooling::render_lsp(&workspace.tooling),
                None => {
                    "Language-server status isn't ready yet — try again in a moment.".to_string()
                }
            };
            app.push_convo(ConvoBlock::System(text));
        }
        id::DEBUG => {
            let text = match app.workspace.as_ref() {
                Some(workspace) => crate::tooling::render_debug(&workspace.tooling),
                None => "Debug-adapter status isn't ready yet — try again in a moment.".to_string(),
            };
            app.push_convo(ConvoBlock::System(text));
        }
        id::IMAGES => {
            let report = crate::tooling::render_images(
                app.images.tier(),
                crate::graphics::ImageTier::resolve_detail(app.config.images.unwrap_or_default())
                    .reason,
                app.config.conversation.image_max_rows(),
            );
            app.push_convo(ConvoBlock::System(report));
        }
        id::MCP => {
            let text = match app.workspace.as_ref() {
                Some(workspace) => crate::tooling::render_mcp(&workspace.tooling),
                None => "MCP status isn't ready yet — try again in a moment.".to_string(),
            };
            app.push_convo(ConvoBlock::System(text));
        }
        id::TOOLS => {
            let text = match app.workspace.as_ref() {
                Some(workspace) => crate::tooling::render_tools(&workspace.tooling),
                None => "Tool status isn't ready yet — try again in a moment.".to_string(),
            };
            app.push_convo(ConvoBlock::System(text));
        }
        // `/peek [path]` — the cheap look. Bare, it opens whatever the dock leads with; with a
        // path, that file. A path nothing has changed still opens, because "this file is clean" is
        // an answer and refusing to show it is not.
        id::CHANGES => match cmd.args.as_deref().map(str::trim).filter(|path| !path.is_empty()) {
            Some(path) => app.update(crate::app::Msg::OpenChanges(path.to_string())),
            None => toggle_changes(app),
        },
        id::DIFF => {
            let arguments = cmd.args.as_deref().map(str::trim).filter(|args| !args.is_empty());
            if arguments == Some("refresh") {
                refresh_guided_review(app);
            } else if arguments.is_none() {
                // The same verb as its key: the review already pinned, or one taken now
                dispatch_action(app, UiAction::ReviewOpen);
            } else {
                let target =
                    match parse_review_target(cmd.args.as_deref(), app.review.last_turn.as_ref()) {
                        Ok(target) => target,
                        Err(message) => {
                            app.push_convo(ConvoBlock::System(message.to_string()));
                            return true;
                        }
                    };
                start_guided_review(app, target);
            }
        }
        id::DOCK => {
            let action = match cmd.args.as_deref() {
                None => UiAction::ToggleDock,
                Some("show") => UiAction::ShowDock,
                Some("hide") => UiAction::HideDock,
                Some("auto") => UiAction::ResetDockAuto,
                Some("left") => {
                    app.dock.position = DockPosition::Left;
                    return true;
                }
                Some("right") => {
                    app.dock.position = DockPosition::Right;
                    return true;
                }
                Some(_) => {
                    app.push_convo(ConvoBlock::System(
                        "Usage: /dock [show|hide|auto|left|right]".to_string(),
                    ));
                    return true;
                }
            };
            dispatch_action(app, action);
        }
        id::STATUSLINE => {
            let action = match cmd.args.as_deref() {
                None | Some("toggle") => UiAction::ToggleStatusline,
                Some("show") => UiAction::ShowStatusline,
                Some("hide") => UiAction::HideStatusline,
                Some("auto") => UiAction::ResetStatuslineAuto,
                Some(_) => {
                    app.push_convo(ConvoBlock::System(
                        "Usage: /statusline [show|hide|auto|toggle]".to_string(),
                    ));
                    return true;
                }
            };
            dispatch_action(app, action);
        }
        id::MOUSE => {
            // Set an explicit state, or toggle when bare; ToggleMouse pushes the status line.
            let desired = match cmd.args.as_deref() {
                None | Some("toggle") => !app.mouse_enabled,
                Some("on") => true,
                Some("off") => false,
                Some(_) => {
                    app.push_convo(ConvoBlock::System("Usage: /mouse [on|off|toggle]".to_string()));
                    return true;
                }
            };
            if desired != app.mouse_enabled {
                dispatch_action(app, UiAction::ToggleMouse);
            }
        }
        id::MODE => match cmd.args.as_deref().map(str::trim).filter(|arg| !arg.is_empty()) {
            // The engine knows the modes and answers a name it does not with a notice
            Some(name) => {
                app.send_action(Action::SetMode(name.to_ascii_lowercase()), "change the mode");
            }
            None => cycle_mode(app),
        },
        id::REVIEW => {
            let focus = cmd.args.as_deref().map(str::trim).filter(|arg| !arg.is_empty());
            let action = Action::RequestReview { focus: focus.map(str::to_string) };
            app.send_action(action, "request a review");
        }
        id::PERMISSIONS => permissions_command(app, cmd.args.as_deref()),
        _ => return false,
    }
    true
}

/// Turns auto-approve on, off, or over (`/permissions [on|off|toggle]`).
fn permissions_command(app: &mut App, arguments: Option<&str>) {
    let desired = match arguments.map(str::trim) {
        None | Some("") | Some("toggle") => !app.permission_auto_approve,
        Some("on") => true,
        Some("off") => false,
        Some(_) => {
            app.push_convo(ConvoBlock::System(
                "Usage: /permissions [on|off|toggle] — on auto-approves tool asks; off prompts"
                    .to_string(),
            ));
            return;
        }
    };
    // the screen says what the engine does, so it changes only once the engine was told
    if !app.send_action(Action::SetPermissionAutoApprove(desired), "set auto-approve") {
        return;
    }
    app.permission_auto_approve = desired;
    let msg = if desired {
        "Auto-approve ON — tool permission asks are allowed automatically (deny rules still apply)."
    } else {
        "Auto-approve OFF — you'll be prompted before asked-for tool calls."
    };
    app.push_convo(ConvoBlock::System(msg.to_string()));
}

pub(crate) fn reload_config(app: &mut App, repo_root: &std::path::Path) {
    let new_config = Config::load(repo_root);
    app.themes.reread();
    for notice in crate::app::theme_notices(&new_config, &app.themes) {
        app.push_convo(ConvoBlock::System(notice));
    }
    let theme_name = new_config.resolved_theme_name(&app.themes);
    let resolved = new_config.resolved_theme(&app.themes);
    let dock_mode = new_config.dock.mode();
    app.theme_styles = ThemeStyles::from_resolved(&resolved, &theme_name);
    app.dock.configured = dock_mode;
    app.dock.position = new_config.dock.position();
    crate::glyphs::resolve_vocabulary(new_config.glyphs());
    app.statusline_state.configured = new_config.statusline.visibility();
    app.guided_review_workspace.word_wrap = new_config.diff.word_wrap();
    app.guided_review_workspace.layout = new_config.diff.layout();
    app.banner = app.banner.with_animation(new_config.banner_animation());
    app.notifier = crate::notify::create_notifier(
        new_config.any_notifications(),
        new_config.notification_method(),
    );
    app.config = new_config;
    app.keymap = app.keymap.clone().with_overrides(load_keybindings(repo_root));
    tracing::info!(theme = theme_name, "reload complete — config + keybindings refreshed");
}

/// Where the session works, and — with `remove` — ending it and removing its worktree.
///
/// Removal is refused while the worktree has anything uncommitted, naming it: the branch keeps
/// the committed work, and nothing else would. Git refuses too, which is the last word.
fn worktree_command(app: &mut App, arguments: Option<&str>) {
    let snapshot = app.workspace.as_ref();
    let worktree = snapshot.and_then(|snapshot| snapshot.worktree.clone());
    let branch = snapshot.and_then(|snapshot| snapshot.project.git_branch.clone());
    let root = snapshot.map(|snapshot| snapshot.project.root.clone());
    let say = |app: &mut App, text: String| app.push_convo(ConvoBlock::System(text));
    match (arguments.map(str::trim).filter(|arguments| !arguments.is_empty()), worktree, root) {
        (None, Some(place), _) => say(
            app,
            format!(
                "This session works in its own worktree, `{place}`, on branch `{}`. Your checkout \
                 is untouched. `/worktree remove` ends the session and removes the worktree once \
                 its work is committed; the branch stays, and resuming the session makes the \
                 worktree again.",
                branch.unwrap_or_default()
            ),
        ),
        (None, None, _) => say(
            app,
            "This session works in your checkout. `luxe --worktree` starts one that works in a \
             checkout of its own."
                .into(),
        ),
        (Some("remove"), Some(_), Some(root)) => {
            // Asked of git now, not read from the last snapshot, which may predate a file made
            // outside luxe
            if app.send_action(Action::CheckWorktreeRemoval, "check the worktree") {
                app.worktree_removal_check = Some(root);
            }
        }
        (Some("remove"), _, _) => {
            say(app, "This session has no worktree of its own to remove.".into());
        }
        (Some(other), _, _) => {
            say(app, format!("`/worktree {other}` is not something it does: try `/worktree` or `/worktree remove`."));
        }
    }
}

#[cfg(test)]
mod tests;
