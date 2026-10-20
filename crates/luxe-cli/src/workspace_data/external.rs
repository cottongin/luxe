//! External changes — edits outside any repository, measured against what was kept before the first.

use super::changes::{collect_change_summary, count_added_lines};
use crate::git_probe;
use luxe_core::external_work::{ExternalBaseline, ExternalWorkRecord};
use luxe_core::{ExternalChangesSummary, ExternalRootSummary, FileChangeStatus, FileChangeSummary};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Most rows shown for one external root before the list is counted instead of listed.
///
/// Smaller than the project's bound on purpose: this section is a pointer to work done elsewhere,
/// not the surface someone reviews it on, and every loose row costs a `git diff` to measure.
const EXTERNAL_FILE_LIMIT: usize = 50;

/// Reduces the session's external-work record to display groups, one per place.
///
/// Two kinds of group, and the difference is worth keeping visible: a Git worktree is asked the
/// same question the project is asked, so its listing includes changes the shell made and changes
/// that were already uncommitted. A loose file has no such witness, so it is measured against what
/// this session found there — which is all anything can honestly claim about it.
pub(super) async fn collect_external_summary(
    record: ExternalWorkRecord,
    project_root: &Path,
) -> ExternalChangesSummary {
    let mut roots = Vec::new();
    for root in &record.roots {
        // A checkout that vanished mid-session (a worktree removed, a mount dropped) has nothing to
        // report and must not render as an empty group.
        if !root.is_dir() {
            continue;
        }
        let changes = collect_change_summary(root.clone()).await;
        // A repository that *contains* the project — an edit in a sibling directory of a checkout
        // luxe was opened one level inside — reports the project's own files too. Those already
        // have a section, and listing them twice would double every number on the screen.
        let mut files = changes.files;
        files.retain(|file| !luxe_core::paths::contains(project_root, &root.join(&file.path)));
        if files.is_empty() {
            continue;
        }
        let dropped = changes.total_files.saturating_sub(files.len());
        let branch = git_probe::branch(root).await;
        roots.push(ExternalRootSummary {
            display_path: luxe_core::paths::home_relative(root),
            root: root.clone(),
            git_branch: branch,
            git: true,
            total_files: changes.total_files - dropped,
            truncated: changes.truncated,
            files,
        });
    }
    roots.extend(collect_loose_groups(&record).await);
    // Stable order regardless of the order the session happened to reach them in.
    roots.sort_by(|left, right| left.root.cmp(&right.root));

    // A count is only reportable when every row it sums has one, matching the project section: a
    // total that quietly omits an unmeasurable file is worse than no total.
    let complete = roots.iter().all(|root| {
        !root.truncated
            && root.files.len() == root.total_files
            && root.files.iter().all(|file| file.additions.is_some() && file.deletions.is_some())
    });
    ExternalChangesSummary {
        total_files: roots.iter().map(|root| root.total_files).sum(),
        additions: complete
            .then(|| roots.iter().flat_map(|r| &r.files).filter_map(|f| f.additions).sum()),
        deletions: complete
            .then(|| roots.iter().flat_map(|r| &r.files).filter_map(|f| f.deletions).sum()),
        truncated: record.truncated,
        roots,
    }
}

/// Groups loose files by the directory they sit in and measures each against its baseline.
async fn collect_loose_groups(record: &ExternalWorkRecord) -> Vec<ExternalRootSummary> {
    let mut by_directory: HashMap<PathBuf, Vec<FileChangeSummary>> = HashMap::new();
    let mut measured = 0usize;
    for file in &record.files {
        let Some(directory) = file.path.parent() else {
            continue;
        };
        let rows = by_directory.entry(directory.to_path_buf()).or_default();
        // Past the bound the file is still counted, but nothing is spawned to measure it.
        let budget_left = measured < EXTERNAL_FILE_LIMIT && rows.len() < EXTERNAL_FILE_LIMIT;
        let Some(change) = loose_change(&file.path, &file.baseline, budget_left).await else {
            continue;
        };
        measured += 1;
        rows.push(change);
    }
    by_directory
        .into_iter()
        .filter(|(_, files)| !files.is_empty())
        .map(|(directory, mut files)| {
            files.sort_by(|left, right| left.path.cmp(&right.path));
            let total_files = files.len();
            let truncated = total_files > EXTERNAL_FILE_LIMIT;
            files.truncate(EXTERNAL_FILE_LIMIT);
            ExternalRootSummary {
                display_path: luxe_core::paths::home_relative(&directory),
                root: directory,
                git_branch: None,
                git: false,
                files,
                total_files,
                truncated,
            }
        })
        .collect()
}

/// One loose file's row, or `None` when nothing about it actually changed.
///
/// A path the session touched and left as it found it is not a change, and neither is one it
/// created and then removed. Reporting either would make the section a list of what the agent
/// looked at rather than what it did.
async fn loose_change(
    path: &Path,
    baseline: &ExternalBaseline,
    measurable: bool,
) -> Option<FileChangeSummary> {
    let row = |status, additions, deletions| {
        Some(FileChangeSummary {
            path: path.file_name().map(PathBuf::from).unwrap_or_else(|| path.to_path_buf()),
            status,
            additions,
            deletions,
            binary: false,
        })
    };
    let present = path.is_file();
    match (baseline, present) {
        // Created and then removed again: the filesystem is where it started.
        (ExternalBaseline::Absent, false) => None,
        (ExternalBaseline::Absent, true) => {
            let added = count_added_lines(path);
            row(FileChangeStatus::Added, added, added.map(|_| 0))
        }
        (ExternalBaseline::Unretained, true) => row(FileChangeStatus::Modified, None, None),
        (ExternalBaseline::Unretained, false) => row(FileChangeStatus::Deleted, None, None),
        // Everything it held is gone, and the baseline is exactly how much that was.
        (ExternalBaseline::Text { .. }, false) => {
            row(FileChangeStatus::Deleted, Some(0), baseline.line_count())
        }
        (ExternalBaseline::Text { .. }, true) => {
            if !measurable {
                return row(FileChangeStatus::Modified, None, None);
            }
            match diff_against_baseline(path, baseline).await {
                // Git found no differences: the session left it exactly as it was.
                Some((0, 0)) => None,
                Some((additions, deletions)) => {
                    row(FileChangeStatus::Modified, Some(additions), Some(deletions))
                }
                None => row(FileChangeStatus::Modified, None, None),
            }
        }
    }
}

/// Measures a loose file against the content this session first saw, via a temporary file.
async fn diff_against_baseline(path: &Path, baseline: &ExternalBaseline) -> Option<(u64, u64)> {
    let ExternalBaseline::Text { content } = baseline else {
        return None;
    };
    let content = content.clone();
    let before = tokio::task::spawn_blocking(move || ScratchFile::holding(content.as_bytes()))
        .await
        .ok()??;
    git_probe::numstat_no_index(&before.path, path).await
}

/// A temporary file that removes itself, for handing one string to a subprocess.
///
/// Named from the process id and a counter rather than a random id, which keeps a dependency out of
/// the binary for a file that lives for one `git diff`: within a process the counter is unique, and
/// across processes the pid is.
struct ScratchFile {
    path: PathBuf,
}

impl ScratchFile {
    fn holding(bytes: &[u8]) -> Option<Self> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "luxe-baseline-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&path, bytes).ok()?;
        Some(Self { path })
    }
}

impl Drop for ScratchFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
