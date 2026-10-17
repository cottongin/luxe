//! Git — the probes a restack asks of a repository, each bounded and read the same way.

use super::*;

pub(super) async fn rev(project: &Path, name: &str) -> Result<String, RestackError> {
    git(project, &["rev-parse", "--verify", "--quiet", &format!("{name}^{{commit}}")])
        .await
        .map_err(|_| RestackError::Git {
            operation: "find a branch of the stack",
            detail: format!("`{name}` is not there after fetching"),
        })
}

pub(super) async fn is_ancestor(project: &Path, ancestor: &str, of: &str) -> bool {
    git(project, &["merge-base", "--is-ancestor", ancestor, of]).await.is_ok()
}

pub(super) async fn count(project: &Path, start: &str, tip: &str) -> usize {
    git(project, &["rev-list", "--count", &format!("{start}..{tip}")])
        .await
        .ok()
        .and_then(|count| count.parse().ok())
        .unwrap_or(0)
}

pub(super) async fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    run(dir, args, GIT_DEADLINE).await
}

/// Git's stdout in `dir`, or the first lines of what it said when it failed.
pub(super) async fn run(dir: &Path, args: &[&str], deadline: Duration) -> Result<String, String> {
    let program = resolve("git").unwrap_or_else(|| PathBuf::from("git"));
    let mut command = captured_command(program, args, dir);
    command.env("GIT_TERMINAL_PROMPT", "0").env("GCM_INTERACTIVE", "never");
    let output = run_bounded(command, deadline).await.map_err(|error| error.to_string())?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
    }
    Err(last_lines(&String::from_utf8_lossy(&output.stderr)))
}

/// The last lines of what git said: its own verdict comes last, after anything the runtime
/// printed first.
pub(super) fn last_lines(said: &str) -> String {
    let lines: Vec<&str> = said.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
    let kept = &lines[lines.len().saturating_sub(3)..];
    if kept.is_empty() {
        "no reason given".into()
    } else {
        kept.join(" ")
    }
}
