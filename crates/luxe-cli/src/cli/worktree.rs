//! Worktrees — giving a session a checkout of its own, and the issue or pull request it works on.
//!
//! Everything here runs before the engine starts, or between one session and the next in the TUI,
//! so there is one working directory for a session and nothing moves under a running tool.

use anyhow::Context;

use super::Args;

/// Moves the session into its own worktree when it has one: asked for with `--worktree`, recorded
/// by the session being resumed, or configured for every new session.
///
/// Before the engine starts, and only then, so there is one working directory for the whole
/// session and nothing moves under a running tool.
pub(super) async fn enter_worktree(
    args: &Args,
    settings: &mut crate::engine_setup::AgentSettings,
) -> anyhow::Result<()> {
    let resumed = {
        use luxe_core::session::SessionStore;
        let store = crate::engine_setup::open_session_store(&settings.state_root);
        crate::engine_setup::resumed_session(&store, &settings.resume)
            .and_then(|session| store.load(&session).ok())
            .and_then(|entries| luxe_core::session::session_worktree(&entries))
    };
    if let Some(number) = args.pr {
        return check_out_pull_request(number, settings).await;
    }
    let new_session = matches!(settings.resume, crate::engine_setup::ResumeMode::New);
    let always =
        new_session && luxe_core::config::Config::load(&settings.state_root).worktree.always();
    // An issue is taken up in a worktree of its own, on a branch named for it unless one was given
    let issue_branch = match args.issue {
        Some(number) => Some(take_up_issue(number, settings).await?),
        None => None,
    };
    let flag = match (args.worktree.as_deref(), issue_branch.as_deref()) {
        (None | Some(""), Some(branch)) => Some(branch),
        (flag, _) => flag,
    };
    let Some(branch) = worktree_branch(flag, resumed, always, generated_branch) else {
        return Ok(());
    };
    let made = luxe_core::worktree::prepare(&settings.state_root, &branch, None)
        .await
        .with_context(|| format!("preparing the session's worktree for `{branch}`"))?;
    adopt_worktree(settings, made)
}

/// Makes `made` the session's own: entered, announced, and recorded.
fn adopt_worktree(
    settings: &mut crate::engine_setup::AgentSettings,
    made: luxe_core::worktree::SessionWorktree,
) -> anyhow::Result<()> {
    let notice = worktree_notice(&made, &settings.state_root);
    bind_worktree(settings, made.path, made.branch)?;
    settings.startup_notices.push(notice);
    Ok(())
}

/// Moves the process into the worktree and points the session's root and record at it.
fn bind_worktree(
    settings: &mut crate::engine_setup::AgentSettings,
    path: std::path::PathBuf,
    branch: String,
) -> anyhow::Result<()> {
    std::env::set_current_dir(&path)
        .with_context(|| format!("entering the worktree {}", path.display()))?;
    settings.work_root = path.clone();
    settings.worktree = Some(crate::engine_setup::WorktreeBinding { branch, path });
    Ok(())
}

/// Moves into the worktree session `session` works in, or back to the project when it has none.
pub(super) async fn enter_session_worktree(
    session: &str,
    settings: &mut crate::engine_setup::AgentSettings,
) -> anyhow::Result<()> {
    use luxe_core::session::SessionStore;
    let store = crate::engine_setup::open_session_store(&settings.state_root);
    let entries = store
        .load(&luxe_core::session::SessionId(session.to_string()))
        .with_context(|| format!("reading session {session}"))?;
    let Some(branch) = luxe_core::session::session_worktree(&entries) else {
        std::env::set_current_dir(&settings.state_root)
            .with_context(|| format!("entering {}", settings.state_root.display()))?;
        settings.work_root = settings.state_root.clone();
        return Ok(());
    };
    let made = luxe_core::worktree::prepare(&settings.state_root, &branch, None)
        .await
        .with_context(|| format!("finding the session's worktree for `{branch}`"))?;
    adopt_worktree(settings, made)
}

/// Checks pull request `number` out in a worktree of its own and makes it the session's, refusing
/// one a push could not reach.
pub(super) async fn check_out_pull_request(
    number: u64,
    settings: &mut crate::engine_setup::AgentSettings,
) -> anyhow::Result<()> {
    use luxe_core::github::{Branches, Discovery, PullRequestState, PullRequests};
    let github = luxe_core::github::GhCli::new();
    let repository = github
        .repository(&settings.state_root)
        .await
        .with_context(|| format!("finding the repository pull request #{number} is on"))?;
    let pull_request = github
        .pull_request(&repository, number)
        .await
        .with_context(|| format!("reading pull request #{number} on {}", repository.slug()))?;
    if pull_request.state != PullRequestState::Open {
        anyhow::bail!(
            "pull request #{number} is {}, so there is nothing to push a fix to",
            if pull_request.state == PullRequestState::Merged { "merged" } else { "closed" }
        );
    }
    let head = github
        .pull_request_head(&repository, number)
        .await
        .with_context(|| format!("asking who can push to pull request #{number}"))?;
    if let Some(refusal) = head.refusal(number) {
        anyhow::bail!(refusal);
    }
    // One worktree per session: the one already there belongs to the session that made it
    let existing = luxe_core::worktree::worktree_path(&settings.state_root, &head.local_branch);
    if existing.exists() {
        anyhow::bail!(
            "pull request #{number} is already checked out, in `{}`; `luxe --from-pr {number}` \
             resumes the session working on it",
            luxe_core::paths::display(
                existing.strip_prefix(&settings.state_root).unwrap_or(&existing)
            )
        );
    }
    let base_remote = repository.remote.clone().unwrap_or_else(|| "origin".to_string());
    let checkout = luxe_core::worktree::PullRequestCheckout {
        head: &head,
        host: &repository.host,
        base_remote: &base_remote,
    };
    let made = luxe_core::worktree::check_out_pull_request(&settings.state_root, &checkout)
        .await
        .with_context(|| format!("checking out pull request #{number}"))?;
    let worktree = made.worktree;
    let place = luxe_core::paths::display(
        worktree.path.strip_prefix(&settings.state_root).unwrap_or(&worktree.path),
    );
    let author = &pull_request.author.login;
    let title = luxe_core::github::untrusted(pull_request.title.trim(), 120).replace('\n', " ");
    let mut notice = format!(
        "Pull request #{number}, \"{title}\" by @{author}, is checked out in its own worktree, \
         `{place}`, on `{}`. `/pr` reviews it against its base: your comments go to the agent, and \
         a plain `git push` lands on the pull request. Your checkout is untouched.",
        worktree.branch
    );
    if made.remote_added {
        notice.push_str(&format!(" The fork is reached by a new remote, `{}`.", made.remote));
    }
    bind_worktree(settings, worktree.path, worktree.branch)?;
    settings.startup_notices.push(notice);
    settings.pull_request = Some((number, pull_request.url.clone()));
    Ok(())
}

/// Takes issue `number` up in a worktree of its own, as `luxe --issue` does from the command line.
pub(super) async fn take_up_issue_in_worktree(
    number: u64,
    settings: &mut crate::engine_setup::AgentSettings,
) -> anyhow::Result<()> {
    let branch = take_up_issue(number, settings).await?;
    let made = luxe_core::worktree::prepare(&settings.state_root, &branch, None)
        .await
        .with_context(|| format!("preparing the session's worktree for `{branch}`"))?;
    adopt_worktree(settings, made)
}

/// Reads issue `number` for the session to take up, answering the branch its work goes on.
async fn take_up_issue(
    number: u64,
    settings: &mut crate::engine_setup::AgentSettings,
) -> anyhow::Result<String> {
    use luxe_core::github::{Discovery, PullRequests};
    let github = luxe_core::github::GhCli::new();
    let repository = github
        .repository(&settings.state_root)
        .await
        .with_context(|| format!("finding the repository issue #{number} is on"))?;
    let issue = github
        .issue(&repository, number)
        .await
        .with_context(|| format!("reading issue #{number} on {}", repository.slug()))?;
    settings.startup_notices.push(format!(
        "Issue #{number} — {} will be sent with your first message as additional context.",
        luxe_core::github::untrusted(issue.title.trim(), 120).replace('\n', " ")
    ));
    settings.issue = Some(crate::engine_setup::IssueBinding {
        number,
        url: issue.url.clone(),
        brief: luxe_core::issue::brief(&issue),
    });
    Ok(luxe_core::issue::branch_name(&issue))
}

/// The branch the session's worktree is on, or `None` for a session without one.
///
/// A branch named on the command line wins; a bare `--worktree` keeps the resumed session's branch,
/// or makes one up; a resumed session that had a worktree re-enters it without being asked; and
/// `worktree.always` gives a new session one.
pub(super) fn worktree_branch(
    flag: Option<&str>,
    resumed: Option<String>,
    always: bool,
    generate: impl FnOnce() -> String,
) -> Option<String> {
    match (flag, resumed) {
        (Some(""), Some(resumed)) | (None, Some(resumed)) => Some(resumed),
        (Some(""), None) => Some(generate()),
        (Some(branch), _) => Some(branch.to_string()),
        (None, None) => always.then(generate),
    }
}

/// A branch name for a session that did not choose one: `luxe/` and a short random part.
pub(super) fn generated_branch() -> String {
    let id = uuid::Uuid::new_v4().simple().to_string();
    format!("luxe/session-{}", &id[..8])
}

/// The notice a session in a worktree starts with.
pub(super) fn worktree_notice(
    made: &luxe_core::worktree::SessionWorktree,
    project: &std::path::Path,
) -> String {
    let path = made.path.strip_prefix(project).unwrap_or(&made.path);
    let place = format!("`{}` on branch `{}`", luxe_core::paths::display(path), made.branch);
    let origin = match (&made.base, made.created, made.fetch_failed) {
        (Some(base), true, true) => {
            format!(", made from `{base}` as last fetched: the remote could not be reached")
        }
        (Some(base), true, false) => format!(", made from `{base}`"),
        (None, true, _) => ", made again from its branch".to_string(),
        (_, false, _) => String::new(),
    };
    format!(
        "Working in the session's own worktree, {place}{origin}. Your checkout is untouched; \
         `/worktree` says more."
    )
}

/// Leaves the session's worktree and removes it, saying how it went on the terminal luxe has
/// just handed back.
pub(super) async fn remove_worktree(project: &std::path::Path, path: &std::path::Path) {
    if let Err(error) = std::env::set_current_dir(project) {
        eprintln!("luxe: could not leave the worktree to remove it: {error}");
        return;
    }
    let place = luxe_core::paths::display(path.strip_prefix(project).unwrap_or(path));
    match luxe_core::worktree::remove(project, path).await {
        Ok(()) => eprintln!("luxe: removed the session's worktree `{place}`; its branch is kept."),
        Err(error) => eprintln!("luxe: the worktree `{place}` was kept: {error}"),
    }
}
