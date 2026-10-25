//! `git` `checks` — why CI is failing, bounded.
//!
//! `gh run view --log-failed` prints every line of every failing step: 3,671 lines for one Clippy
//! step of luxe's own, with the error in the last dozen. This reads the same logs through GitHub
//! and keeps the tail of each failing step, says how much it left out, and also lists the failing
//! checks that are not Actions jobs at all — a commit status from another CI, which that command
//! cannot see. Reading only; re-running a job stays behind the permission engine's `ask`.

use luxe_core::github::{
    Branches, CheckLog, Checks, Discovery, FailingCheck, GitHubError, PullRequests, Repository,
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

/// What `checks` was asked about, read from its `args`.
#[derive(Debug, PartialEq, Eq)]
struct Request {
    /// Pull request named, when one was
    number: Option<u64>,
    /// Repository named with `-R`, when it isn't this checkout's
    repository: Option<Repository>,
}

/// Why checks are failing on the branch's pull request, on pull request `#n` when `args` names one,
/// or on `HEAD` when the branch has no pull request. `-R owner/name` reads another repository's.
pub(super) async fn checks(
    github: &(impl Discovery + PullRequests + Branches + Checks + ?Sized),
    cwd: &Path,
    args: &[String],
) -> Result<String, String> {
    let (repository, target) = resolve(github, cwd, parse_request(args)?).await?;
    let failing = github
        .failing_checks(&repository, &target.oid, LOG_TAIL)
        .await
        .map_err(|error| failure(&error))?;
    Ok(render(&target, &failing))
}

/// Why `-R` alone is refused: the branch-to-pull-request lookup only knows this checkout.
const REPO_NEEDS_NUMBER: &str = "Name the pull request as well as `-R`: the branch here belongs \
                                 to this checkout, not that repository.";

/// The repository and commit `request` asks about. A repository named with `-R` is named in the
/// answer too, so it can't be mistaken for this checkout's.
async fn resolve(
    github: &(impl Discovery + PullRequests + Branches + ?Sized),
    cwd: &Path,
    request: Request,
) -> Result<(Repository, Target), String> {
    if let Some(repository) = request.repository {
        let number = request.number.ok_or(REPO_NEEDS_NUMBER)?;
        let mut target = named_target(github, &repository, number).await?;
        target.name = format!("{}{}", repository.slug(), target.name);
        return Ok((repository, target));
    }
    let repository = github.repository(cwd).await.map_err(|error| failure(&error))?;
    let target = match request.number {
        Some(number) => named_target(github, &repository, number).await?,
        None => branch_target(github, cwd, &repository).await?,
    };
    Ok((repository, target))
}

/// The pull request number and `-R` repository in `args`. Anything else is refused rather than
/// dropped: an argument skipped in silence answers a different question than the one asked.
fn parse_request(args: &[String]) -> Result<Request, String> {
    let mut request = Request { number: None, repository: None };
    let mut args = args.iter().map(String::as_str);
    while let Some(arg) = args.next() {
        if let Some(slug) = repo_flag(arg, &mut args)? {
            request.repository = Some(repository_from_slug(slug)?);
        } else if request.number.replace(pull_request_number(arg)?).is_some() {
            return Err("`checks` reads one pull request at a time.".into());
        }
    }
    Ok(request)
}

/// The repository `arg` names when it is `-R`/`--repo` (taking the next argument) or `--repo=`.
fn repo_flag<'a>(
    arg: &'a str,
    rest: &mut impl Iterator<Item = &'a str>,
) -> Result<Option<&'a str>, String> {
    match arg {
        "-R" | "--repo" => {
            rest.next().map(Some).ok_or_else(|| "`-R` needs a repository: `-R owner/name`.".into())
        }
        other => Ok(other.strip_prefix("--repo=")),
    }
}

/// `arg` as a pull request number, `#` or not; a flag `checks` doesn't know is refused by name.
fn pull_request_number(arg: &str) -> Result<u64, String> {
    if arg.starts_with('-') {
        return Err(format!(
            "`checks` doesn't take `{arg}`. It takes a pull request number, and `-R owner/name` \
             for a repository other than this checkout's."
        ));
    }
    let number = arg.trim_start_matches('#');
    number.parse().map_err(|_| format!("`{number}` is not a pull request number."))
}

/// The repository `owner/name` names on github.com, or `host/owner/name` on another host, as
/// `gh -R` reads it.
fn repository_from_slug(slug: &str) -> Result<Repository, String> {
    let (host, owner, name) = slug_parts(slug)
        .ok_or_else(|| format!("`{slug}` is not a repository: `-R` takes owner/name."))?;
    Ok(Repository {
        host: host.to_ascii_lowercase(),
        owner: owner.to_string(),
        name: name.to_string(),
        // Reading a pull request's checks never consults either, so neither is looked up
        default_branch: String::new(),
        remote: None,
    })
}

/// Host, owner and name, when each part of `slug` is one GitHub would accept.
fn slug_parts(slug: &str) -> Option<(&str, &str, &str)> {
    let valid = |part: &str| {
        !part.is_empty()
            && part.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    let parts: Vec<&str> = slug.split('/').collect();
    let (host, owner, name) = match parts.as_slice() {
        [owner, name] => ("github.com", *owner, *name),
        [host, owner, name] => (*host, *owner, *name),
        _ => return None,
    };
    [host, owner, name].into_iter().all(valid).then_some((host, owner, name))
}

/// Pull request `number`'s head, by its number and title.
async fn named_target(
    github: &(impl PullRequests + ?Sized),
    repository: &Repository,
    number: u64,
) -> Result<Target, String> {
    let pr = github.pull_request(repository, number).await.map_err(|e| failure(&e))?;
    Ok(Target {
        oid: pr.head_oid,
        name: format!("#{} · {}", pr.number, luxe_core::github::untrusted(&pr.title, 120)),
        pending: Some(pr.checks.pending),
    })
}

/// The branch's pull request head, or `HEAD` when there is no pull request to ask about.
async fn branch_target(
    github: &(impl Branches + ?Sized),
    cwd: &Path,
    repository: &Repository,
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
