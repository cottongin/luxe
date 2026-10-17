//! Git probe — bounded, best-effort git questions for the CLI's read-only surfaces.
//!
//! The dashboard, the status line, and the Context Dock all want the same few facts about the
//! repository they opened in, and none of them may block on getting them: a huge repository, a
//! network filesystem, or a held index lock must cost a missing row, never a late first frame.
//! Every call here is capped and fails to `None`.
//!
//! Owning the invocation in one place also settles how output is trimmed, which is not a detail —
//! see [`run`].

use std::path::Path;
use std::time::Duration;

use tokio::process::Command;

/// How long any single git call may take before its answer is abandoned.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// Runs `git <args>` in `root`, returning its stdout with only the **trailing** newline removed.
///
/// Not `trim()`. In `--porcelain -z` output the leading character of a status code is a
/// significant space — a worktree-only modification reads `" M path"` — so trimming the front
/// shifts the first record by one, costing its path a character (`src/a.txt` → `rc/a.txt`) and,
/// keyed on that path, silently zeroing its `+/-` counts.
pub(crate) async fn run(root: &Path, args: &[&str]) -> Option<String> {
    let mut command = Command::new("git");
    command.args(args).current_dir(root).kill_on_drop(true);
    let output = tokio::time::timeout(PROBE_TIMEOUT, command.output()).await.ok()?.ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim_end().to_string())
}

/// `(additions, deletions)` between two files Git does not track, or `None` when it cannot say.
///
/// `--no-index` is how a file with no repository behind it gets measured: the pre-edit content is
/// handed over as a temporary file and Git diffs the pair. It cannot go through [`run`], because a
/// diff that *found* differences exits 1 — the very case this is called for.
///
/// Three outcomes, and collapsing any two of them is a lie: identical files exit 0 with no output
/// and are `Some((0, 0))`; a binary pair prints `-` for both columns and is `None`, because it
/// changed by an amount lines cannot express; and a Git that would not run is also `None`. "No
/// differences" and "no answer" reaching a caller as one value would list an untouched file as
/// changed.
pub(crate) async fn numstat_no_index(before: &Path, after: &Path) -> Option<(u64, u64)> {
    let mut command = Command::new("git");
    command
        .args(["diff", "--no-index", "--numstat", "--"])
        .arg(before)
        .arg(after)
        .kill_on_drop(true);
    let output = tokio::time::timeout(PROBE_TIMEOUT, command.output()).await.ok()?.ok()?;
    if !matches!(output.status.code(), Some(0 | 1)) {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let Some(line) = text.lines().next() else {
        return Some((0, 0));
    };
    let mut fields = line.splitn(3, '\t');
    let additions = fields.next()?.parse().ok()?;
    let deletions = fields.next()?.parse().ok()?;
    Some((additions, deletions))
}

/// The current branch, falling back to `@<short-oid>` when HEAD is detached.
pub(crate) async fn branch(root: &Path) -> Option<String> {
    let branch = run(root, &["branch", "--show-current"]).await?;
    if !branch.is_empty() {
        return Some(branch);
    }
    run(root, &["rev-parse", "--short", "HEAD"])
        .await
        .filter(|commit| !commit.is_empty())
        .map(|commit| format!("@{commit}"))
}

/// The commit `HEAD` points at, or `None` outside a repository and before the first commit.
///
/// Distinct from [`branch`], which falls back to a commit only on a detached `HEAD` and prefixes
/// it. This is the raw identity, and what it is for is comparison between snapshots.
pub(crate) async fn head(root: &Path) -> Option<String> {
    run(root, &["rev-parse", "HEAD"]).await.filter(|commit| !commit.is_empty())
}

/// Whether the worktree has any tracked or untracked change.
pub(crate) async fn is_dirty(root: &Path) -> bool {
    run(root, &["status", "--porcelain", "--untracked-files=normal"])
        .await
        .is_some_and(|status| !status.is_empty())
}
