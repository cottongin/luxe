//! Diff classification — conservative presentation-only noise labels.
//!
//! The parser retains canonical lines; these helpers only assign labels and must prefer semantic
//! output whenever equivalence is uncertain.

use super::{NoiseKind, ReviewHunk, ReviewLineKind};
use std::path::Path;

/// Returns a lightweight language label from a file extension.
pub(super) fn language_for_path(path: &Path) -> Option<String> {
    path.extension().and_then(|extension| extension.to_str()).map(str::to_owned)
}

/// Identifies lockfiles which stay visible but group as lower-signal review material.
pub(super) fn is_lockfile_path(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some(
            "Cargo.lock"
                | "Gemfile.lock"
                | "package-lock.json"
                | "pnpm-lock.yaml"
                | "poetry.lock"
                | "uv.lock"
                | "yarn.lock"
        )
    )
}

/// Classifies a hunk without changing its canonical source lines.
pub(super) fn classify_hunk_noise(
    hunk: &ReviewHunk,
    path: Option<&Path>,
    file_noise: Option<NoiseKind>,
) -> NoiseKind {
    file_noise.unwrap_or_else(|| {
        if is_line_comment_only(hunk, path) {
            NoiseKind::CommentOnly
        } else if is_safe_whitespace_only(hunk) {
            NoiseKind::FormattingOnly
        } else {
            NoiseKind::Semantic
        }
    })
}

fn is_line_comment_only(hunk: &ReviewHunk, path: Option<&Path>) -> bool {
    let Some(extension) = path.and_then(Path::extension).and_then(|value| value.to_str()) else {
        return false;
    };
    if !matches!(extension, "c" | "cc" | "cpp" | "go" | "java" | "js" | "rs" | "ts") {
        return false;
    }

    let mut changed = false;
    for line in &hunk.lines {
        if matches!(line.kind, ReviewLineKind::Addition | ReviewLineKind::Deletion) {
            changed = true;
            if !line.text.trim_start().starts_with("//") {
                return false;
            }
        }
    }
    changed
}

fn is_safe_whitespace_only(hunk: &ReviewHunk) -> bool {
    let deleted: Vec<_> = hunk
        .lines
        .iter()
        .filter(|line| line.kind == ReviewLineKind::Deletion)
        .map(|line| line.text.as_str())
        .collect();
    let added: Vec<_> = hunk
        .lines
        .iter()
        .filter(|line| line.kind == ReviewLineKind::Addition)
        .map(|line| line.text.as_str())
        .collect();

    !deleted.is_empty()
        && deleted.len() == added.len()
        && deleted.iter().zip(added).all(|(old, new)| is_safe_whitespace_line_change(old, new))
}

fn is_safe_whitespace_line_change(old: &str, new: &str) -> bool {
    if old == new || old.contains(['\'', '"', '`']) || new.contains(['\'', '"', '`']) {
        return false;
    }

    let old_chars: Vec<_> = old.chars().filter(|character| !character.is_whitespace()).collect();
    let new_chars: Vec<_> = new.chars().filter(|character| !character.is_whitespace()).collect();
    if old_chars != new_chars {
        return false;
    }

    let old_gaps = whitespace_gaps(old);
    let new_gaps = whitespace_gaps(new);
    old_gaps.iter().zip(new_gaps).enumerate().all(|(index, (old_gap, new_gap))| {
        let safe_boundary = index > 0
            && matches!(old_chars[index - 1], '{' | '}' | '(' | ')' | '[' | ']' | ',' | ';')
            || index < old_chars.len()
                && matches!(old_chars[index], '{' | '}' | '(' | ')' | '[' | ']' | ',' | ';');
        old_gap == &new_gap || safe_boundary
    })
}

fn whitespace_gaps(line: &str) -> Vec<String> {
    let mut gaps = vec![String::new()];
    for character in line.chars() {
        if character.is_whitespace() {
            if let Some(gap) = gaps.last_mut() {
                gap.push(character);
            }
        } else {
            gaps.push(String::new());
        }
    }
    gaps
}
