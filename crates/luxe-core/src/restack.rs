//! Restack — moving a stack's branches back onto the ones below them after one changes.
//!
//! A stack is chained base branches, and a change low in it leaves every branch above sitting on
//! the old version of the one below: a review fix added to the bottom, or the bottom squash-merged
//! into trunk, which leaves the next one carrying commits trunk already has in another form.
//! [`plan`] works out, from what GitHub has, which branches have to move and makes their new
//! commits with `git replay` — nothing is checked out, and a conflict anywhere changes nothing —
//! so the preview shows exactly what [`apply`] pushes. Applying is one atomic push, each branch
//! leased on what the plan read, then the local branches.
//!
//! Front-end → `Action::PlanRestack` → engine → [`plan`] → `Event::RestackPlanned` → preview →
//! `Action::Restack` naming the plan's [`RestackId`] → [`apply`] and the retargets →
//! `Event::Restacked`. The engine keeps the plan in between, so what is pushed is what was shown.

use crate::program::{captured_command, resolve, run_bounded};
use local::Expected;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

mod local;

mod git;
mod replay;

use git::{count, git, is_ancestor, last_lines, rev, run};
use replay::replay;
#[cfg(test)]
use replay::{replay_run, Replayed};

/// How long a local git command may take.
const GIT_DEADLINE: Duration = Duration::from_secs(30);

/// How long fetching or pushing the stack may take.
const NETWORK_DEADLINE: Duration = Duration::from_secs(120);

/// Where the temporary branches `git replay` reports on live, one per layer, never left behind.
const SCRATCH_PREFIX: &str = "refs/heads/luxe-restack/";

/// One pull request in the stack, bottom first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layer {
    /// Number within its repository
    pub number: u64,
    /// Its branch
    pub head_ref: String,
    /// The branch it targets
    pub base_ref: String,
}

/// Identifies one restack plan, which is what a push confirms.
///
/// The engine keeps the plan it made and pushes it by this id, so a front-end confirms what it was
/// shown without handing a plan back. A plan carries leases and local checkout paths, which is not
/// something to take on a client's word.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RestackId(String);

impl RestackId {
    /// Creates a distinct identifier, so a push of a superseded plan is recognized and refused.
    pub fn new() -> Self {
        Self(format!("restack:{}", uuid::Uuid::new_v4()))
    }

    /// Returns the stable serialized identifier.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for RestackId {
    fn default() -> Self {
        Self::new()
    }
}

/// The pull request that was merged out from under the bottom of a stack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergedBelow {
    /// Number within its repository
    pub number: u64,
    /// Its head when it merged, which the bottom branch still carries
    pub head_oid: String,
    /// The branch it merged into, which the bottom now sits on
    pub base_ref: String,
}

/// What restacking does to one pull request's branch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestackStep {
    /// Number within its repository
    pub number: u64,
    /// Its branch
    pub head_ref: String,
    /// The branch's tip on GitHub now, which the push is leased on
    pub old: String,
    /// Its tip after restacking; the same as `old` when only its base changes
    pub new: String,
    /// Commits of its own that were replayed
    pub commits: usize,
    /// The branch it now sits on
    pub onto: String,
    /// The base its pull request moves to, when it has to
    pub retarget: Option<String>,
    /// Where it is checked out here, which moves with it
    pub checked_out: Option<PathBuf>,
    /// Whether a local branch by its name moves with it
    pub local: bool,
}

/// Everything a restack does, worked out before any of it happens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestackPlan {
    /// The remote the stack is fetched from and pushed to
    pub remote: String,
    /// The branches that change, bottom first; empty when the stack already sits right
    pub steps: Vec<RestackStep>,
}

/// A plan, and what it saw of each branch here, which [`apply`] checks again before it pushes
/// and before it moves anything here.
///
/// Kept beside the published [`RestackPlan`] rather than in it: the local tips and checkouts are
/// for this process to check, not something a client is shown or hands back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    /// What the preview shows and the push carries
    pub plan: RestackPlan,
    expected: Vec<Expected>,
}

impl std::ops::Deref for Planned {
    type Target = RestackPlan;

    fn deref(&self) -> &RestackPlan {
        &self.plan
    }
}

/// How applying a plan stopped short.
#[derive(Debug, thiserror::Error)]
pub enum ApplyError {
    /// Refused before anything was pushed: nothing anywhere changed.
    #[error(transparent)]
    Refused(RestackError),
    /// The push did not finish. An atomic push lands whole or not at all, but a push that timed
    /// out cannot say which, so nothing is claimed.
    #[error(transparent)]
    PushFailed(RestackError),
    /// The branches were pushed; moving the ones here to match did not happen.
    #[error(transparent)]
    AfterPush(RestackError),
}

/// Why a stack could not be restacked, or a step of it could not be done.
#[derive(Debug, thiserror::Error)]
pub enum RestackError {
    /// Git refused, in its own words, bounded.
    #[error("git could not {operation}: {detail}")]
    Git {
        /// What was being done
        operation: &'static str,
        /// What git said
        detail: String,
    },
    /// A branch's commits do not apply to the one below it any more.
    #[error("#{number}'s branch `{head_ref}` does not replay cleanly onto `{onto}`: {detail}")]
    Conflict {
        /// Its pull request
        number: u64,
        /// Its branch
        head_ref: String,
        /// Where it was being moved
        onto: String,
        /// What git said
        detail: String,
    },
    /// A local branch has commits GitHub does not, which restacking would leave behind.
    #[error("`{0}` has commits here that are not on GitHub: push them first, or reset it")]
    Unpushed(String),
    /// A branch here moved, or its checkout changed, since the plan was made.
    #[error("`{head_ref}` changed here since the plan ({what}): plan the restack again")]
    Changed {
        /// The branch
        head_ref: String,
        /// What changed
        what: &'static str,
    },
    /// A branch is checked out with changes not committed, which moving it would put at risk.
    #[error("`{head_ref}` is checked out at {path} with changes not committed: commit or stash them first")]
    Dirty {
        /// The branch
        head_ref: String,
        /// Where
        path: String,
    },
}

/// Works out what restacking `layers` does, from what `remote` has.
///
/// A branch moves when the one below it no longer sits where it branched from: the one below
/// gained commits, was rewritten, or is itself moving now. The bottom moves when `merged_below`
/// merged out from under it, onto the branch that merged it — and only its own commits go with it,
/// since trunk has the merged ones, squashed or not. Its new commits are made now, by `git replay`,
/// so a conflict is found before anything changes rather than halfway through.
pub async fn plan(
    project: &Path,
    remote: &str,
    layers: &[Layer],
    merged_below: Option<&MergedBelow>,
) -> Result<Planned, RestackError> {
    let mut refspecs: Vec<String> =
        layers.iter().map(|layer| tracking_refspec(remote, &layer.head_ref)).collect();
    if let Some(merged) = merged_below {
        refspecs.push(tracking_refspec(remote, &merged.base_ref));
    }
    let mut fetch = vec!["fetch", "--quiet", "--no-tags", remote];
    fetch.extend(refspecs.iter().map(String::as_str));
    run(project, &fetch, NETWORK_DEADLINE)
        .await
        .map_err(|detail| RestackError::Git { operation: "fetch the stack", detail })?;
    let listed = local::worktrees(project).await?;

    let (mut steps, mut expected) = (Vec::new(), Vec::new());
    // What the layer below was, and is after this plan
    let mut below: Option<(String, String)> = None;
    for (index, layer) in layers.iter().enumerate() {
        let tip = rev(project, &format!("refs/remotes/{remote}/{}", layer.head_ref)).await?;
        let (start, onto, onto_label, retarget) = match (index, &below, merged_below) {
            (0, _, Some(merged)) => {
                let onto =
                    rev(project, &format!("refs/remotes/{remote}/{}", merged.base_ref)).await?;
                // Merged with a merge commit, trunk has the very commits: nothing to replay
                let start = (!is_ancestor(project, &merged.head_oid, &onto).await)
                    .then(|| merged.head_oid.clone());
                let retarget = (layer.base_ref != merged.base_ref).then(|| merged.base_ref.clone());
                (start, onto, merged.base_ref.clone(), retarget)
            }
            (0, _, None) => (None, String::new(), layer.base_ref.clone(), None),
            (_, Some((was, now)), _) => {
                let start = if was != now || !is_ancestor(project, was, &tip).await {
                    Some(
                        branch_point(project, remote, &layers[index - 1].head_ref, was, &tip).await,
                    )
                } else {
                    None
                };
                (start, now.clone(), layers[index - 1].head_ref.clone(), None)
            }
            (_, None, _) => unreachable!("every layer after the first has one below it"),
        };
        let (new, commits) = match start {
            Some(start) => {
                let new = replay(project, layer, &start, &onto, &tip).await.map_err(|detail| {
                    RestackError::Conflict {
                        number: layer.number,
                        head_ref: layer.head_ref.clone(),
                        onto: onto_label.clone(),
                        detail,
                    }
                })?;
                (new, count(project, &start, &tip).await)
            }
            None => (tip.clone(), 0),
        };
        below = Some((tip.clone(), new.clone()));
        if new == tip && retarget.is_none() {
            continue;
        }
        let seen = local::local_state(project, &listed, &layer.head_ref, &tip).await?;
        let (local, checked_out) = (seen.local_tip.is_some(), seen.checked_out.clone());
        expected.push(seen);
        steps.push(RestackStep {
            number: layer.number,
            head_ref: layer.head_ref.clone(),
            old: tip,
            new,
            commits,
            onto: onto_label,
            retarget,
            checked_out,
            local,
        });
    }
    Ok(Planned { plan: RestackPlan { remote: remote.to_string(), steps }, expected })
}

/// Pushes what the plan made, all of it or none, each branch only if GitHub still has what the plan
/// read; then moves the local branches, and the checkouts of them, to match.
///
/// Before the push, and again before anything here moves, every moving branch here must be where
/// the plan saw it, checked out where it was, and clean: work done since the preview is refused,
/// never reset away. Retargeting a pull request's base is the caller's, once the push has landed.
pub async fn apply(project: &Path, planned: &Planned) -> Result<(), ApplyError> {
    let moving = local::moving(planned);
    if moving.is_empty() {
        return Ok(());
    }
    local::recheck(project, &moving).await.map_err(ApplyError::Refused)?;
    let mut args: Vec<String> =
        vec!["push".into(), "--quiet".into(), "--atomic".into(), planned.remote.clone()];
    for (step, _) in &moving {
        args.push(format!("--force-with-lease=refs/heads/{}:{}", step.head_ref, step.old));
    }
    for (step, _) in &moving {
        args.push(format!("{}:refs/heads/{}", step.new, step.head_ref));
    }
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    run(project, &args, NETWORK_DEADLINE).await.map_err(|detail| {
        ApplyError::PushFailed(RestackError::Git { operation: "push the stack", detail })
    })?;
    local::reconcile(project, &moving).await.map_err(ApplyError::AfterPush)
}

fn tracking_refspec(remote: &str, branch: &str) -> String {
    format!("+refs/heads/{branch}:refs/remotes/{remote}/{branch}")
}

/// Where a branch's own commits start, above the one below it that has since changed: the last
/// version of that one it contains — from the remote's reflog when git kept it — else their
/// merge base.
async fn branch_point(project: &Path, remote: &str, below: &str, was: &str, tip: &str) -> String {
    if is_ancestor(project, was, tip).await {
        return was.to_string();
    }
    let tracking = format!("refs/remotes/{remote}/{below}");
    if let Ok(point) = git(project, &["merge-base", "--fork-point", &tracking, tip]).await {
        return point;
    }
    git(project, &["merge-base", was, tip]).await.unwrap_or_else(|_| was.to_string())
}

#[cfg(test)]
mod tests;
