//! Command — every Git command a review runs, bounded and reaped.
//!
//! A snapshot `git add` runs the project's clean filters and a commit can wait on a signer, so a
//! review's Git commands are found the way every other program is and run through
//! [`program::run_bounded`]: a deadline, and their whole tree killed when it passes or the capture
//! waiting on them is dropped.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::GuidedReviewError;
use crate::program::{self, BoundedRunError};

/// How long one review Git command may take. Generous: a snapshot `add` cleans a whole tree.
pub(super) const GIT_DEADLINE: Duration = Duration::from_secs(120);

/// Runs `git args` in `repo` with `env` set, held to `deadline`.
///
/// Returns the output whatever its status, since which statuses count is the caller's: exit 1 is
/// an answer from `diff --quiet` and a failure from nearly anything else.
pub(super) async fn run_git_output(
    repo: &Path,
    operation: &'static str,
    args: &[&OsStr],
    env: &[(&str, &OsStr)],
    deadline: Duration,
) -> Result<std::process::Output, GuidedReviewError> {
    run_git_with_input(repo, operation, args, env, &[], deadline).await
}

/// [`run_git_output`] with `input` on Git's stdin, the write held to the same deadline.
pub(super) async fn run_git_with_input(
    repo: &Path,
    operation: &'static str,
    args: &[&OsStr],
    env: &[(&str, &OsStr)],
    input: &[u8],
    deadline: Duration,
) -> Result<std::process::Output, GuidedReviewError> {
    let git = program::resolve("git").unwrap_or_else(|| PathBuf::from("git"));
    let mut command = program::captured_command(&git, &[], repo);
    command.args(args).envs(env.iter().copied());
    let failed = |source| GuidedReviewError::GitSpawn {
        operation,
        repo_path: repo.to_owned(),
        error: source,
    };
    program::run_bounded_with_input(command, input, deadline).await.map_err(|error| match error {
        BoundedRunError::Spawn(source) | BoundedRunError::Wait(source) => failed(source),
        BoundedRunError::Timeout { after } => GuidedReviewError::GitTimeout { operation, after },
    })
}

/// `arguments` as the `OsStr`s [`run_git_output`] takes.
pub(super) fn os_args<'a>(arguments: &[&'a str]) -> Vec<&'a OsStr> {
    arguments.iter().map(|argument| OsStr::new(*argument)).collect()
}
