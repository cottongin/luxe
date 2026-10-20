//! `git` `checks` — why CI is failing, bounded.
//!
//! `gh run view --log-failed` prints every line of every failing step: 3,671 lines for one Clippy
//! step of luxe's own, with the error in the last dozen. This reads the same logs through GitHub
//! and keeps the tail of each failing step, says how much it left out, and also lists the failing
//! checks that are not Actions jobs at all — a commit status from another CI, which that command
//! cannot see. Reading only; re-running a job stays behind the permission engine's `ask`.

use luxe_core::github::{
    Branches, CheckLog, Checks, Discovery, FailingCheck, GitHubError, PullRequests,
};
use std::path::Path;

/// Lines kept from the end of each failing step's log.
pub(super) const LOG_TAIL: usize = 30;

/// Longest the whole answer may run, in lines, whatever the number of failures.
pub(super) const MAX_LINES: usize = 200;

/// The commit whose checks were read, and how it was chosen.
struct Target {
    /// Tip of what was asked about
    oid: String,
    /// What to call it: the pull request, or the commit when there is none
    name: String,
    /// Checks still running, when the pull request said
    pending: Option<usize>,
}

/// Why checks are failing on the branch's pull request, on pull request `#n` when `args` names one,
/// or on `HEAD` when the branch has no pull request.
pub(super) async fn checks(
    github: &(impl Discovery + PullRequests + Branches + Checks + ?Sized),
    cwd: &Path,
    args: &[String],
) -> Result<String, String> {
    let repository = github.repository(cwd).await.map_err(|error| failure(&error))?;
    let target = match args.first().map(|arg| arg.trim_start_matches('#')) {
        Some(number) => {
            let number: u64 =
                number.parse().map_err(|_| format!("`{number}` is not a pull request number."))?;
            let pr = github.pull_request(&repository, number).await.map_err(|e| failure(&e))?;
            Target {
                oid: pr.head_oid,
                name: format!("#{} · {}", pr.number, luxe_core::github::untrusted(&pr.title, 120)),
                pending: Some(pr.checks.pending),
            }
        }
        None => branch_target(github, cwd, &repository).await?,
    };
    let failing = github
        .failing_checks(&repository, &target.oid, LOG_TAIL)
        .await
        .map_err(|error| failure(&error))?;
    Ok(render(&target, &failing))
}

/// The branch's pull request head, or `HEAD` when there is no pull request to ask about.
async fn branch_target(
    github: &(impl Branches + ?Sized),
    cwd: &Path,
    repository: &luxe_core::github::Repository,
) -> Result<Target, String> {
    let head = luxe_core::github::branch_head(cwd).await;
    if let Some(head) = head {
        let status = github.branch_status(repository, &head).await.map_err(|e| failure(&e))?;
        if let Some(pr) = status.pull_request {
            return Ok(Target {
                oid: pr.head_oid,
                name: format!("#{} · {}", pr.number, pr.title),
                pending: Some(pr.checks.pending),
            });
        }
    }
    let oid = head_commit(cwd).await.ok_or("This checkout has no commit to read checks for.")?;
    let name = format!("{} (this branch has no pull request)", short(&oid));
    Ok(Target { oid, name, pending: None })
}

async fn head_commit(cwd: &Path) -> Option<String> {
    let program =
        luxe_core::program::resolve("git").unwrap_or_else(|| std::path::PathBuf::from("git"));
    let command = luxe_core::program::captured_command(program, &["rev-parse", "HEAD"], cwd);
    let output =
        luxe_core::program::run_bounded(command, std::time::Duration::from_secs(10)).await.ok()?;
    let oid = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (output.status.success() && !oid.is_empty()).then_some(oid)
}

/// The answer: each failing check, where to read it, and the end of each failing step.
fn render(target: &Target, failing: &[FailingCheck]) -> String {
    let running = match target.pending {
        Some(0) | None => String::new(),
        Some(1) => " 1 check is still running.".to_string(),
        Some(count) => format!(" {count} checks are still running."),
    };
    if failing.is_empty() {
        return format!("No checks are failing on {}.{running}", target.name);
    }
    let count = match failing.len() {
        1 => "1 check".to_string(),
        count => format!("{count} checks"),
    };
    let mut lines =
        vec![format!("{count} failing on {} ({}).{running}", target.name, short(&target.oid))];
    for check in failing {
        lines.push(String::new());
        let summary = check.summary.as_deref().map(|text| format!(": {text}")).unwrap_or_default();
        lines.push(format!("{} — {}{summary}", check.name, check.outcome));
        if !check.url.is_empty() {
            lines.push(format!("  {}", check.url));
        }
        match &check.log {
            CheckLog::NotActions => {
                lines.push(
                    "  Not a GitHub Actions job, so there is no log here; the link has it.".into(),
                );
            }
            CheckLog::NotRead => lines.push(
                "  Log not read: only the first few failing jobs are. The link, or `gh run view \
                 --job <id> --log-failed`, has it."
                    .into(),
            ),
            CheckLog::Unreadable(reason) => lines.push(format!("  Log unreadable: {reason}")),
            CheckLog::Steps(steps) if steps.is_empty() => {
                lines.push("  No failing step printed a log.".into());
            }
            CheckLog::Steps(steps) => {
                for step in steps {
                    let name = match step.name.as_str() {
                        "UNKNOWN STEP" => "the job's log (gh could not split it by step)".into(),
                        name => format!("step \"{name}\""),
                    };
                    let shown = match (step.omitted, step.after) {
                        (0, 0) => format!("  {name}:"),
                        (omitted, 0) => format!(
                            "  {name}, last {} of {} lines:",
                            step.tail.len(),
                            step.tail.len() + omitted
                        ),
                        (omitted, after) => format!(
                            "  {name}, the {} up to its last error, of {}:",
                            match step.tail.len() {
                                1 => "line".to_string(),
                                count => format!("{count} lines"),
                            },
                            step.tail.len() + omitted + after
                        ),
                    };
                    lines.push(shown);
                    lines.extend(step.tail.iter().map(|line| format!("    {line}")));
                }
            }
        }
    }
    crate::output_limit::cap_lines(
        &lines.join("\n"),
        MAX_LINES,
        "lines of check output",
        "ask about one pull request at a time, or open a check's link",
    )
}

/// What went wrong, as the sentence the model acts on.
fn failure(error: &GitHubError) -> String {
    match error {
        GitHubError::NotInstalled => {
            "The GitHub CLI (`gh`) is not installed. Install it and run `gh auth login`.".into()
        }
        GitHubError::NotAuthenticated { .. } => {
            "The GitHub CLI is not signed in. The user needs to run `gh auth login`.".into()
        }
        GitHubError::NotGitHub => "This repository has no remote on GitHub.".into(),
        GitHubError::NotFound => {
            "GitHub has no such pull request or commit that this account can see; \
                                 a commit that was never pushed has no checks."
                .into()
        }
        GitHubError::RateLimited => {
            "GitHub's rate limit for this account is spent; it refills within the hour.".into()
        }
        other => format!("Could not read the checks: {other}."),
    }
}

fn short(oid: &str) -> &str {
    oid.get(..7).unwrap_or(oid)
}

#[cfg(test)]
mod tests;
