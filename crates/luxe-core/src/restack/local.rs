//! Local state — what a restack saw of each branch here, and the checks that keep work done since
//! the preview from being reset away.
//!
//! The plan records each moving branch's local tip and the checkout holding it. [`recheck`] runs
//! before the push and again before anything here moves; a probe that fails counts as a refusal,
//! never as "clean".

use super::{git, is_ancestor, Planned, RestackError, RestackStep};
use std::path::{Path, PathBuf};

/// What the plan saw of one branch here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Expected {
    pub(super) head_ref: String,
    /// The local branch's tip, `None` when there was no local branch by that name
    pub(super) local_tip: Option<String>,
    /// The checkout holding it
    pub(super) checked_out: Option<PathBuf>,
}

/// The steps that move a branch, each with what the plan saw of it here.
pub(super) fn moving(planned: &Planned) -> Vec<(&RestackStep, &Expected)> {
    planned
        .plan
        .steps
        .iter()
        .zip(&planned.expected)
        .filter(|(step, _)| step.new != step.old)
        .collect()
}

/// Every moving branch is where the plan saw it, checked out where it was, and clean.
pub(super) async fn recheck(
    project: &Path,
    moving: &[(&RestackStep, &Expected)],
) -> Result<(), RestackError> {
    let listed = worktrees(project).await?;
    for (_, expected) in moving {
        let branch = &expected.head_ref;
        let changed = |what| RestackError::Changed { head_ref: branch.clone(), what };
        if local_tip(project, branch).await != expected.local_tip {
            return Err(changed("its branch moved"));
        }
        if checkout_of(&listed, branch) != expected.checked_out {
            return Err(changed("where it is checked out changed"));
        }
        if let Some(path) = &expected.checked_out {
            clean(branch, path).await?;
        }
    }
    Ok(())
}

/// Moves the branches here to what was pushed, after checking them all again.
///
/// A checkout moves with `reset --keep`, which refuses to lose a change git finds in the way; a
/// branch nobody has checked out moves only if it still holds the tip the plan saw.
pub(super) async fn reconcile(
    project: &Path,
    moving: &[(&RestackStep, &Expected)],
) -> Result<(), RestackError> {
    recheck(project, moving).await?;
    for (step, expected) in moving {
        match (&expected.checked_out, &expected.local_tip) {
            (Some(path), was) => {
                // as close to the move as it can be: a commit since the recheck stays on the branch
                if git(path, &["rev-parse", "HEAD"]).await.ok() != *was {
                    let what = "a commit was made in its checkout";
                    return Err(RestackError::Changed { head_ref: step.head_ref.clone(), what });
                }
                git(path, &["reset", "--quiet", "--keep", &step.new]).await.map_err(|detail| {
                    RestackError::Git { operation: "move the checked-out branch", detail }
                })?;
            }
            (None, Some(was)) => {
                let name = format!("refs/heads/{}", step.head_ref);
                git(project, &["update-ref", &name, &step.new, was]).await.map_err(|detail| {
                    RestackError::Git { operation: "move the local branch", detail }
                })?;
            }
            (None, None) => {}
        }
    }
    Ok(())
}

/// `git worktree list --porcelain`, refusing rather than guessing when git cannot say.
pub(super) async fn worktrees(project: &Path) -> Result<String, RestackError> {
    git(project, &["worktree", "list", "--porcelain"])
        .await
        .map_err(|detail| RestackError::Git { operation: "list the checkouts", detail })
}

async fn local_tip(project: &Path, branch: &str) -> Option<String> {
    git(project, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")]).await.ok()
}

/// Where `branch` is checked out, by the porcelain listing.
fn checkout_of(listed: &str, branch: &str) -> Option<PathBuf> {
    let wanted = format!("branch refs/heads/{branch}");
    let mut current = None;
    let mut checked_out = None;
    for line in listed.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            current = Some(PathBuf::from(path));
        } else if line == wanted {
            checked_out = current.clone();
        }
    }
    checked_out
}

/// The checkout at `path` has nothing uncommitted; a status git cannot give is a refusal.
async fn clean(branch: &str, path: &Path) -> Result<(), RestackError> {
    let status = git(path, &["status", "--porcelain"])
        .await
        .map_err(|detail| RestackError::Git { operation: "check the checkout is clean", detail })?;
    if status.is_empty() {
        return Ok(());
    }
    Err(RestackError::Dirty { head_ref: branch.to_string(), path: crate::paths::display(path) })
}

/// Whether a local branch by this name moves with the stack, and where it is checked out; refused
/// when it has commits GitHub does not, or its checkout has changes.
pub(super) async fn local_state(
    project: &Path,
    listed: &str,
    branch: &str,
    tip: &str,
) -> Result<Expected, RestackError> {
    let Some(local) = local_tip(project, branch).await else {
        return Ok(Expected { head_ref: branch.to_string(), local_tip: None, checked_out: None });
    };
    if local != tip && !is_ancestor(project, &local, tip).await {
        return Err(RestackError::Unpushed(branch.to_string()));
    }
    let checked_out = checkout_of(listed, branch);
    if let Some(path) = &checked_out {
        clean(branch, path).await?;
    }
    Ok(Expected { head_ref: branch.to_string(), local_tip: Some(local), checked_out })
}
