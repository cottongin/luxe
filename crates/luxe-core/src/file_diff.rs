//! File diff — one file's change, captured for a glance.
//!
//! Guided Review pins an immutable source, persists it, and pays a model to explain it. That is
//! the right price for reading a change properly and the wrong one for answering *what did it do
//! to this file* — the question the Context Dock's CHANGES list raises every time it lists a name.
//! This module answers that second question with one `git diff`, parsed by the review's own
//! [`parse_unified_diff`] into the review's own [`ReviewFile`], and nothing kept afterwards.
//!
//! Reusing the review's content model is the point: there is one diff data model in Luxe, so the
//! Changes pane and the review cannot disagree about what a hunk is, and escalating from one to
//! the other shows the same lines. What this does *not* share is the capture: no private index
//! snapshot, no stability re-read, no artifact on disk, no chaptering. A glance is allowed to be
//! whatever the worktree says right now.

use crate::external_work::ExternalBaseline;
use crate::guided_review::{parse_unified_diff, ReviewFile, UntrackedSourcePolicy};
use crate::program::BoundedRunError;
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};
use thiserror::Error;
use tokio::process::Command as TokioCommand;
use uuid::Uuid;

/// How long any single git call may take before the capture gives up.
///
/// A glance that blocks is worse than a glance that fails: the reader clicked a row and is waiting
/// on a pane, so a held index lock or a network filesystem has to cost an error message rather
/// than a frozen surface.
const GIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Diff bytes past which the capture reports itself too large instead of rendering.
///
/// A regenerated lockfile or a vendored blob produces megabytes of unified diff that nobody is
/// going to read a hunk at a time. Naming the limit is honest; silently truncating the middle of a
/// diff is not.
const MAX_DIFF_BYTES: usize = 2 * 1024 * 1024;

/// Context lines git is asked for either side of a change.
///
/// Three is git's own default and what every reviewer has calibrated on.
const CONTEXT_LINES: &str = "-U3";

/// Flags that make git's output *ours* rather than the user's.
///
/// A diff is a rendering, and a person's `~/.gitconfig` is entitled to change it — which is a
/// problem for a parser. `diff.mnemonicprefix` alone rewrites `a/`/`b/` to `c/`/`w/` and every
/// path in the parsed record comes back wrong; `diff.external` and `diff.textconv` can replace the
/// content outright. Pin all four, the way working-tree capture already does.
const CANONICAL_OUTPUT: [&str; 5] =
    [OWN_RENDERING[0], OWN_RENDERING[1], OWN_RENDERING[2], "--src-prefix=a/", "--dst-prefix=b/"];

/// The flags that keep a user's diff config out of a capture: no color, no external diff, no
/// textconv. Every complete Guided Review capture pins them too.
pub(crate) const OWN_RENDERING: [&str; 3] = ["--no-color", "--no-ext-diff", "--no-textconv"];

/// Identifies one asynchronous file-diff request.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FileDiffRequestId(String);

impl FileDiffRequestId {
    /// Creates a distinct request identifier, so a superseded capture can be discarded.
    pub fn new() -> Self {
        Self(format!("file-diff:{}", Uuid::new_v4()))
    }

    /// Returns the stable serialized identifier.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for FileDiffRequestId {
    fn default() -> Self {
        Self::new()
    }
}

/// One file's change, as the Changes pane presents it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileDiff {
    /// Project-relative path the capture was asked for, with forward slashes.
    pub path: String,
    /// The parsed change, or `None` when the file matches its base after all.
    pub file: Option<Box<ReviewFile>>,
    /// Whether the file was captured as untracked content rather than a tracked diff.
    pub untracked: bool,
}

/// Why one file's change could not be captured.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Error)]
#[serde(rename_all = "snake_case")]
pub enum FileDiffFailure {
    /// The requested path is not a safe project-relative path.
    #[error("that is not a path inside this project")]
    UnsafePath,
    /// A file outside any repository, with no retained copy of what it held before.
    #[error("nothing was kept of this file from before it changed, so there is no diff to show")]
    NoBaseline,
    /// The project is not a Git worktree, so there is no base to compare against.
    #[error("this project is not a Git worktree, so there is nothing to compare against")]
    NotAWorktree,
    /// Git could not be run, or refused the operation.
    #[error("git could not describe this file: {0}")]
    Git(String),
    /// The change is larger than the pane will render.
    #[error("this change is too large to show here — open the full review for it")]
    TooLarge,
    /// The file's content is not valid UTF-8, so there are no source lines to show.
    #[error("this file is not text, so there are no lines to show")]
    NotText,
    /// Git produced a diff the review parser rejected.
    #[error("git produced a diff Luxe could not read: {0}")]
    Unreadable(String),
}

/// Captures the change to one `root`-relative `path`, against `HEAD`.
///
/// `HEAD` rather than the index, so a staged change and an unstaged one both show — the reader
/// asked what the file looks like now, not which of git's two waiting rooms the edit is sitting in.
/// A path git does not track falls back to its whole content as an addition, which is what makes a
/// brand-new file (the common case in an agent session) show anything at all.
///
/// `root` is the open project for a file under CHANGES and another worktree entirely for one under
/// ELSEWHERE; the question and the answer are the same either way, which is the point.
pub async fn capture(root: &Path, path: &str) -> Result<FileDiff, FileDiffFailure> {
    let relative = PathBuf::from(path);
    if !is_safe_relative_path(&relative) {
        return Err(FileDiffFailure::UnsafePath);
    }
    if !is_worktree(root).await {
        return Err(FileDiffFailure::NotAWorktree);
    }

    let base = if head_exists(root).await { "HEAD" } else { "--cached" };
    let mut arguments = vec!["diff"];
    arguments.extend_from_slice(&CANONICAL_OUTPUT);
    arguments.extend_from_slice(&["--find-renames", CONTEXT_LINES, base, "--", path]);
    let diff = git(root, &arguments).await?;

    if diff.trim().is_empty() {
        return capture_untracked(root, path, &relative).await;
    }
    Ok(FileDiff { path: path.to_owned(), file: parse_one(&diff, path)?, untracked: false })
}

/// The whole file as an addition, for a path git does not track yet.
///
/// Returns an unchanged result rather than an error when the path is neither tracked nor untracked:
/// a file listed by a snapshot and deleted a moment later is a race, not a fault, and the pane can
/// say "no change" without inventing one.
async fn capture_untracked(
    root: &Path,
    path: &str,
    relative: &Path,
) -> Result<FileDiff, FileDiffFailure> {
    let listed = git(root, &["ls-files", "--others", "--exclude-standard", "--", path]).await?;
    if !listed.lines().any(|line| line == path) {
        return Ok(FileDiff { path: path.to_owned(), file: None, untracked: false });
    }

    let absolute = root.join(relative);
    let limit = UntrackedSourcePolicy::default().max_file_bytes;
    let metadata = tokio::fs::metadata(&absolute)
        .await
        .map_err(|error| FileDiffFailure::Git(error.to_string()))?;
    if metadata.len() > limit {
        return Err(FileDiffFailure::TooLarge);
    }
    let bytes = tokio::fs::read(&absolute)
        .await
        .map_err(|error| FileDiffFailure::Git(error.to_string()))?;
    let contents = String::from_utf8(bytes).map_err(|_| FileDiffFailure::NotText)?;
    let diff = crate::guided_review::render_untracked_addition(relative, &contents);
    Ok(FileDiff { path: path.to_owned(), file: parse_one(&diff, path)?, untracked: true })
}

/// Whether a peeked path names a file outside the open project.
///
/// One rule, said once: a path the dock hands over is relative to the project, unless it is rooted
/// — which only work done elsewhere ever is. The front-end branches on it to decide what a row can
/// offer and the engine branches on it to decide what to ask, and two spellings of "is this
/// outside" would eventually disagree about the same file.
///
/// Rooted rather than *absolute*, which on Windows additionally requires a drive: `/work/sdk` is
/// not absolute there, and a rule that answered differently per platform would make one of them
/// treat a path from somewhere else as a project file and go looking for it in the project.
pub fn is_external_path(path: &str) -> bool {
    Path::new(path).has_root()
}

/// Captures the change to one absolute path that lives outside the open project.
///
/// `requested` comes straight back on the result, so a front-end that has moved on can discard a
/// late answer by the string it asked with.
///
/// Two kinds of place, and which one this is decides what "changed" can even mean. Another Git
/// worktree answers exactly the question the project answers, against its own `HEAD`. A file in no
/// repository has no such witness, so the only honest base is the copy this session retained
/// before it first wrote there — and a path no copy was kept of says so rather than presenting the
/// whole file as new.
pub async fn capture_external(
    requested: &str,
    baseline: Option<&ExternalBaseline>,
) -> Result<FileDiff, FileDiffFailure> {
    let absolute = Path::new(requested);
    if !is_external_path(requested)
        || absolute.components().any(|part| part == Component::ParentDir)
    {
        return Err(FileDiffFailure::UnsafePath);
    }
    let Some(root) = crate::external_work::worktree_root(absolute) else {
        return capture_loose(absolute, requested, baseline).await;
    };
    let Ok(relative) = absolute.strip_prefix(&root) else {
        return Err(FileDiffFailure::UnsafePath);
    };
    let mut diff = capture(&root, &crate::paths::display(relative)).await?;
    // Answered under the name it was asked for: the pane discards a capture whose path is not the
    // one it is looking at, and it is looking at the absolute path the dock gave it.
    diff.path = requested.to_string();
    Ok(diff)
}

/// One file in no repository, measured against what this session first found there.
async fn capture_loose(
    absolute: &Path,
    requested: &str,
    baseline: Option<&ExternalBaseline>,
) -> Result<FileDiff, FileDiffFailure> {
    // A baseline of `Unretained` and no record at all are the same answer to the reader: Luxe
    // cannot say what this file used to hold, so it will not pretend to.
    let Some(baseline @ (ExternalBaseline::Absent | ExternalBaseline::Text { .. })) = baseline
    else {
        return Err(FileDiffFailure::NoBaseline);
    };
    let name = PathBuf::from(loose_label(absolute));
    let unchanged = || FileDiff { path: requested.to_string(), file: None, untracked: false };
    let settled = |diff: &str, untracked| {
        Ok(FileDiff {
            path: requested.to_string(),
            file: parse_one(diff, &name.to_string_lossy())?,
            untracked,
        })
    };
    match (baseline, read_text(absolute).await?) {
        // Created and then removed inside one session: the filesystem is where it started.
        (ExternalBaseline::Absent, None) => Ok(unchanged()),
        (ExternalBaseline::Absent, Some(after)) => {
            settled(&crate::guided_review::render_untracked_addition(&name, &after), true)
        }
        (ExternalBaseline::Text { content }, None) => {
            settled(&crate::guided_review::render_whole_file_deletion(&name, content), false)
        }
        (ExternalBaseline::Text { content }, Some(after)) if *content == after => Ok(unchanged()),
        (ExternalBaseline::Text { content }, Some(after)) => {
            let diff = diff_no_index(&name, content.as_bytes(), after.as_bytes()).await?;
            settled(&diff, false)
        }
        (ExternalBaseline::Unretained, _) => Err(FileDiffFailure::NoBaseline),
    }
}

/// The name a loose file's rendered diff carries.
///
/// Its base name, not its path: the header decides the language a peek highlights it as, and the
/// pane titles the file with the absolute path it asked for either way. A path with no final
/// component cannot exist as a file, so the fallback is only there to keep this total.
fn loose_label(absolute: &Path) -> String {
    absolute.file_name().map_or_else(|| "file".to_string(), |name| name.to_string_lossy().into())
}

/// The file's text now, `None` when there is no file there.
async fn read_text(path: &Path) -> Result<Option<String>, FileDiffFailure> {
    let Ok(metadata) = tokio::fs::metadata(path).await else {
        return Ok(None);
    };
    if !metadata.is_file() {
        return Ok(None);
    }
    if metadata.len() > UntrackedSourcePolicy::default().max_file_bytes {
        return Err(FileDiffFailure::TooLarge);
    }
    let bytes =
        tokio::fs::read(path).await.map_err(|error| FileDiffFailure::Git(error.to_string()))?;
    String::from_utf8(bytes).map(Some).map_err(|_| FileDiffFailure::NotText)
}

/// Git's own diff of two in-memory versions of one file.
///
/// Git rather than a differ of our own, so a peek at a loose file and a review of a tracked one
/// cannot disagree about what a hunk is. The two sides are laid out as `a/<name>` and `b/<name>`
/// under one scratch directory and the prefixes are emptied, which is what makes the header come
/// out as the canonical `a/<name> b/<name>` — including the extension, which is what the parser
/// reads the language from.
async fn diff_no_index(
    name: &Path,
    before: &[u8],
    after: &[u8],
) -> Result<String, FileDiffFailure> {
    let (before, after, name) = (before.to_vec(), after.to_vec(), name.to_path_buf());
    let scratch = tokio::task::spawn_blocking(move || ScratchPair::holding(&name, &before, &after))
        .await
        .map_err(|error| FileDiffFailure::Git(error.to_string()))??;

    let mut command = git_command(&scratch.directory);
    command
        .args(["diff", "--no-index", "--no-color", "--no-ext-diff", "--no-textconv"])
        .args(["--src-prefix=", "--dst-prefix=", CONTEXT_LINES, "--"])
        .arg(&scratch.before)
        .arg(&scratch.after);
    let output = run_git(command).await?;
    // A diff that *found* differences exits 1, which is the whole reason this is here.
    if !matches!(output.status.code(), Some(0 | 1)) {
        return Err(FileDiffFailure::Git(stderr_summary(&output.stderr)));
    }
    if output.stdout.len() > MAX_DIFF_BYTES {
        return Err(FileDiffFailure::TooLarge);
    }
    String::from_utf8(output.stdout).map_err(|_| FileDiffFailure::NotText)
}

/// Two versions of one file on disk, under `a/` and `b/`, removed when the diff is done.
///
/// Both may be private (a personal config file, say), and the directory sits in the shared temp
/// directory: a UUID name created exclusively and owner-only, so no other user can have prepared
/// it, and owner-only files inside.
struct ScratchPair {
    directory: PathBuf,
    before: PathBuf,
    after: PathBuf,
}

impl ScratchPair {
    fn holding(name: &Path, before: &[u8], after: &[u8]) -> Result<Self, FileDiffFailure> {
        let directory = std::env::temp_dir().join(format!("luxe-peek-{}", uuid::Uuid::new_v4()));
        Self::holding_at(directory, name, before, after)
    }

    /// Lay the pair out in `directory`, which must not exist yet.
    fn holding_at(
        directory: PathBuf,
        name: &Path,
        before: &[u8],
        after: &[u8],
    ) -> Result<Self, FileDiffFailure> {
        let failed = |error: std::io::Error| FileDiffFailure::Git(error.to_string());
        crate::fs_atomic::create_private_dir_new(&directory).map_err(failed)?;
        // made here, so from now on the drop is what removes it
        let pair =
            Self { directory, before: Path::new("a").join(name), after: Path::new("b").join(name) };
        for (relative, bytes) in [(&pair.before, before), (&pair.after, after)] {
            let absolute = pair.directory.join(relative);
            if let Some(parent) = absolute.parent() {
                crate::fs_atomic::create_private_dir(parent).map_err(failed)?;
            }
            crate::fs_atomic::write_private(&absolute, bytes).map_err(failed)?;
        }
        Ok(pair)
    }
}

impl Drop for ScratchPair {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

/// Reads the one file record out of a single-path diff.
fn parse_one(diff: &str, path: &str) -> Result<Option<Box<ReviewFile>>, FileDiffFailure> {
    let parsed =
        parse_unified_diff(diff).map_err(|error| FileDiffFailure::Unreadable(error.to_string()))?;
    // A rename shows under its new name and a deletion under its old one, so match either side
    // before falling back to the single record git produced for the pathspec it was given.
    let file = parsed
        .files
        .iter()
        .find(|file| {
            [file.new_path.as_deref(), file.old_path.as_deref()]
                .into_iter()
                .flatten()
                .any(|candidate| crate::paths::display(candidate) == path)
        })
        .or_else(|| parsed.files.first());
    Ok(file.cloned().map(Box::new))
}

/// Whether `root` is inside a non-bare Git worktree.
async fn is_worktree(root: &Path) -> bool {
    git(root, &["rev-parse", "--is-inside-work-tree"])
        .await
        .is_ok_and(|output| output.trim() == "true")
}

/// Whether the repository has a commit to compare against, or is still unborn.
async fn head_exists(root: &Path) -> bool {
    git(root, &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"]).await.is_ok()
}

/// Runs one bounded git command in `root` and returns its stdout as text.
async fn git(root: &Path, arguments: &[&str]) -> Result<String, FileDiffFailure> {
    let mut command = git_command(root);
    command.args(arguments);
    let output = run_git(command).await?;
    if !output.status.success() {
        return Err(FileDiffFailure::Git(stderr_summary(&output.stderr)));
    }
    if output.stdout.len() > MAX_DIFF_BYTES {
        return Err(FileDiffFailure::TooLarge);
    }
    String::from_utf8(output.stdout).map_err(|_| FileDiffFailure::NotText)
}

/// Git in `dir`, leading its own process group, so a peek given up on takes git's filters and
/// hooks with it rather than only git.
fn git_command(dir: &Path) -> TokioCommand {
    let git = crate::program::resolve("git").unwrap_or_else(|| PathBuf::from("git"));
    crate::program::captured_command(git, &[], dir)
}

/// Runs a [`git_command`] under [`GIT_TIMEOUT`], whatever its exit status.
async fn run_git(command: TokioCommand) -> Result<std::process::Output, FileDiffFailure> {
    crate::program::run_bounded(command, GIT_TIMEOUT).await.map_err(|error| match error {
        BoundedRunError::Timeout { .. } => {
            FileDiffFailure::Git("git did not answer in time".into())
        }
        BoundedRunError::Spawn(source) | BoundedRunError::Wait(source) => {
            FileDiffFailure::Git(source.to_string())
        }
    })
}

/// The first line of git's complaint, bounded, for a message a person reads.
fn stderr_summary(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let line = text.lines().find(|line| !line.trim().is_empty()).unwrap_or("no diagnostic output");
    line.chars().take(200).collect()
}

/// Whether a path can safely be resolved beneath a project root.
fn is_safe_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path.components().all(|component| matches!(component, Component::Normal(_)))
}

#[cfg(test)]
mod tests;
