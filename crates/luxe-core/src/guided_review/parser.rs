//! Unified-diff parsing for immutable Guided Review source records.
//!
//! This parser preserves source lines and anchors first; classification and chaptering only
//! consume its records later. It intentionally rejects malformed hunk structure rather than
//! presenting a partial source as a complete review.

use super::classification::{classify_hunk_noise, is_lockfile_path, language_for_path};
use super::diff_paths::parse_git_header_paths;
use super::{
    FileId, LineRange, NoiseKind, NonTextChange, NonTextChangeKind, ReviewFile, ReviewFileStatus,
    ReviewHunk, ReviewLine, ReviewLineKind,
};
use std::path::PathBuf;
use thiserror::Error;

mod files;
mod hunks;

use super::diff_paths::parse_path;
use files::*;
use hunks::*;

/// Complete text-file records parsed from one unified diff source.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedDiff {
    /// Parsed text files in canonical source order.
    pub files: Vec<ReviewFile>,
}

/// Errors that prevent a unified diff from being presented as complete.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum GuidedReviewParseError {
    /// A required old/new file path was absent or invalid.
    #[error("invalid file header: {line}")]
    InvalidFileHeader {
        /// Header line that could not be parsed.
        line: String,
    },
    /// A hunk header did not contain both source ranges.
    #[error("invalid hunk header: {header}")]
    InvalidHunkHeader {
        /// Header line that could not be parsed.
        header: String,
    },
    /// A line inside a hunk lacked a recognized unified-diff prefix.
    #[error("invalid hunk line: {line}")]
    InvalidHunkLine {
        /// Source line that could not be classified.
        line: String,
    },
    /// Text file headers were not followed by a source hunk.
    #[error("missing text hunk after file header: {line}")]
    MissingTextHunk {
        /// Old-side file header that lacked source lines.
        line: String,
    },
    /// A nonempty canonical source contained no Git file blocks.
    #[error("unrecognized unified diff source: {line}")]
    UnrecognizedSource {
        /// First nonempty line that could not start a canonical Git source.
        line: String,
    },
    /// A Git file block had no complete text or non-text representation.
    #[error("unrecognized Git file block: {header}")]
    UnrecognizedFileBlock {
        /// Original `diff --git` header.
        header: String,
    },
    /// The canonical source repeated one file lineage in separate Git blocks.
    #[error("duplicate Git file block: {header}")]
    DuplicateFileBlock {
        /// Original duplicate `diff --git` header.
        header: String,
    },
    /// A hunk's actual source records did not match its declared ranges.
    #[error(
        "hunk line count mismatch in {header}: old {actual_old}/{expected_old}, new {actual_new}/{expected_new}"
    )]
    HunkLineCountMismatch {
        /// Original hunk header.
        header: String,
        /// Declared old-side line count.
        expected_old: u32,
        /// Parsed old-side line count.
        actual_old: u32,
        /// Declared new-side line count.
        expected_new: u32,
        /// Parsed new-side line count.
        actual_new: u32,
    },
}

/// Parses canonical file records from a unified diff.
///
/// It handles line-bearing text hunks, rename/copy metadata, quoted paths, and explicit binary,
/// submodule, and mode-only records.
pub fn parse_unified_diff(diff: &str) -> Result<ParsedDiff, GuidedReviewParseError> {
    let lines: Vec<&str> = diff.lines().collect();
    let mut files = Vec::new();
    let mut index = 0;

    while index < lines.len() {
        if !lines[index].starts_with("diff --git ") {
            if lines[index].trim().is_empty() {
                index += 1;
                continue;
            }
            return Err(GuidedReviewParseError::UnrecognizedSource {
                line: lines[index].to_owned(),
            });
        }

        let block_end = lines[index + 1..]
            .iter()
            .position(|line| line.starts_with("diff --git "))
            .map_or(lines.len(), |offset| index + offset + 1);
        let header = lines[index];
        let block = &lines[index + 1..block_end];
        let file = if let Some(file) = parse_non_text_file(header, block)? {
            file
        } else if let Some(file) = parse_text_file(block)? {
            file
        } else if let Some(file) = parse_empty_file(header, block)? {
            file
        } else {
            return Err(GuidedReviewParseError::UnrecognizedFileBlock {
                header: header.to_owned(),
            });
        };
        if files.iter().any(|existing: &ReviewFile| existing.id == file.id) {
            return Err(GuidedReviewParseError::DuplicateFileBlock { header: header.to_owned() });
        }
        files.push(file);
        index = block_end;
    }

    if files.is_empty() {
        if let Some(line) = lines.iter().find(|line| !line.trim().is_empty()) {
            return Err(GuidedReviewParseError::UnrecognizedSource { line: (*line).to_owned() });
        }
    }

    Ok(ParsedDiff { files })
}

#[cfg(test)]
mod tests;
