//! Status data — bounded workspace metadata for the TUI footer.
//!
//! The binary gathers process/environment details before entering the UI. The TUI stays a
//! thin renderer and receives one [`luxe_tui::statusline::StatusContext`] value.

use crate::git_probe;
use luxe_core::provider::ThinkingLevel;
use luxe_tui::statusline::StatusContext;
use std::path::Path;

/// Collects the startup metadata shown by the chat statusline.
///
/// `thinking` is the level the session actually starts on, not a placeholder: the engine only
/// announces one when something changes it, so a model whose own config sets the level would have
/// read `off` in the footer until the first turn.
///
/// `model` is `None` when nobody has chosen one, which the bar reports as `no model` rather than
/// naming something nobody picked.
pub(crate) async fn collect(
    repo_root: &Path,
    model: Option<String>,
    provider: String,
    context_window: u32,
    thinking: ThinkingLevel,
    auto_approve: bool,
) -> StatusContext {
    let (git_branch, git_dirty) =
        tokio::join!(git_probe::branch(repo_root), git_probe::is_dirty(repo_root));
    StatusContext {
        model: model.unwrap_or_default(),
        provider,
        context_window,
        thinking,
        git_branch,
        git_dirty,
        project: Some(luxe_core::paths::dir_name(repo_root)),
        cwd: Some(luxe_core::paths::home_relative(repo_root)),
        auto_approve,
    }
}
