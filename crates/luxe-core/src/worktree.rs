//! Worktree — a session's own checkout, on a branch of its own.
//!
//! A session that asks for one works in `<project>/.luxe/worktrees/<name>/` instead of the
//! checkout you are standing in, on a branch a person would have made, so its edits never land in
//! your working tree and abandoning it is removing a directory. `.luxe/` ignores itself, so the
//! worktree is invisible to your `git status`, your editor's search and your tests.
//!
//! The CLI calls [`prepare`] before the engine starts — once, so nothing moves under a running
//! tool — and makes the result the process's working directory. Sessions, reviews and logs stay
//! in the project's `.luxe/`: removing a worktree never takes any of them with it.

mod push_route;

pub(crate) use push_route::PUSH_ROUTE_MARK;

use crate::program::{captured_command, resolve, run_bounded};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long a local git command may take.
const GIT_DEADLINE: Duration = Duration::from_secs(30);

/// How long fetching the base may take before the worktree is made from what is already here.
const FETCH_DEADLINE: Duration = Duration::from_secs(30);

/// How long fetching a pull request's head may take. Nothing can be made without it.
const HEAD_FETCH_DEADLINE: Duration = Duration::from_secs(120);

/// Where a project's session worktrees live, relative to it.
pub const WORKTREES_DIR: &str = ".luxe/worktrees";

/// A session's worktree, made or found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionWorktree {
    /// The checkout
    pub path: PathBuf,
    /// Its branch
    pub branch: String,
    /// What a new branch was made from, like `origin/main`; `None` for a branch that existed
    pub base: Option<String>,
    /// Whether it was made just now, rather than found from an earlier session
    pub created: bool,
    /// Whether the base could not be fetched, so a new branch starts from what was already here
    pub fetch_failed: bool,
}

/// Why a worktree could not be made.
#[derive(Debug, thiserror::Error)]
pub enum WorktreeError {
    /// The project is not a git repository, so there is nothing to branch.
    #[error("this project is not a git repository, so it cannot have a worktree")]
    NotARepository,
    /// The branch name is one git will not accept.
    #[error("`{0}` is not a branch name git accepts")]
    InvalidBranch(String),
    /// The branch is already checked out somewhere, which git allows only once.
    #[error("branch `{branch}` is already checked out at {path}")]
    CheckedOutElsewhere {
        /// The branch
        branch: String,
        /// Where
        path: String,
    },
    /// Git refused, in its own words, bounded.
    #[error("git could not {operation}: {detail}")]
    Git {
        /// What was being done
        operation: &'static str,
        /// What git said
        detail: String,
    },
    /// Git could not be run at all.
    #[error("could not run git: {0}")]
    Run(String),
    /// A local branch by the name a pull request needs holds other work.
    #[error(
        "a branch named `{0}` already exists here and is not this pull request's: rename it, and \
         try again"
    )]
    BranchTaken(String),
    /// Nothing says how to reach the fork the pull request comes from.
    #[error("no remote here says how to reach the fork, and none could be made from `{0}`")]
    NoForkRemote(String),
}

/// A pull request to check out, and where its head lives.
#[derive(Debug, Clone)]
pub struct PullRequestCheckout<'a> {
    /// Where the head lives and the local branch it gets
    pub head: &'a crate::github::PullRequestHead,
    /// The host both repositories are on, which matches remotes to them
    pub host: &'a str,
    /// The remote this checkout reaches its own repository by, usually `origin`
    pub base_remote: &'a str,
}

/// A pull request's worktree, and how a push from it reaches the pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequestWorktree {
    /// The worktree, on the pull request's local branch
    pub worktree: SessionWorktree,
    /// The remote its head is fetched from and pushed to
    pub remote: String,
    /// Whether that remote was added for it, rather than found
    pub remote_added: bool,
}

/// The directory a branch's worktree lives in: its name, with the slashes a branch may carry
/// flattened so one branch is one directory.
pub fn worktree_path(project: &Path, branch: &str) -> PathBuf {
    project.join(WORKTREES_DIR).join(branch.replace(['/', '\\'], "-"))
}

/// The project a Luxe-made worktree belongs to, when `dir` is one or is inside one: what
/// `<project>/.luxe/worktrees/<name>` leaves behind. `None` anywhere else.
///
/// What a session in a worktree needs to reach what lives beside the project's `.luxe/` and never
/// in a checkout, since `.luxe/` ignores itself: the project's own skills, for one.
pub fn project_of(dir: &Path) -> Option<PathBuf> {
    let parts: Vec<&std::ffi::OsStr> = dir.iter().collect();
    let at = parts.windows(2).rposition(|pair| pair[0] == ".luxe" && pair[1] == "worktrees")?;
    // A worktree's name follows, and the project is everything before `.luxe`
    (at + 2 < parts.len() && at > 0).then(|| parts[..at].iter().collect())
}

/// Makes the session's worktree for `branch`, or finds the one an earlier session made.
///
/// A branch that exists is checked out as it is. A new one starts from `base` when given, else
/// from the remote's default branch after a bounded fetch — so work starts from what is current —
/// and failing that from `HEAD`. Offline, the fetch fails and the worktree is still made, from what
/// is already here, and says so.
pub async fn prepare(
    project: &Path,
    branch: &str,
    base: Option<&str>,
) -> Result<SessionWorktree, WorktreeError> {
    if git(project, &["rev-parse", "--git-dir"]).await.is_err() {
        return Err(WorktreeError::NotARepository);
    }
    if git(project, &["check-ref-format", "--branch", branch]).await.is_err() {
        return Err(WorktreeError::InvalidBranch(branch.to_string()));
    }
    let path = worktree_path(project, branch);
    // A worktree directory deleted by hand leaves git thinking it still exists
    let _ = git(project, &["worktree", "prune"]).await;
    let listed = git(project, &["worktree", "list", "--porcelain"]).await.unwrap_or_default();
    match checked_out_at(&listed, branch) {
        Some(at) if same_path(&at, &path) => {
            return Ok(SessionWorktree {
                path,
                branch: branch.to_string(),
                base: None,
                created: false,
                fetch_failed: false,
            });
        }
        Some(at) => {
            return Err(WorktreeError::CheckedOutElsewhere {
                branch: branch.to_string(),
                path: crate::paths::display(&at),
            })
        }
        None => {}
    }
    let path_arg = crate::paths::display(&path);
    let exists =
        git(project, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")])
            .await
            .is_ok();
    if exists {
        git(project, &["worktree", "add", &path_arg, branch])
            .await
            .map_err(|detail| WorktreeError::Git { operation: "add the worktree", detail })?;
        return Ok(SessionWorktree {
            path,
            branch: branch.to_string(),
            base: None,
            created: true,
            fetch_failed: false,
        });
    }
    let (base, fetch_failed) = match base {
        Some(base) => (base.to_string(), false),
        None => default_base(project).await,
    };
    git(project, &["worktree", "add", "--no-track", "-b", branch, &path_arg, &base])
        .await
        .map_err(|detail| WorktreeError::Git { operation: "add the worktree", detail })?;
    // So a plain `git push` from the worktree publishes the branch under its own name, and the
    // pull request watch can tell where it went
    if has_remote(project, "origin").await {
        let _ = git(&path, &["config", &format!("branch.{branch}.remote"), "origin"]).await;
        let merge = format!("refs/heads/{branch}");
        let _ = git(&path, &["config", &format!("branch.{branch}.merge"), &merge]).await;
    }
    Ok(SessionWorktree {
        path,
        branch: branch.to_string(),
        base: Some(base),
        created: true,
        fetch_failed,
    })
}

/// Checks a pull request out in a worktree of its own, on a branch that tracks its head, so a plain
/// `git push` lands on the pull request.
///
/// A fork is reached by a remote pointing at it, found or added in the style this checkout reaches
/// its own repository; `push_route::route_push` says where a push from the branch goes. A branch by
/// that name holding other work is refused rather than moved, and a fetch that fails leaves nothing
/// it added behind; a push route that cannot be written is an error, with the worktree in place.
pub async fn check_out_pull_request(
    project: &Path,
    checkout: &PullRequestCheckout<'_>,
) -> Result<PullRequestWorktree, WorktreeError> {
    let head = checkout.head;
    let branch = head.local_branch.as_str();
    if git(project, &["rev-parse", "--git-dir"]).await.is_err() {
        return Err(WorktreeError::NotARepository);
    }
    if git(project, &["check-ref-format", "--branch", branch]).await.is_err() {
        return Err(WorktreeError::InvalidBranch(branch.to_string()));
    }
    let path = worktree_path(project, branch);
    let _ = git(project, &["worktree", "prune"]).await;
    let listed = git(project, &["worktree", "list", "--porcelain"]).await.unwrap_or_default();
    if let Some(at) = checked_out_at(&listed, branch) {
        if !same_path(&at, &path) {
            return Err(WorktreeError::CheckedOutElsewhere {
                branch: branch.to_string(),
                path: crate::paths::display(&at),
            });
        }
    }
    let (remote, remote_added) = head_remote(project, checkout).await?;
    let tracking = format!("{remote}/{}", head.head_ref);
    let refspec = format!("+refs/heads/{}:refs/remotes/{tracking}", head.head_ref);
    if let Err(detail) =
        run(project, &["fetch", "--quiet", "--no-tags", &remote, &refspec], HEAD_FETCH_DEADLINE)
            .await
    {
        if remote_added {
            let _ = git(project, &["remote", "remove", &remote]).await;
        }
        return Err(WorktreeError::Git { operation: "fetch the pull request", detail });
    }
    let found = checked_out_at(&listed, branch).is_some();
    let exists =
        git(project, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")])
            .await
            .is_ok();
    let path_arg = crate::paths::display(&path);
    if exists {
        // Behind the head, it catches up; ahead of it, it keeps the commits not pushed yet
        let behind = is_ancestor(project, branch, &tracking).await;
        if !behind && !is_ancestor(project, &tracking, branch).await {
            return Err(WorktreeError::BranchTaken(branch.to_string()));
        }
        if !found {
            git(project, &["worktree", "add", &path_arg, branch])
                .await
                .map_err(|detail| WorktreeError::Git { operation: "add the worktree", detail })?;
        }
        if behind {
            git(&path, &["merge", "--quiet", "--ff-only", &tracking])
                .await
                .map_err(|detail| WorktreeError::Git { operation: "catch up with it", detail })?;
        }
    } else {
        git(project, &["worktree", "add", "--no-track", "-b", branch, &path_arg, &tracking])
            .await
            .map_err(|detail| WorktreeError::Git { operation: "add the worktree", detail })?;
    }
    push_route::route_push(project, &remote, branch, &head.head_ref).await?;
    Ok(PullRequestWorktree {
        worktree: SessionWorktree {
            path,
            branch: branch.to_string(),
            base: Some(tracking),
            created: !found,
            fetch_failed: false,
        },
        remote,
        remote_added,
    })
}

/// The remote a pull request's head is reached by: this repository's own for a branch in it; for
/// a fork, the remote pointing at it, else one added under the fork owner's name.
async fn head_remote(
    project: &Path,
    checkout: &PullRequestCheckout<'_>,
) -> Result<(String, bool), WorktreeError> {
    let head = checkout.head;
    if !head.cross_repository {
        return Ok((checkout.base_remote.to_string(), false));
    }
    if let Some(found) =
        crate::github::remote_pointing_at(project, checkout.host, &head.owner, &head.name).await
    {
        return Ok((found, false));
    }
    // As configured, not as `insteadOf` rewrites it: the rewrite applies again to the fork's
    let key = format!("remote.{}.url", checkout.base_remote);
    let own = git(project, &["config", "--get", &key]).await.unwrap_or_default();
    let url = crate::github::sibling_url(&own, &head.owner, &head.name)
        .ok_or_else(|| WorktreeError::NoForkRemote(checkout.base_remote.to_string()))?;
    // One this made before, found by the URL it was given, which `insteadOf` may display otherwise
    let configured = git(project, &["config", "--get-regexp", r"^remote\..*\.url$"]).await;
    let made_before = configured.unwrap_or_default().lines().find_map(|line| {
        let (key, value) = line.split_once(' ')?;
        (value == url)
            .then(|| key.strip_prefix("remote.")?.strip_suffix(".url").map(str::to_string))?
    });
    if let Some(name) = made_before {
        return Ok((name, false));
    }
    let taken = git(project, &["remote"]).await.unwrap_or_default();
    let taken: Vec<&str> = taken.lines().collect();
    let name = std::iter::once(head.owner.clone())
        .chain((2..).map(|n| format!("{}-{n}", head.owner)))
        .find(|name| !taken.contains(&name.as_str()))
        .expect("an unbounded sequence has a free name");
    git(project, &["remote", "add", &name, &url])
        .await
        .map_err(|detail| WorktreeError::Git { operation: "add a remote for the fork", detail })?;
    Ok((name, true))
}

async fn is_ancestor(project: &Path, ancestor: &str, of: &str) -> bool {
    git(project, &["merge-base", "--is-ancestor", ancestor, of]).await.is_ok()
}

/// Removes a session's worktree, keeping its branch.
///
/// Never forced: git itself refuses a worktree with changes not committed, which is the last word
/// on whether anything would be lost. Run it from the project, not from inside the worktree.
pub async fn remove(project: &Path, path: &Path) -> Result<(), WorktreeError> {
    let path = crate::paths::display(path);
    git(project, &["worktree", "remove", &path])
        .await
        .map(|_| ())
        .map_err(|detail| WorktreeError::Git { operation: "remove the worktree", detail })
}

/// The remote's default branch after a bounded fetch, else `HEAD`; and whether the fetch failed.
async fn default_base(project: &Path) -> (String, bool) {
    let Ok(symbolic) = git(project, &["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"]).await
    else {
        return ("HEAD".to_string(), false);
    };
    let Some(default) = symbolic.strip_prefix("refs/remotes/origin/").map(str::to_string) else {
        return ("HEAD".to_string(), false);
    };
    let fetched = run(
        project,
        &["fetch", "--quiet", "--no-write-fetch-head", "origin", &default],
        FETCH_DEADLINE,
    )
    .await
    .is_ok();
    (format!("origin/{default}"), !fetched)
}

async fn has_remote(project: &Path, name: &str) -> bool {
    git(project, &["remote"])
        .await
        .is_ok_and(|remotes| remotes.lines().any(|remote| remote == name))
}

/// Where `branch` is checked out, from `git worktree list --porcelain`.
fn checked_out_at(listing: &str, branch: &str) -> Option<PathBuf> {
    let wanted = format!("branch refs/heads/{branch}");
    let mut current = None;
    for line in listing.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            current = Some(PathBuf::from(path));
        } else if line == wanted {
            return current;
        }
    }
    None
}

/// Whether two paths name the same directory, however each was spelled.
fn same_path(left: &Path, right: &Path) -> bool {
    match (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

async fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    run(dir, args, GIT_DEADLINE).await
}

/// Git's stdout in `dir`, or the first lines of what it said when it failed.
async fn run(dir: &Path, args: &[&str], deadline: Duration) -> Result<String, String> {
    let program = resolve("git").unwrap_or_else(|| PathBuf::from("git"));
    let mut command = captured_command(program, args, dir);
    command.env("GIT_TERMINAL_PROMPT", "0").env("GCM_INTERACTIVE", "never");
    let output = run_bounded(command, deadline).await.map_err(|error| error.to_string())?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
    }
    let said = String::from_utf8_lossy(&output.stderr);
    let lines: Vec<&str> =
        said.lines().map(str::trim).filter(|line| !line.is_empty()).take(3).collect();
    Err(if lines.is_empty() { "no reason given".into() } else { lines.join(" ") })
}

#[cfg(test)]
mod tests;
