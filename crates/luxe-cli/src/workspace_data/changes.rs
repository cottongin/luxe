//! Changes — what git says is different in the checkout, and how much.

use super::LUXE_PROJECT_DIR;
use crate::git_probe::run as run_git;
use luxe_core::{FileChangeStatus, FileChangeSummary, WorkspaceChangesSummary};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub(super) async fn collect_change_summary(root: PathBuf) -> WorkspaceChangesSummary {
    let (status, numstat) = tokio::join!(
        run_git(&root, &["status", "--porcelain=v1", "-z"]),
        run_git(&root, &["diff", "--numstat", "-z", "HEAD"]),
    );
    let Some(status) = status else {
        return WorkspaceChangesSummary::default();
    };
    let counts = numstat.as_deref().map(parse_numstat).unwrap_or_default();
    let files = parse_status(&status)
        .into_iter()
        // Luxe's own project directory — sessions, logs, tasks, review artifacts — is not the
        // user's work. Listing it made the dock's count disagree with the review's forever.
        .filter(|(path, _)| !path.starts_with(LUXE_PROJECT_DIR))
        .map(|(path, status)| {
            let (additions, deletions, binary) =
                counts.get(&path).copied().unwrap_or((None, None, false));
            FileChangeSummary { path, status, additions, deletions, binary }
        })
        .collect::<Vec<_>>();
    let total_files = files.len();
    let truncated = total_files > 200;
    let mut files = files.into_iter().take(200).collect::<Vec<_>>();
    // `git diff` says nothing about untracked paths, so a brand-new file arrived with no counts —
    // and rendered as `+0 −0`, which claims it changed nothing. Its whole content is an addition.
    let root_for_counts = root.clone();
    files = tokio::task::spawn_blocking(move || {
        for file in &mut files {
            if file.status == FileChangeStatus::Untracked && file.additions.is_none() {
                if let Some(lines) = count_added_lines(&root_for_counts.join(&file.path)) {
                    file.additions = Some(lines);
                    file.deletions = Some(0);
                }
            }
        }
        files
    })
    .await
    .unwrap_or_default();
    let totals_known = total_files == 0
        || files.len() == total_files
            && files.iter().all(|file| file.additions.is_some() && file.deletions.is_some());
    WorkspaceChangesSummary {
        additions: totals_known.then(|| files.iter().filter_map(|file| file.additions).sum()),
        deletions: totals_known.then(|| files.iter().filter_map(|file| file.deletions).sum()),
        files,
        total_files,
        truncated,
    }
}

pub(super) fn parse_status(output: &str) -> Vec<(PathBuf, FileChangeStatus)> {
    let mut entries = output.split('\0').filter(|entry| !entry.is_empty());
    let mut changes = Vec::new();
    while let Some(entry) = entries.next() {
        let bytes = entry.as_bytes();
        if bytes.len() < 4 {
            continue;
        }
        let code = &entry[..2];
        let path = PathBuf::from(&entry[3..]);
        let status = if code == "??" {
            FileChangeStatus::Untracked
        } else if code.contains('D') {
            FileChangeStatus::Deleted
        } else if code.contains('R') || code.contains('C') {
            let _ = entries.next();
            FileChangeStatus::Renamed
        } else if code.contains('A') {
            FileChangeStatus::Added
        } else {
            FileChangeStatus::Modified
        };
        changes.push((path, status));
    }
    changes
}

pub(super) fn parse_numstat(output: &str) -> HashMap<PathBuf, (Option<u64>, Option<u64>, bool)> {
    output
        .split('\0')
        .filter_map(|entry| {
            let mut fields = entry.splitn(3, '\t');
            let additions = fields.next()?;
            let deletions = fields.next()?;
            let path = fields.next()?;
            if path.is_empty() {
                return None;
            }
            let binary = additions == "-" || deletions == "-";
            Some((PathBuf::from(path), (additions.parse().ok(), deletions.parse().ok(), binary)))
        })
        .collect()
}

/// Lines in a newly added file, or `None` when it is not a countable text file.
///
/// Bounded on purpose: a snapshot refresh runs on every change, so this reads a capped prefix and
/// gives up on anything binary rather than paying for a large blob. Git reports an untracked
/// *directory* as a single `dir/` entry, which is not a file at all and simply reports nothing.
pub(super) fn count_added_lines(path: &Path) -> Option<u64> {
    /// Past this, the count is not worth the read for a dock row.
    const MAX_BYTES: u64 = 1 << 20;

    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    if bytes.iter().take(8192).any(|byte| *byte == 0) {
        return None;
    }
    if bytes.is_empty() {
        return Some(0);
    }
    let newlines = bytes.iter().filter(|byte| **byte == b'\n').count() as u64;
    // A trailing byte that is not a newline is still a line.
    Some(newlines + u64::from(bytes.last() != Some(&b'\n')))
}
