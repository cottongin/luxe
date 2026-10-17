//! Command menu — which commands can run on the screen the user is looking at.

use crate::app::App;

/// What the app can say about itself, for deciding which commands a menu lists.
pub(in crate::app) fn command_context(app: &App) -> crate::commands::CommandContext {
    use crate::commands::{CommandContext, Surface};
    let review = app.current_view == crate::app::ViewMode::GuidedReview;
    CommandContext {
        surface: if review { Surface::Review } else { Surface::Chat },
        pull_request_review: review && crate::app::messages::github_post::is_pull_request(app),
        changes_pane: app.changes.is_some(),
        has_turn: app.has_user_message(),
        has_image: app.newest_visible_image().is_some(),
        messages_waiting: !app.pending_submissions.is_empty(),
        // The launch knows the branch before the first snapshot lands, and the snapshot keeps up
        // with a checkout made since
        in_repository: app.status.git_branch.is_some()
            || app.workspace.as_ref().is_some_and(|state| state.project.git_branch.is_some()),
    }
}

/// The commands that can run here, the recently used first, with the keys bound to them now.
pub(in crate::app) fn command_menu(app: &App) -> crate::commands::Menu {
    crate::commands::CommandRegistry::all().arrange(
        &command_context(app),
        &app.keymap,
        app.recent_commands.ids(),
    )
}
