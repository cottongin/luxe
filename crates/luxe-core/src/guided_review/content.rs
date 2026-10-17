//! Immutable review file and hunk content.
//!
//! A Guided Review owns canonical source lines. Presentation may fold a classification, but it
//! never mutates or omits these source records.

use super::{FileId, HunkId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

/// A one-based line range in one side of a unified diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineRange {
    /// First line number, or zero when this side has no lines.
    pub start: u32,
    /// Number of represented lines.
    pub count: u32,
}

impl LineRange {
    /// Builds a range from one unified-diff side.
    pub const fn new(start: u32, count: u32) -> Self {
        Self { start, count }
    }
}

/// Status of a reviewed file, including Git copy detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewFileStatus {
    /// Newly added file.
    Added,
    /// Existing file with text or metadata changes.
    Modified,
    /// Deleted file.
    Deleted,
    /// File with a preserved path lineage change.
    Renamed,
    /// File copied from another path.
    Copied,
    /// Eligible untracked file included in the captured source.
    Untracked,
}

/// A non-text change that remains reviewable without invented line counts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NonTextChange {
    /// Exact non-text change category.
    pub kind: NonTextChangeKind,
    /// Display-safe metadata from the canonical source header.
    pub summary: String,
}

/// Explicit non-text review categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NonTextChangeKind {
    /// Binary content differs.
    Binary,
    /// Git submodule object identifier differs.
    Submodule,
    /// File mode changed without text content changes.
    ModeOnly,
    /// A path lineage changed without text content changes.
    Rename,
    /// A file was copied without text content changes.
    Copy,
}

/// One immutable file in a captured review source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewFile {
    /// Stable path-lineage identifier.
    pub id: FileId,
    /// Path before this change, absent for new/untracked files.
    pub old_path: Option<PathBuf>,
    /// Path after this change, absent for deleted files.
    pub new_path: Option<PathBuf>,
    /// File status preserved from the canonical source.
    pub status: ReviewFileStatus,
    /// Detected source language when known.
    pub language: Option<String>,
    /// Text additions, or `None` for non-text/unknown counts.
    pub additions: Option<u64>,
    /// Text deletions, or `None` for non-text/unknown counts.
    pub deletions: Option<u64>,
    /// Immutable text hunks in source order.
    pub hunks: Vec<ReviewHunk>,
    /// Whether the canonical source identified binary content.
    pub binary: bool,
    /// Whether generated-file evidence was found.
    pub generated: bool,
    /// Explicit non-text detail, when no line-level review exists.
    pub non_text: Option<NonTextChange>,
}

impl ReviewFile {
    /// Returns whether this file has source lines that can be rendered as a text diff.
    pub fn is_text(&self) -> bool {
        !self.binary && self.non_text.is_none()
    }
}

/// A source-anchored contiguous block of review lines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewHunk {
    /// Stable identifier derived from immutable changed content.
    pub id: HunkId,
    /// Old-side source line range.
    pub old_range: LineRange,
    /// New-side source line range.
    pub new_range: LineRange,
    /// Optional unified-diff trailing heading.
    pub heading: Option<String>,
    /// Attributed semantic unit when structural analysis can identify one.
    pub symbol: Option<SymbolContext>,
    /// Complete immutable source lines for this hunk.
    pub lines: Vec<ReviewLine>,
    /// Presentation classification that never changes source coverage.
    pub noise: NoiseKind,
}

/// Best-effort semantic attribution for a hunk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolContext {
    /// Structural kind, such as `function` or `struct`.
    pub kind: String,
    /// Symbol name when the language adapter found one.
    pub name: Option<String>,
}

/// A single line with anchors on each applicable source side.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewLine {
    /// Old-side line number for context/deletions.
    pub old_line: Option<u32>,
    /// New-side line number for context/additions.
    pub new_line: Option<u32>,
    /// Unified-diff role of this line.
    pub kind: ReviewLineKind,
    /// Exact UTF-8 line text without its unified-diff prefix.
    pub text: String,
}

impl ReviewLine {
    /// Returns a stable content digest for conservative finding-anchor remapping.
    pub fn context_hash(&self) -> String {
        let mut digest = Sha256::new();
        digest.update(b"luxe.review.finding-line.v1\\0");
        digest.update(match self.kind {
            ReviewLineKind::Context => b"context\\0".as_slice(),
            ReviewLineKind::Deletion => b"deletion\\0".as_slice(),
            ReviewLineKind::Addition => b"addition\\0".as_slice(),
            ReviewLineKind::NoNewlineMarker => b"no-newline\\0".as_slice(),
        });
        digest.update(self.text.as_bytes());
        format!("sha256:{}", base16ct::lower::encode_string(&digest.finalize()))
    }
}

/// Role of a line within a unified-diff hunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewLineKind {
    /// Unchanged context line.
    Context,
    /// Line present only in the old source.
    Deletion,
    /// Line present only in the new source.
    Addition,
    /// `\\ No newline at end of file` source marker.
    NoNewlineMarker,
}

/// Conservative presentation classification for a complete hunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoiseKind {
    /// Source change is semantically meaningful or has not been proven otherwise.
    Semantic,
    /// Token-equivalent formatting change.
    FormattingOnly,
    /// Comment-only change.
    CommentOnly,
    /// Generated-file content.
    Generated,
    /// Lockfile content.
    Lockfile,
    /// Conservative mechanical change, such as an exact rename.
    Mechanical,
    /// Classification lacks sufficient evidence.
    Unknown,
}

impl NoiseKind {
    /// Whether a change of this kind can be folded out of the reading list on its own evidence.
    ///
    /// The property belongs to the classification rather than to any one consumer, because two of
    /// them ask it: the budget, to account for a hunk, and the attention list, to decide a file is
    /// already explained and needs no call-out. Answering it in each place separately is how the
    /// same change ends up counted as churn in one panel and flagged as a risk in the other.
    ///
    /// `CommentOnly` is deliberately absent. A comment is prose a person will act on, and a wrong
    /// one outlives the code it describes.
    pub const fn is_foldable(self) -> bool {
        matches!(self, Self::FormattingOnly | Self::Generated | Self::Lockfile | Self::Mechanical)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn non_text_files_keep_counts_unknown_and_have_no_text_surface() {
        let file = ReviewFile {
            id: FileId::from_path_lineage(Some(Path::new("a.bin")), Some(Path::new("a.bin"))),
            old_path: Some("a.bin".into()),
            new_path: Some("a.bin".into()),
            status: ReviewFileStatus::Modified,
            language: None,
            additions: None,
            deletions: None,
            hunks: Vec::new(),
            binary: true,
            generated: false,
            non_text: Some(NonTextChange {
                kind: NonTextChangeKind::Binary,
                summary: "Binary files differ".into(),
            }),
        };

        let encoded = serde_json::to_string(&file).unwrap();
        assert_eq!(serde_json::from_str::<ReviewFile>(&encoded).unwrap(), file);
        assert!(!file.is_text());
        assert!(file.additions.is_none());
        assert!(file.deletions.is_none());
    }

    #[test]
    fn text_hunk_preserves_both_source_line_anchors() {
        let line = ReviewLine {
            old_line: Some(12),
            new_line: Some(12),
            kind: ReviewLineKind::Context,
            text: "unchanged".into(),
        };

        assert_eq!(line.old_line, line.new_line);
        assert_eq!(line.text, "unchanged");
    }

    #[test]
    fn line_context_hash_keeps_the_hex_spelling_anchors_were_recorded_with() {
        let context = ReviewLine {
            old_line: Some(1),
            new_line: Some(9),
            kind: ReviewLineKind::Context,
            text: "same text".into(),
        };

        // Literal computed outside this crate: a finding anchored in a saved review is remapped by
        // comparing this string, so a change in how the digest is spelled orphans every anchor.
        assert_eq!(
            context.context_hash(),
            "sha256:2fea11da6b3de0b3fdeb4888ea207a8b6723630b68bff58e42fc5c2525a1f8df"
        );
    }

    #[test]
    fn line_context_hash_is_stable_but_distinguishes_diff_roles() {
        let context = ReviewLine {
            old_line: Some(1),
            new_line: Some(9),
            kind: ReviewLineKind::Context,
            text: "same text".into(),
        };
        let deletion = ReviewLine { kind: ReviewLineKind::Deletion, ..context.clone() };

        assert_eq!(context.context_hash(), context.context_hash());
        assert_ne!(context.context_hash(), deletion.context_hash());
        assert!(context.context_hash().starts_with("sha256:"));
    }
}
