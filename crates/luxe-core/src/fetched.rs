//! Fetched repositories — the GitHub clones `web_fetch` keeps under `.luxe/gh-cache/`.
//!
//! A clone lives inside the project so the agent can read and search it without tripping the
//! external-directory guard. That makes it the one directory in the tree whose contents came from a
//! stranger rather than from the person running Luxe, so anything that would *run* repository code
//! on the user's behalf — a language server rooted there, a formatter loading its plugins — checks
//! [`contains`] and stays out. This module is the single answer to where that directory is and
//! whether a path is inside it.

use std::path::{Component, Path, PathBuf};

/// The cache's path under a project root, one component at a time.
const CACHE_DIR: [&str; 2] = [".luxe", "gh-cache"];

/// Where fetched repositories live for the project at `root`.
pub fn cache_dir(root: &Path) -> PathBuf {
    CACHE_DIR.iter().fold(root.to_path_buf(), |path, part| path.join(part))
}

/// Whether `path` is inside a fetched repository, under any project.
///
/// Matched on components rather than a prefix, because the callers hold a file path and nothing
/// else: a language server resolves its root from the file, not from the session. Case-insensitive,
/// on every platform: Windows filesystems are, and a clone reached as `.LUXE\GH-CACHE` there is still a
/// clone. On unix that also refuses a differently-cased directory, which is the safe direction.
pub fn contains(path: &Path) -> bool {
    let parts: Vec<String> = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(name) => Some(name.to_string_lossy().to_ascii_lowercase()),
            _ => None,
        })
        .collect();
    parts.windows(CACHE_DIR.len()).any(|window| window.iter().eq(CACHE_DIR.iter()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cache_is_two_directories_under_the_project() {
        let root = Path::new("/home/dev/project");
        assert_eq!(cache_dir(root), root.join(".luxe").join("gh-cache"));
        assert!(contains(&cache_dir(root).join("o").join("r").join("src").join("lib.rs")));
    }

    /// A session's own worktree also lives under `.luxe/`, and it is the user's code.
    #[test]
    fn only_the_cache_counts_and_not_the_rest_of_dot_luxe() {
        for path in [
            "/home/dev/project/src/lib.rs",
            "/home/dev/project/.luxe/worktrees/fix-1/src/lib.rs",
            "/home/dev/project/.luxe/sessions/a.jsonl",
            "/home/dev/project/gh-cache/x.rs",
            "/home/dev/project/.luxe-gh-cache/x.rs",
        ] {
            assert!(!contains(Path::new(path)), "{path}");
        }
    }

    #[test]
    fn a_relative_or_differently_cased_path_still_counts() {
        assert!(contains(Path::new(".luxe/gh-cache/o/r/Cargo.toml")));
        assert!(contains(Path::new("/p/.LUXE/GH-Cache/o/r/x.rs")));
    }
}
