//! Untracked source rendering — deterministic whole-file diffs for captured review sources.
//!
//! Working-tree capture owns policy and file I/O; this module only turns already-approved bytes
//! into canonical unified-diff fragments. Two shapes: a file that is entirely new, and a file that
//! is entirely gone — neither has a diff of its own, and Git cannot be asked for one where there
//! is no repository to ask.

use sha2::{Digest, Sha256};
use std::path::Path;

/// Renders an untracked text file as the addition a diff reader expects to see.
///
/// Exposed for [`crate::file_diff`], which asks the same question of one file that working-tree
/// capture asks of the whole worktree: a path git does not track has no diff of its own, and the
/// only honest rendering of it is every line, added.
pub fn render_untracked_addition(relative_path: &Path, contents: &str) -> String {
    render_text_addition(relative_path, contents)
}

/// Renders a file that is gone as the deletion of everything it held.
///
/// The mirror of [`render_untracked_addition`], for [`crate::file_diff`]'s one case Git cannot
/// answer: a file outside any repository that this session removed. All that is left of it is the
/// copy retained before the change, and every line of that copy is a deletion.
pub fn render_whole_file_deletion(relative_path: &Path, contents: &str) -> String {
    let path = relative_path.to_string_lossy();
    let old_path = git_path_token(&format!("a/{path}"));
    let new_path = git_path_token(&format!("b/{path}"));
    let mut diff = format!(
        "diff --git {old_path} {new_path}\ndeleted file mode 100644\n--- {old_path}\n+++ /dev/null\n"
    );
    if contents.is_empty() {
        return diff;
    }
    let lines = contents.strip_suffix('\n').unwrap_or(contents).split('\n').collect::<Vec<_>>();
    diff.push_str(&format!("@@ -{} +0,0 @@\n", git_range(1, lines.len())));
    for line in lines {
        diff.push('-');
        diff.push_str(line);
        diff.push('\n');
    }
    if !contents.ends_with('\n') {
        diff.push_str("\\ No newline at end of file\n");
    }
    diff
}

/// Renders an explicit binary addition without exposing its original bytes.
pub(super) fn render_binary_addition(relative_path: &Path, contents: &[u8]) -> String {
    let path = relative_path.to_string_lossy();
    let old_path = git_path_token(&format!("a/{path}"));
    let new_path = git_path_token(&format!("b/{path}"));
    let digest = base16ct::lower::encode_string(&Sha256::digest(contents));
    format!(
        "diff --git {old_path} {new_path}\nnew file mode 100644\n--- /dev/null\n+++ {new_path}\nBinary files /dev/null and {new_path} differ\nBinary source sha256 {digest}\n"
    )
}

/// Renders an untracked UTF-8 file as a deterministic unified-diff addition.
pub(super) fn render_text_addition(relative_path: &Path, contents: &str) -> String {
    let path = relative_path.to_string_lossy();
    let old_path = git_path_token(&format!("a/{path}"));
    let new_path = git_path_token(&format!("b/{path}"));
    let mut diff = format!(
        "diff --git {old_path} {new_path}\nnew file mode 100644\n--- /dev/null\n+++ {new_path}\n"
    );
    if contents.is_empty() {
        return diff;
    }
    let lines = contents.strip_suffix('\n').unwrap_or(contents).split('\n').collect::<Vec<_>>();
    diff.push_str(&format!("@@ -0,0 +{} @@\n", git_range(1, lines.len())));
    for line in lines {
        diff.push('+');
        diff.push_str(line);
        diff.push('\n');
    }
    if !contents.ends_with('\n') {
        diff.push_str("\\ No newline at end of file\n");
    }
    diff
}

/// A hunk range the way git writes it: the count left off when it is one. The header is part of a
/// hunk's identity, so a file reviewed untracked and then committed only reads as the same change
/// when both headers are spelled alike.
fn git_range(start: usize, count: usize) -> String {
    if count == 1 {
        start.to_string()
    } else {
        format!("{start},{count}")
    }
}

fn git_path_token(path: &str) -> String {
    if path.chars().all(|character| !character.is_whitespace() && !matches!(character, '"' | '\\'))
    {
        return path.into();
    }
    let escaped = path
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t");
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_addition_keeps_the_hex_spelling_captured_sources_carry() {
        let rendered = render_binary_addition(Path::new("assets/logo.png"), &[0, 1, 2, 255]);

        // Literal computed outside this crate: the rendered fragment is the captured source a
        // review is pinned to, so the digest line is part of a persisted artifact.
        assert!(rendered.contains(
            "Binary source sha256 3d1f57c984978ef98a18378c8166c1cb8ede02c03eeb6aee7e2f121dfeee3e56\n"
        ));
    }

    /// The ranges are spelled as git spells them — a count of one left off — since the header is
    /// part of a hunk's identity. A capture already pinned keeps the bytes it was stored with.
    #[test]
    fn ranges_are_spelled_as_git_spells_them() {
        let one = render_untracked_addition(Path::new("a.md"), "one\n");
        assert!(one.contains("\n@@ -0,0 +1 @@\n+one\n"), "{one}");
        let two = render_untracked_addition(Path::new("a.md"), "one\ntwo\n");
        assert!(two.contains("\n@@ -0,0 +1,2 @@\n"), "{two}");
        let gone = render_whole_file_deletion(Path::new("a.md"), "one\n");
        assert!(gone.contains("\n@@ -1 +0,0 @@\n-one\n"), "{gone}");
        let both = render_whole_file_deletion(Path::new("a.md"), "one\ntwo");
        assert!(both.contains("\n@@ -1,2 +0,0 @@\n"), "{both}");
        assert!(both.ends_with("\\ No newline at end of file\n"), "{both}");
    }
}
