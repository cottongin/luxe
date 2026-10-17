//! Running git and `gh` — bounded, with the tree reaped on a timeout and a failure explained.

use std::path::Path;

use anyhow::Result;

use super::failure::{explain_failure, GH_ERROR_MAX_LINES};
use super::GitTool;

/// How long a `gh` invocation may take before it is reaped.
///
/// Every `gh` subcommand is a network call, so this is not a guess about speed — it is the bound
/// that stops a hung connection from stalling the turn with nothing to cancel.
const GH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// How long a `git` invocation may take before its process tree is reaped.
///
/// Deliberately under the engine's own deadline so this fires first, because only this one can kill
/// the *tree*: the engine bounds a call by dropping the future, which reaps `git` and leaves the
/// `gpg` pinentry or pre-commit hook it started running — holding `.git/index.lock` against every
/// call that follows. Generous enough for a hook that runs a linter; the engine remains the backstop
/// under any configuration that shortens its own bound.
const GIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(90);

impl GitTool {
    /// Run a git command and return its stdout.
    pub(super) async fn run_git(&self, cwd: &Path, args: &[&str]) -> Result<String> {
        let mut cmd = luxe_core::program::captured_command("git", args, cwd);
        // Git opens `$EDITOR` for a commit it has no message for, and that editor would inherit a
        // terminal the TUI owns while waiting for a human who cannot see it. Failing costs nothing:
        // every commit reaching here already carries a message or names where git should find one,
        // and `explain_failure` turns git's reply into the two arguments that would have worked.
        cmd.env("GIT_EDITOR", "false");

        let output =
            run_bounded(cmd, "git", GIT_TIMEOUT, "a commit hook or a signing passphrase").await?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            Err(anyhow::anyhow!(
                "{}",
                explain_failure(
                    &String::from_utf8_lossy(&output.stderr),
                    &String::from_utf8_lossy(&output.stdout),
                    cwd
                )
            ))
        }
    }

    /// Make the branch a pull request can actually be opened from, or say why it cannot be.
    ///
    /// `gh pr create` has one precondition it does not fix for you: the branch has to exist on the
    /// remote. Otherwise the call fails with a message about a head branch, and the model spends
    /// two round trips on `git push -u` and a retry, for a step that has exactly one correct
    /// answer.
    ///
    /// Returns a line to prepend to the result when something was done, so a push is never silent.
    /// The push is plain — `--set-upstream`, never `--force` — because publishing a branch that
    /// does not exist yet cannot destroy anything, and rewriting one that does is a different
    /// operation with a different permission story.
    pub(super) async fn ready_branch_for_pr(&self, cwd: &Path) -> Result<Option<String>> {
        let branch = self.run_git(cwd, &["rev-parse", "--abbrev-ref", "HEAD"]).await?;
        let branch = branch.trim().to_string();
        if branch.is_empty() || branch == "HEAD" {
            anyhow::bail!(
                "HEAD is detached, so there is no branch to open a pull request from — \
                 `git checkout -b <name>` first"
            );
        }
        // A pull request from trunk to trunk is not a thing, and the error `gh` gives for it sends
        // people looking at their remote rather than at their branch.
        if matches!(branch.as_str(), "main" | "master") {
            anyhow::bail!(
                "you are on `{branch}`, which is what a pull request would target — put the work \
                 on its own branch first (`git checkout -b <name>`)"
            );
        }
        // An upstream already set means the branch is published; `gh` takes it from here.
        if self.run_git(cwd, &["rev-parse", "--abbrev-ref", "@{u}"]).await.is_ok() {
            return Ok(None);
        }
        self.run_git(cwd, &["push", "--set-upstream", "origin", &branch])
            .await
            .map_err(|error| anyhow::anyhow!("could not publish `{branch}`: {error}"))?;
        Ok(Some(format!("Pushed `{branch}` to origin (it had no upstream), then:")))
    }

    /// Run a `gh` command and return its stdout.
    ///
    /// Resolved rather than assumed: `gh` installed by scoop, winget or Homebrew is frequently
    /// absent from the `PATH` an app inherits.
    pub(super) async fn run_gh(&self, cwd: &Path, args: &[&str]) -> Result<String> {
        let program = luxe_core::program::resolve("gh").ok_or_else(|| {
            anyhow::anyhow!(
                "the GitHub CLI (`gh`) is not installed \u{2014} see https://cli.github.com, then run \
                 `gh auth login`"
            )
        })?;
        let cmd = luxe_core::program::captured_command(program, args, cwd);
        let output = run_bounded(cmd, "gh", GH_TIMEOUT, "network access").await?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            // Same trap as `git`: a failure whose only words went to stdout would otherwise come
            // back as a bare "gh command failed:" with the reason cut off.
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let reported = match stderr.trim() {
                "" => stdout.trim(),
                text => text,
            };
            Err(anyhow::anyhow!(
                "gh command failed{}",
                match reported {
                    "" => " (no output).".to_string(),
                    text => format!(
                        ": {}",
                        crate::output_limit::cap_lines(
                            text,
                            GH_ERROR_MAX_LINES,
                            "lines of gh output",
                            "Run the command directly to see the rest.",
                        )
                    ),
                }
            ))
        }
    }
}

/// Run `cmd` to completion under `deadline`, killing its whole tree if it expires.
///
/// `waiting_for` names the usual culprit, so a timeout is a lead rather than a shrug.
async fn run_bounded(
    cmd: tokio::process::Command,
    program: &str,
    deadline: std::time::Duration,
    waiting_for: &str,
) -> Result<std::process::Output> {
    use luxe_core::program::BoundedRunError;
    luxe_core::program::run_bounded(cmd, deadline).await.map_err(|error| match error {
        BoundedRunError::Timeout { after } => anyhow::anyhow!(
            "{program} timed out after {}s \u{2014} {waiting_for} is the usual cause; run the \
             command yourself to see what it is waiting for",
            after.as_secs()
        ),
        BoundedRunError::Spawn(source) | BoundedRunError::Wait(source) => {
            anyhow::Error::new(source).context(format!("Failed to execute {program} command"))
        }
    })
}
