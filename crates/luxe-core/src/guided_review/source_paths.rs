//! Source path validation — safe repository-relative paths for review capture.
//!
//! These helpers reject malformed Git path output before the loader reads or persists workspace
//! content.

use super::GuidedReviewError;
use std::path::{Component, Path, PathBuf};

/// Parses Git's NUL-delimited untracked path list into safe repository-relative paths.
pub(super) fn parse_untracked_paths(output: &[u8]) -> Result<Vec<PathBuf>, GuidedReviewError> {
    if !output.is_empty() && output.last() != Some(&0) {
        return Err(GuidedReviewError::MalformedGitOutput {
            operation: "list untracked paths",
            reason: "NUL-delimited path output has no final terminator",
        });
    }
    output
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|raw_path| {
            let path = String::from_utf8(raw_path.to_vec()).map_err(|_| {
                GuidedReviewError::MalformedGitOutput {
                    operation: "list untracked paths",
                    reason: "path is not valid UTF-8",
                }
            })?;
            let relative = PathBuf::from(&path);
            if !is_safe_relative_path(&relative) {
                return Err(GuidedReviewError::MalformedGitOutput {
                    operation: "list untracked paths",
                    reason: "path is not a safe repository-relative path",
                });
            }
            Ok(relative)
        })
        .collect()
}

/// Returns whether a path can safely be resolved beneath a repository root.
pub(super) fn is_safe_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path.components().all(|component| matches!(component, Component::Normal(_)))
}
