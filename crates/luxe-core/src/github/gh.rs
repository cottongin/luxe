//! GhCli — [`GitHub`](super::GitHub) over the user's own `gh`.
//!
//! Every call is a bounded, non-interactive subprocess ([`crate::program::run_bounded`]): stdin
//! closed, prompts disabled, output captured, the tree killed at the deadline. `gh` is resolved
//! through [`crate::program::resolve`], so a copy installed by scoop, winget or Homebrew outside
//! the inherited `PATH` is still found.

mod branches;
mod checks;
mod conversation;
mod discovery;
mod issue_overview;
mod overview;
mod pull_requests;
mod reviews;
mod search;
mod threads;
mod writes;

use search::parse_search;
#[cfg(test)]
use {
    branches::*, checks::*, discovery::*, pull_requests::*, reviews::*, threads::*,
    writes::parse_opened,
};

use super::remote::{matching_remote, parse_remote_url, parse_remotes};
use super::{
    Account, Author, BranchHead, BranchStatus, Branches, CheckLog, CheckRollup, Checks,
    CommentableLines, ConversationComment, Discovery, ExistingReview, FailedStep, FailingCheck,
    FilePatch, GitHubError, Issue, IssueComment, IssueRef, NewPullRequest, OpenedPullRequest,
    PostedReview, PullRequest, PullRequestHead, PullRequestState, PullRequestStatus, PullRequests,
    PushAccess, RateLimit, Repository, ReviewDecision, ReviewEvent, ReviewThread, ReviewThreads,
    Reviews, SearchPage, Stack, StackEntry, SubmitReview, ThreadComment, ThreadSide, Threads,
    Writes,
};
use crate::program::{captured_command, run_bounded, BoundedRunError};
use async_trait::async_trait;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long a `gh` call may take before it is reaped. Every one is a network call.
const GH_DEADLINE: Duration = Duration::from_secs(60);

/// How long `git remote -v` may take. It reads a config file; anything slower is stuck.
const GIT_DEADLINE: Duration = Duration::from_secs(10);

/// What one `gh` run printed, and whether it succeeded.
struct GhRun {
    success: bool,
    stdout: String,
    stderr: String,
}

/// [`GitHub`](super::GitHub) backed by the `gh` command-line tool.
#[derive(Debug, Clone)]
pub struct GhCli {
    deadline: Duration,
    /// A specific `gh` to run instead of resolving one, for tests
    program: Option<PathBuf>,
}

impl Default for GhCli {
    fn default() -> Self {
        Self { deadline: GH_DEADLINE, program: None }
    }
}

impl GhCli {
    /// A client that runs `gh` with the default deadline.
    pub fn new() -> Self {
        Self::default()
    }

    /// Run `gh` in `cwd` and return its stdout, or the failure classified.
    async fn gh(&self, cwd: &Path, args: &[&str]) -> Result<String, GitHubError> {
        let ran = self.run(cwd, args).await?;
        if ran.success {
            Ok(ran.stdout)
        } else {
            Err(classify_failure(&ran.stderr, &ran.stdout))
        }
    }

    /// Run `gh` in `cwd` and hand back both streams whatever the exit status.
    ///
    /// For the one call whose failure is worth reading closely: a refused post answers with JSON
    /// on stdout naming the reasons, and only `HTTP 422` on stderr.
    async fn run(&self, cwd: &Path, args: &[&str]) -> Result<GhRun, GitHubError> {
        let program = match &self.program {
            Some(program) => program.clone(),
            None => crate::program::resolve("gh").ok_or(GitHubError::NotInstalled)?,
        };
        let mut command = captured_command(&program, args, cwd);
        // No prompt could be answered from behind the TUI, and an update notice on stderr would
        // be read as part of the error
        command
            .env("GH_PROMPT_DISABLED", "1")
            .env("GH_NO_UPDATE_NOTIFIER", "1")
            .env("NO_COLOR", "1");
        let output = run_bounded(command, self.deadline).await.map_err(|error| match error {
            BoundedRunError::Timeout { after } => GitHubError::Timeout { after },
            BoundedRunError::Spawn(source) | BoundedRunError::Wait(source) => {
                GitHubError::Run { program: "gh", source }
            }
        })?;
        Ok(GhRun {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    async fn remotes(cwd: &Path) -> Vec<(String, String)> {
        local_remotes(cwd).await
    }
}

/// `git remote -v` in `cwd`, or an empty list when it cannot be read.
///
/// A checkout whose remotes cannot be listed still has a repository `gh` could name; it just has no
/// remote to fetch it through, which [`Repository::remote`] says with `None`.
pub(super) async fn local_remotes(cwd: &Path) -> Vec<(String, String)> {
    let program = crate::program::resolve("git").unwrap_or_else(|| PathBuf::from("git"));
    let command = captured_command(program, &["remote", "-v"], cwd);
    match run_bounded(command, GIT_DEADLINE).await {
        Ok(output) if output.status.success() => {
            parse_remotes(&String::from_utf8_lossy(&output.stdout))
        }
        _ => Vec::new(),
    }
}

/// The REST path of a pull request, for `gh api`.
fn pulls_endpoint(repository: &Repository, number: u64) -> String {
    format!("repos/{}/{}/pulls/{number}", repository.owner, repository.name)
}

/// Points `gh api` at a GitHub Enterprise host; github.com needs nothing.
fn push_hostname<'a>(args: &mut Vec<&'a str>, repository: &'a Repository) {
    if repository.host != "github.com" {
        args.extend(["--hostname", repository.host.as_str()]);
    }
}

/// A file only its owner can read, removed when dropped.
///
/// `gh api --input` reads a review from a file, and a review is the reader's own words before
/// anyone else has seen them: kept `0600` on unix, and gone however the post ends.
struct PrivateFile {
    path: PathBuf,
}

impl PrivateFile {
    fn write(contents: &str) -> std::io::Result<Self> {
        use std::io::Write;
        let path = std::env::temp_dir().join(format!("luxe-review-{}.json", uuid::Uuid::new_v4()));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = Self { path };
        options.open(&file.path)?.write_all(contents.as_bytes())?;
        Ok(file)
    }
}

impl Drop for PrivateFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// GitHub's reasons for refusing a request, when its answer carries them.
fn rejection(stdout: &str) -> Option<GitHubError> {
    #[derive(Deserialize)]
    struct Refusal {
        errors: Vec<serde_json::Value>,
    }
    let refusal: Refusal = serde_json::from_str(stdout.trim()).ok()?;
    let reasons: Vec<String> = refusal
        .errors
        .iter()
        .map(|error| match error {
            serde_json::Value::String(reason) => reason.clone(),
            other => other
                .get("message")
                .and_then(|m| m.as_str())
                .map_or_else(|| other.to_string(), str::to_owned),
        })
        .map(|reason| super::untrusted(&reason, FAILURE_MAX_CHARS))
        .collect();
    (!reasons.is_empty()).then_some(GitHubError::Rejected { reasons })
}

/// How `gh --repo` names a repository: `owner/name`, prefixed with the host off github.com.
fn repo_arg(repository: &Repository) -> String {
    match repository.host.as_str() {
        "github.com" => repository.slug(),
        host => format!("{host}/{}", repository.slug()),
    }
}

/// Turn a failed `gh` call into the variant a caller can act on.
///
/// The one place `gh`'s wording is read. Both streams, because `gh` does not always say why on
/// stderr. Anything unrecognized becomes [`GitHubError::Failed`] with `gh`'s own words, bounded —
/// guessing at an error nobody anticipated is how a tool starts lying about what went wrong.
pub fn classify_failure(stderr: &str, stdout: &str) -> GitHubError {
    let reported = match stderr.trim() {
        "" => stdout.trim(),
        text => text,
    };
    let lower = reported.to_lowercase();
    // First: `gh`'s "no GitHub remote" message ends by suggesting `gh auth login`, and read as a
    // sign-in problem it would send the reader to fix an account that is fine
    if lower.contains("no git remotes found")
        || lower.contains("none of the git remotes configured for this repository")
        || lower.contains("not a git repository")
    {
        return GitHubError::NotGitHub;
    }
    if lower.contains("gh auth login")
        || lower.contains("not logged")
        || lower.contains("authentication token")
        || lower.contains("requires authentication")
        || lower.contains("bad credentials")
    {
        return GitHubError::NotAuthenticated { host: None };
    }
    // Before anything else a 403 might be read as: a spent budget is a wait, not a fault
    if lower.contains("rate limit exceeded")
        || lower.contains("rate limit already exceeded")
        || lower.contains("secondary rate limit")
        || lower.contains("http 429")
    {
        return GitHubError::RateLimited;
    }
    if lower.contains("could not resolve to a") || lower.contains("http 404") {
        return GitHubError::NotFound;
    }
    let message = match reported {
        "" => "no output".to_string(),
        text => super::untrusted(text, FAILURE_MAX_CHARS),
    };
    GitHubError::Failed { message }
}

#[derive(Deserialize)]
struct IssueAuthor {
    login: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Connection<T> {
    #[serde(default)]
    total_count: usize,
    nodes: Vec<T>,
}

#[derive(Deserialize)]
struct Nodes<T> {
    nodes: Vec<T>,
}

#[derive(Deserialize)]
struct Login {
    login: String,
}

/// Seconds since the Unix epoch for GitHub's `2026-09-24T19:05:02Z`, or `None` for anything else.
pub fn rfc3339_seconds(stamp: &str) -> Option<u64> {
    let (date, time) = stamp.strip_suffix('Z')?.split_once('T')?;
    let mut date = date.splitn(3, '-').map(str::parse::<i64>);
    let (year, month, day) = (date.next()?.ok()?, date.next()?.ok()?, date.next()?.ok()?);
    let mut time = time.splitn(3, ':');
    let hour: i64 = time.next()?.parse().ok()?;
    let minute: i64 = time.next()?.parse().ok()?;
    // Fractional seconds are dropped; a refill time is not that precise
    let second: i64 = time.next()?.split('.').next()?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 {
        return None;
    }
    // Days from the civil calendar, after Howard Hinnant's `days_from_civil`
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    u64::try_from(days * 86_400 + hour * 3_600 + minute * 60 + second).ok()
}

/// Longest slice of `gh`'s own error text carried in [`GitHubError::Failed`], in characters.
pub(super) const FAILURE_MAX_CHARS: usize = 600;

/// Longest title or summary a status row carries, in characters.
pub(super) const STATUS_TEXT_MAX_CHARS: usize = 200;

/// GitHub's own tally of the head's checks, sorted the way [`roll_up`](pull_requests::roll_up)
/// sorts a list of them.
pub(super) fn count_checks(contexts: &RollupContexts) -> CheckRollup {
    let mut rollup = CheckRollup::default();
    for StateCount { state, count } in &contexts.check_run_counts_by_state {
        let bucket = match state.as_str() {
            "SUCCESS" => &mut rollup.passed,
            "NEUTRAL" | "SKIPPED" => &mut rollup.skipped,
            "FAILURE" | "TIMED_OUT" | "CANCELLED" | "ACTION_REQUIRED" | "STARTUP_FAILURE"
            | "STALE" => &mut rollup.failed,
            "QUEUED" | "IN_PROGRESS" | "PENDING" | "WAITING" => &mut rollup.pending,
            _ => continue,
        };
        *bucket += count;
    }
    for StateCount { state, count } in &contexts.status_context_counts_by_state {
        let bucket = match state.as_str() {
            "SUCCESS" => &mut rollup.passed,
            "PENDING" | "EXPECTED" => &mut rollup.pending,
            "FAILURE" | "ERROR" => &mut rollup.failed,
            _ => continue,
        };
        *bucket += count;
    }
    rollup
}

/// A stranger's one-liner, fit for a status row: filtered, on one line, and bounded.
pub(super) fn status_text(text: &str) -> String {
    let clean = crate::instructions::sanitize(text).text;
    let line = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= STATUS_TEXT_MAX_CHARS {
        return line;
    }
    let kept: String = line.chars().take(STATUS_TEXT_MAX_CHARS - 1).collect();
    format!("{}…", kept.trim_end())
}

/// One JSON object per line, the shape `--paginate --jq '.[] | …'` prints across every page.
pub(super) fn parse_lines<T: serde::de::DeserializeOwned>(
    body: &str,
) -> Result<Vec<T>, GitHubError> {
    body.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).map_err(GitHubError::Unreadable))
        .collect()
}

#[derive(Deserialize)]
pub(super) struct StateCount {
    pub(super) state: String,
    pub(super) count: usize,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RollupContexts {
    #[serde(default)]
    pub(super) check_run_counts_by_state: Vec<StateCount>,
    #[serde(default)]
    pub(super) status_context_counts_by_state: Vec<StateCount>,
}

#[cfg(test)]
mod tests;
