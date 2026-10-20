//! External work — where a session edited outside the project, and what was there first.
//!
//! Git answers "what changed?" for the project, and stops at the worktree boundary. A session that
//! edits `~/.config/foo.toml` or a sibling checkout has still done work, and a Changes surface that
//! reports only the project is not merely incomplete — it reads as a claim about the whole session.
//!
//! This is the record that closes that gap. [`ExternalWorkHook`] sits on `BeforeToolUse`, which is
//! the same call the `external_directory` permission guard is evaluated on, and retains what a path
//! held *before* the tool ran. The collector turns that into a Changes group later; nothing here
//! knows about Git, diffs, or rendering.
//!
//! # What it can and cannot see
//!
//! Only paths luxe's own file tools name. A `bash` command that deletes a file elsewhere is
//! invisible here, which is why an external path inside a Git worktree is recorded as a *root*
//! instead — Git then reports that tree the same way it reports the project, and catches what the
//! shell did too. `ExternalWork::roots` is the honest answer for a checkout; `ExternalWork::loose`
//! is the best available answer for a file that belongs to no repository.

use crate::permission::{external_target, is_mutating_path_tool};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

/// Most bytes retained for one loose file's pre-edit content.
///
/// Matches the review loader's untracked-file limit. Past it the file is still recorded — it was
/// still changed — but without the content a line count could be derived from.
const MAX_RETAINED_BYTES: u64 = 1_048_576;

/// Most loose files retained before the record stops growing.
///
/// A session that rewrites a thousand files in `/tmp` is not describing itself with a list, and the
/// record is read on every workspace refresh.
const MAX_LOOSE_FILES: usize = 200;

/// What a loose external path held when this session first reached it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ExternalBaseline {
    /// Nothing was there — the session created it.
    Absent,
    /// It already existed, and its text was retained so the change can be measured.
    Text {
        /// Complete pre-edit content.
        content: String,
    },
    /// It existed but could not be retained: too large, binary, or unreadable.
    ///
    /// Kept as a distinct state rather than folded into [`Self::Absent`], because "created" and
    /// "changed, and I cannot say by how much" are different claims and only one of them is true.
    Unretained,
}

impl ExternalBaseline {
    /// Lines in the retained content, or `None` when there is none to count.
    pub fn line_count(&self) -> Option<u64> {
        match self {
            Self::Absent => Some(0),
            Self::Text { content } => Some(count_lines(content)),
            Self::Unretained => None,
        }
    }
}

/// Lines in `text`, counting a final line that carries no newline.
fn count_lines(text: &str) -> u64 {
    if text.is_empty() {
        return 0;
    }
    let newlines = text.bytes().filter(|byte| *byte == b'\n').count() as u64;
    newlines + u64::from(!text.ends_with('\n'))
}

/// One loose external file and the state the session found it in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalFile {
    /// Absolute path, normalized the way the permission guard normalizes it.
    pub path: PathBuf,
    /// What was there before this session touched it.
    pub baseline: ExternalBaseline,
}

/// Everything one session has touched outside its project root.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalWorkRecord {
    /// Git worktree roots outside the project, reported by Git like the project itself.
    pub roots: Vec<PathBuf>,
    /// Files belonging to no worktree, measured against what the session first saw.
    pub files: Vec<ExternalFile>,
    /// Whether files were dropped because the record reached `MAX_LOOSE_FILES`.
    pub truncated: bool,
}

impl ExternalWorkRecord {
    /// Whether this session has done no work outside its project.
    pub fn is_empty(&self) -> bool {
        self.roots.is_empty() && self.files.is_empty()
    }
}

#[derive(Debug, Default)]
struct State {
    roots: BTreeSet<PathBuf>,
    files: BTreeMap<PathBuf, ExternalBaseline>,
    truncated: bool,
}

/// A shared, cheaply cloned record of one session's work outside its project.
///
/// Cloning shares the record, matching [`crate::lsp::DiagnosticsStore`]: the hook writes to it and
/// the workspace collector reads from it, and neither owns it.
#[derive(Debug, Clone, Default)]
pub struct ExternalWork {
    state: Arc<Mutex<State>>,
}

impl ExternalWork {
    /// Creates an empty record.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records that `path` — already known to be outside the project — is about to be changed.
    ///
    /// Called *before* the change, so the first call for a path is the only one that can see what
    /// was there; later calls leave the retained baseline alone. A path inside a Git worktree is
    /// recorded as its root instead of as a file, because Git can then report that tree completely
    /// rather than only the parts luxe touched.
    pub fn observe(&self, path: &Path) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        if let Some(root) = worktree_root(path) {
            state.roots.insert(root);
            return;
        }
        // Its own worktree already covers it; a file listed twice is a file counted twice.
        if state.roots.iter().any(|root| crate::paths::contains(root, path)) {
            return;
        }
        if state.files.contains_key(path) {
            return;
        }
        if state.files.len() >= MAX_LOOSE_FILES {
            state.truncated = true;
            return;
        }
        state.files.insert(path.to_path_buf(), capture_baseline(path));
    }

    /// The record as it stands, for a collector or a front-end.
    pub fn record(&self) -> ExternalWorkRecord {
        let Ok(state) = self.state.lock() else {
            return ExternalWorkRecord::default();
        };
        ExternalWorkRecord {
            roots: state.roots.iter().cloned().collect(),
            files: state
                .files
                .iter()
                .map(|(path, baseline)| ExternalFile {
                    path: path.clone(),
                    baseline: baseline.clone(),
                })
                .collect(),
            truncated: state.truncated,
        }
    }

    /// What `path` held when this session first reached it, for a loose file only.
    ///
    /// A path under one of the retained [`ExternalWorkRecord::roots`] answers `None` on purpose:
    /// Git is the witness there, and it is a better one than a copy of a single file.
    pub fn baseline(&self, path: impl AsRef<Path>) -> Option<ExternalBaseline> {
        let path = path.as_ref();
        self.state.lock().ok()?.files.get(path).cloned()
    }

    /// Whether anything outside the project has been touched.
    pub fn is_empty(&self) -> bool {
        self.state
            .lock()
            .map(|state| state.roots.is_empty() && state.files.is_empty())
            .unwrap_or(true)
    }
}

/// Reads what `path` holds now, classifying anything a line count cannot describe.
fn capture_baseline(path: &Path) -> ExternalBaseline {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return ExternalBaseline::Absent;
    };
    if !metadata.is_file() || metadata.len() > MAX_RETAINED_BYTES {
        return ExternalBaseline::Unretained;
    }
    match std::fs::read(path) {
        // NUL first: invalid UTF-8 and binary are the same answer here, and checking the cheap one
        // first keeps a large blob from being decoded before it is rejected.
        Ok(bytes) if bytes.contains(&0) => ExternalBaseline::Unretained,
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(content) => ExternalBaseline::Text { content },
            Err(_) => ExternalBaseline::Unretained,
        },
        Err(_) => ExternalBaseline::Unretained,
    }
}

/// The working copy `path` sits in, found by walking up from its directory.
///
/// Shared with [`crate::file_diff`], which has to sort a peeked path into the same two kinds this
/// does: a place a VCS can be asked about, and a place only the retained baseline can answer for.
/// What counts as a top is [`crate::project`]'s to say.
pub(crate) fn worktree_root(path: &Path) -> Option<PathBuf> {
    let mut current = path.parent();
    while let Some(directory) = current {
        if crate::project::is_working_copy(directory) {
            return Some(directory.to_path_buf());
        }
        current = directory.parent();
    }
    None
}

/// Records external file work as the agent does it.
///
/// Sits on `BeforeToolUse` because that is the last moment the *previous* content of a path still
/// exists. `FileChanged` fires after the write, by which time the baseline is gone.
#[derive(Debug)]
pub struct ExternalWorkHook {
    project_root: PathBuf,
    work: ExternalWork,
}

impl ExternalWorkHook {
    /// Records work outside `project_root` into `work`.
    pub fn new(project_root: impl Into<PathBuf>, work: ExternalWork) -> Self {
        Self { project_root: project_root.into(), work }
    }
}

#[async_trait::async_trait]
impl crate::hook::Hook for ExternalWorkHook {
    fn name(&self) -> &'static str {
        "external-work"
    }

    fn interest(&self) -> &[crate::hook::HookPoint] {
        &[crate::hook::HookPoint::BeforeToolUse]
    }

    async fn on(
        &self,
        _point: crate::hook::HookPoint,
        event: &crate::hook::HookEvent,
        _cx: &crate::hook::HookCtx,
    ) -> crate::hook::HookOutcome {
        let crate::hook::HookEvent::ToolUse { tool_name, args } = event else {
            return crate::hook::HookOutcome::Continue;
        };
        if is_mutating_path_tool(tool_name) {
            if let Some(path) = external_target(tool_name, args, &self.project_root) {
                // Reading a file to retain it is blocking work on the runtime's thread; the tool is
                // about to do the same read anyway, and a hook that returns late delays the call.
                let work = self.work.clone();
                let _ = tokio::task::spawn_blocking(move || work.observe(&path)).await;
            }
        }
        crate::hook::HookOutcome::Continue
    }
}

#[cfg(test)]
mod tests;
