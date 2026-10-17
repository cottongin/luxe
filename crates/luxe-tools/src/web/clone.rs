//! Fetched repositories — a GitHub repository URL handed to `web_fetch`, checked out where the agent
//! can read it.
//!
//! The checkout lives at `.luxe/gh-cache/<owner>/<repo>` ([`luxe_core::fetched`]), inside the
//! project so reading it never trips the external-directory guard. That convenience is also the
//! risk: it is a stranger's tree sitting inside the user's, judged by the same lexical path checks.
//! So the clone is made to carry nothing that acts on its own:
//!
//! - **Symbolic links are checked out as plain files** holding their target
//!   (`core.symlinks=false`). A link is the one thing in a repository that can point *out* of it,
//!   and a `README.md` linked to `../../../../.env` would otherwise hand the project's secrets to
//!   the model past the `.env` rule.
//! - **Nothing prompts.** A private or misspelled repository makes git ask for a username on the
//!   terminal, which here is the TUI's screen; with prompts off it fails at once and says why.
//! - **No LFS objects, no submodules, no tags.** The source is the point, not the binaries.
//! - **Nothing half-made is ever cached.** It clones into a staging directory and renames it into
//!   place, so a clone stopped by a timeout, a cancelled turn or the size cap leaves nothing that
//!   looks like a finished checkout.
//!
//! Language servers and format-on-save stay out of the cache too, but that is their check, not
//! this module's — see [`luxe_core::fetched::contains`].

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use luxe_core::tool::{ToolContext, ToolResultData};

/// How old a checkout gets before the next fetch of it clones again.
const REFRESH_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

/// How long a checkout nobody has fetched is kept.
const PRUNE_AFTER: Duration = Duration::from_secs(14 * 24 * 60 * 60);

/// How old a staging directory must be before it counts as left behind rather than in progress.
const STAGING_STALE_AFTER: Duration = Duration::from_secs(60 * 60);

/// What a clone may run to, on disk.
const MAX_CLONE_BYTES: u64 = 500 * 1024 * 1024;

/// Longest a clone runs. Under the tool's own network deadline, so a slow clone ends with this
/// module's explanation and cleanup rather than dispatch dropping the call.
const CLONE_DEADLINE: Duration = Duration::from_secs(45);

/// How often a running clone's size is checked against [`MAX_CLONE_BYTES`].
const SIZE_POLL: Duration = Duration::from_millis(500);

/// Name prefix of a directory a clone is being made in, or a replaced checkout on its way out.
const STAGING_PREFIX: &str = ".staging-";

/// When a checkout was cloned, in seconds since the epoch, inside its `.git`.
const FETCHED_STAMP: &str = "luxe-fetched";

/// When a checkout was last fetched by anyone, inside its `.git`; what pruning goes by.
const USED_STAMP: &str = "luxe-used";

/// What bounds one clone. A struct so a test can make the cap small enough to hit.
#[derive(Debug, Clone, Copy)]
struct Limits {
    max_bytes: u64,
    deadline: Duration,
}

const LIMITS: Limits = Limits { max_bytes: MAX_CLONE_BYTES, deadline: CLONE_DEADLINE };

/// Why a clone did not produce a checkout.
#[derive(Debug, thiserror::Error)]
enum CloneError {
    #[error("git is not installed")]
    NoGit,
    #[error("git clone failed: {0}")]
    Git(String),
    #[error("the clone took longer than {}s and was stopped", .0.as_secs())]
    Timeout(Duration),
    #[error("the repository is larger than {} MB on disk and the clone was stopped", .0 / (1024 * 1024))]
    TooLarge(u64),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

/// Fetch `owner/repo` from GitHub for the project at `cx.cwd`, or reuse the checkout already there.
pub(super) async fn fetch_repository(owner: &str, repo: &str, cx: &ToolContext) -> ToolResultData {
    let source = format!("https://github.com/{owner}/{repo}.git");
    fetch_into(&source, owner, repo, &cx.cwd, LIMITS).await
}

/// [`fetch_repository`] from any `source` git can clone, so the tests can use a local repository.
async fn fetch_into(
    source: &str,
    owner: &str,
    repo: &str,
    root: &Path,
    limits: Limits,
) -> ToolResultData {
    let cache = luxe_core::fetched::cache_dir(root);
    let Some(dest) = checkout_dir(&cache, owner, repo) else {
        return ToolResultData::error(format!(
            "`{owner}/{repo}` is not a GitHub repository name Luxe will clone."
        ));
    };
    {
        let cache = cache.clone();
        let _ = tokio::task::spawn_blocking(move || tidy(&cache, now_secs())).await;
    }

    let fetched = read_stamp(&dest, FETCHED_STAMP);
    let have = dest.join(".git").is_dir();
    let stale = fetched.is_none_or(|at| now_secs().saturating_sub(at) >= REFRESH_AFTER.as_secs());
    let note = if have && !stale {
        None
    } else {
        match clone_into(source, &cache, &dest, owner, repo, limits).await {
            Ok(()) => None,
            // A copy that could not be refreshed is still a copy, and usually the right one
            Err(reason) if have => Some(format!(
                "Could not refresh it ({reason}), so this copy is from {}.",
                age(fetched.unwrap_or(0))
            )),
            Err(reason) => {
                return ToolResultData::error(format!(
                    "Could not clone {owner}/{repo}: {reason}. If it exists and is private, git \
                     needs credentials for github.com, such as `gh auth setup-git`."
                ));
            }
        }
    };
    write_stamp(&dest, USED_STAMP);
    describe(owner, repo, &dest, note).await
}

/// Where `owner/repo` is checked out, or `None` for a name that is not safe to use as a path.
///
/// One directory per owner, so `a-b/c` and `a/b-c` cannot share a checkout the way a joined
/// `owner-repo` name let them.
fn checkout_dir(cache: &Path, owner: &str, repo: &str) -> Option<PathBuf> {
    let owner_ok =
        !owner.is_empty() && owner.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    let repo_ok = !repo.is_empty()
        && repo.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && !matches!(repo, "." | "..")
        && !repo.eq_ignore_ascii_case(".git");
    (owner_ok && repo_ok).then(|| cache.join(owner).join(repo))
}

/// Clone `source` into a staging directory and move it to `dest`, replacing what was there.
async fn clone_into(
    source: &str,
    cache: &Path,
    dest: &Path,
    owner: &str,
    repo: &str,
    limits: Limits,
) -> Result<(), CloneError> {
    let git = luxe_core::program::resolve("git").ok_or(CloneError::NoGit)?;
    std::fs::create_dir_all(cache)?;
    let staging = staging_dir(cache, owner, repo);
    let mut guard = Staging { dir: Some(staging.clone()), pid: None };

    let child = clone_command(&git, source, &staging, cache).spawn()?;
    guard.pid = child.id();
    let wait = child.wait_with_output();
    tokio::pin!(wait);
    let deadline = tokio::time::sleep(limits.deadline);
    tokio::pin!(deadline);
    let mut poll = tokio::time::interval(SIZE_POLL);
    let output = loop {
        tokio::select! {
            output = &mut wait => break output?,
            () = &mut deadline => return Err(CloneError::Timeout(limits.deadline)),
            _ = poll.tick() => {
                let dir = staging.clone();
                let max = limits.max_bytes;
                let over = tokio::task::spawn_blocking(move || size_over(&dir, max))
                    .await
                    .unwrap_or(false);
                if over {
                    return Err(CloneError::TooLarge(limits.max_bytes));
                }
            }
        }
    };
    guard.pid = None;
    if !output.status.success() {
        return Err(CloneError::Git(String::from_utf8_lossy(&output.stderr).trim().to_string()));
    }
    // A clone that finished under the cap can still have gone over it on its last stretch
    if size_over(&staging, limits.max_bytes) {
        return Err(CloneError::TooLarge(limits.max_bytes));
    }
    write_stamp(&staging, FETCHED_STAMP);
    write_stamp(&staging, USED_STAMP);

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let retired = dest.exists().then(|| staging_dir(cache, owner, repo));
    if install(&staging, dest, retired.as_deref())? {
        guard.dir = None;
    }
    if let Some(retired) = retired {
        remove_in_background(retired);
    }
    Ok(())
}

/// Move a finished clone from `staging` to `dest`, first moving what was there to `retired`.
///
/// `Ok(false)` when another fetch of the same repository installed one in between; its checkout is
/// as good as this one, and `staging` is left for the caller to remove. On failure the previous
/// checkout is put back.
fn install(staging: &Path, dest: &Path, retired: Option<&Path>) -> std::io::Result<bool> {
    if let Some(retired) = retired {
        std::fs::rename(dest, retired)?;
    }
    match std::fs::rename(staging, dest) {
        Ok(()) => Ok(true),
        Err(_) if dest.join(".git").is_dir() => Ok(false),
        Err(error) => {
            if let Some(retired) = retired {
                let _ = std::fs::rename(retired, dest);
            }
            Err(error)
        }
    }
}

/// The `git clone` that makes a fetched checkout, with everything that would let it act on its own
/// turned off.
fn clone_command(git: &Path, source: &str, staging: &Path, cwd: &Path) -> tokio::process::Command {
    let staging = staging.to_string_lossy();
    let mut command = luxe_core::program::captured_command(
        git,
        &[
            "clone",
            "--depth",
            "1",
            "--no-tags",
            "--quiet",
            // `--config` rather than `git -c`: it is written into the new repository, so it holds
            // for the checkout this command makes and for anything run in it afterwards
            "--config",
            "core.symlinks=false",
            source,
            &staging,
        ],
        cwd,
    );
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env("GIT_LFS_SKIP_SMUDGE", "1");
    command
}

/// A fresh staging directory name under `cache`, carrying the time it was made so [`tidy`] can tell
/// a leftover from a clone still running in another Luxe.
fn staging_dir(cache: &Path, owner: &str, repo: &str) -> PathBuf {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_nanos());
    let secs = nanos / 1_000_000_000;
    cache.join(format!("{STAGING_PREFIX}{secs}-{nanos}-{}-{owner}-{repo}", std::process::id()))
}

/// A clone in progress: on drop, its process tree is killed and its directory removed, which is
/// what happens when the deadline, the size cap or a cancelled turn ends it early.
struct Staging {
    dir: Option<PathBuf>,
    pid: Option<u32>,
}

impl Drop for Staging {
    fn drop(&mut self) {
        if let Some(pid) = self.pid {
            luxe_core::program::kill_tree(pid);
        }
        if let Some(dir) = self.dir.take() {
            remove_in_background(dir);
        }
    }
}

/// Remove a directory off the async runtime; a checkout can be thousands of files.
fn remove_in_background(dir: PathBuf) {
    std::thread::spawn(move || {
        let _ = std::fs::remove_dir_all(&dir);
    });
}

/// Whether the files under `dir` add up to more than `max` bytes. Links are not followed.
fn size_over(dir: &Path, max: u64) -> bool {
    let mut total = 0u64;
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_dir() {
                pending.push(entry.path());
            } else if let Ok(metadata) = entry.metadata() {
                total += metadata.len();
                if total > max {
                    return true;
                }
            }
        }
    }
    false
}

/// Clear out the cache: staging directories left behind, checkouts nobody has fetched in
/// [`PRUNE_AFTER`], and clones made before this layout existed, which may hold live symbolic links.
/// Best effort; a cache that cannot be tidied still works.
fn tidy(cache: &Path, now: u64) {
    let Ok(entries) = std::fs::read_dir(cache) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        if let Some(rest) = name.strip_prefix(STAGING_PREFIX) {
            let made = rest.split('-').next().and_then(|secs| secs.parse::<u64>().ok());
            if made.is_none_or(|made| now.saturating_sub(made) >= STAGING_STALE_AFTER.as_secs()) {
                let _ = std::fs::remove_dir_all(&path);
            }
        } else if path.join(".git").exists() {
            // The old `<owner>-<repo>` layout, cloned with symbolic links intact
            let _ = std::fs::remove_dir_all(&path);
        } else {
            let Ok(repos) = std::fs::read_dir(&path) else { continue };
            for repo in repos.flatten().map(|repo| repo.path()) {
                let used = read_stamp(&repo, USED_STAMP).unwrap_or(0);
                if now.saturating_sub(used) >= PRUNE_AFTER.as_secs() {
                    let _ = std::fs::remove_dir_all(&repo);
                }
            }
            // Only succeeds once the owner has nothing left
            let _ = std::fs::remove_dir(&path);
        }
    }
}

/// The reply: where the checkout is, which commit, how fresh, and enough of it to start from.
async fn describe(owner: &str, repo: &str, dest: &Path, note: Option<String>) -> ToolResultData {
    let path = luxe_core::paths::display(dest);
    let fetched = read_stamp(dest, FETCHED_STAMP).unwrap_or(0);
    let commit =
        head_commit(dest).await.map(|commit| format!(", at commit {commit}")).unwrap_or_default();
    let mut text = format!("{owner}/{repo} is at {path}{commit}, fetched {}.", age(fetched));
    if let Some(note) = note {
        text.push(' ');
        text.push_str(&note);
    }
    text.push_str(
        "\nSymbolic links in it are plain files holding the link's target, and language servers do \
         not start there.\n\nTop level:\n",
    );
    text.push_str(&top_level_listing(dest));
    text.push('\n');
    text.push_str(&readme_preview(dest));
    text.push_str("\nExplore it with the read, glob, and grep tools at that path.");
    ToolResultData::text(text)
}

/// `abc1234 (2026-10-01)` for the checkout's `HEAD`, when git can say.
async fn head_commit(dest: &Path) -> Option<String> {
    let git = luxe_core::program::resolve("git")?;
    let command =
        luxe_core::program::captured_command(git, &["log", "-1", "--format=%h (%cs)"], dest);
    let output = luxe_core::program::run_bounded(command, Duration::from_secs(5)).await.ok()?;
    let line = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (output.status.success() && !line.is_empty()).then_some(line)
}

/// Sorted top-level entries, directories suffixed `/`, bounded. Entry types are read without
/// following links.
fn top_level_listing(dir: &Path) -> String {
    let Ok(entries) = std::fs::read_dir(dir) else { return "  (unreadable)".to_string() };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.file_name() != ".git")
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => format!("{name}/"),
                _ => name,
            }
        })
        .collect();
    names.sort();
    names.truncate(60);
    names.iter().map(|name| format!("  {name}")).collect::<Vec<_>>().join("\n")
}

/// The first ~1.5KB of the README, if there is one and it is a regular file.
///
/// A link is skipped even though the clone should not contain any: this is the read that sends a
/// file's contents to the model with no further step, so it does not get to trust the clone.
fn readme_preview(dir: &Path) -> String {
    use std::io::Read;
    for candidate in ["README.md", "README.markdown", "README.rst", "README.txt", "README"] {
        let path = dir.join(candidate);
        if !std::fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.is_file()) {
            continue;
        }
        let mut content = String::new();
        let Ok(file) = std::fs::File::open(&path) else { continue };
        if file.take(16 * 1024).read_to_string(&mut content).is_err() {
            continue;
        }
        let preview: String = content.chars().take(1500).collect();
        return format!("\nREADME ({candidate}):\n{preview}\n");
    }
    String::new()
}

fn read_stamp(checkout: &Path, stamp: &str) -> Option<u64> {
    std::fs::read_to_string(checkout.join(".git").join(stamp)).ok()?.trim().parse().ok()
}

fn write_stamp(checkout: &Path, stamp: &str) {
    let _ = std::fs::write(checkout.join(".git").join(stamp), now_secs().to_string());
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs())
}

/// How long ago `then` was, in the largest whole unit.
fn age(then: u64) -> String {
    let secs = now_secs().saturating_sub(then);
    let (count, unit) = match secs {
        0..60 => return "just now".to_string(),
        60..3_600 => (secs / 60, "minute"),
        3_600..86_400 => (secs / 3_600, "hour"),
        _ => (secs / 86_400, "day"),
    };
    let plural = if count == 1 { "" } else { "s" };
    format!("{count} {unit}{plural} ago")
}

#[cfg(test)]
mod tests;
