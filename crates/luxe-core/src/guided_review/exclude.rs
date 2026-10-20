//! Review exclusions — what a review leaves out, and why that is safe to do.
//!
//! Two things a repository carries that nobody reviews: luxe's own `.luxe/` directory (sessions,
//! logs, caches, screenshots, task and review memory — byproducts of the tool running, not changes
//! to the project), and whatever bookkeeping a given project keeps that its reviewers already
//! ignore. The first is excluded always and needs no configuration; the second is `review.exclude`.
//!
//! Exclusions are **counted**, never silent. A review that quietly shows less than the diff is the
//! failure mode this whole surface exists to avoid, so the count travels with the review and the
//! Brief says it out loud.

use serde::{Deserialize, Serialize};

/// luxe's own project directory. Never reviewed, never configurable.
pub const LUXE_PROJECT_DIR: &str = ".luxe";

/// What a review leaves out of the diff it presents.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewExclusions {
    patterns: Vec<String>,
}

impl ReviewExclusions {
    /// Builds the rule set from configured globs. Blank entries are dropped.
    pub fn new(patterns: &[String]) -> Self {
        Self {
            patterns: patterns
                .iter()
                .map(|pattern| pattern.trim().to_owned())
                .filter(|pattern| !pattern.is_empty())
                .collect(),
        }
    }

    /// Whether this repository-relative path is left out of reviews.
    pub fn excludes(&self, path: &str) -> bool {
        is_luxe_artifact(path) || self.patterns.iter().any(|pattern| matches_glob(pattern, path))
    }

    /// Whether anything beyond luxe's own directory is configured.
    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }
}

/// Whether a path is a byproduct of luxe running rather than a change to the project.
pub fn is_luxe_artifact(path: &str) -> bool {
    path == LUXE_PROJECT_DIR
        || path.starts_with(&format!("{LUXE_PROJECT_DIR}/"))
        || path.starts_with(&format!("{LUXE_PROJECT_DIR}\\"))
}

/// Matches a repository-relative path against one glob.
///
/// Supports `?` (one character, never `/`), `*` (any run within one segment) and `**` (any run
/// across segments). A pattern ending in `/` matches that directory and everything under it, which
/// is how people write `docs/design/` without thinking about it. Deliberately small: a review's
/// exclusion list is a handful of paths a human typed, not a search index.
pub fn matches_glob(pattern: &str, path: &str) -> bool {
    if let Some(directory) = pattern.strip_suffix('/') {
        return path == directory || path.starts_with(&format!("{directory}/"));
    }
    // A bare directory name with no wildcards means the directory and its contents.
    if !pattern.contains(['*', '?']) {
        return path == pattern || path.starts_with(&format!("{pattern}/"));
    }
    glob_match(pattern.as_bytes(), path.as_bytes())
}

/// Recursive glob match over bytes, segment-aware for `*` versus `**`.
///
/// Recursion rather than single-point backtracking: two star kinds need two resume points, and a
/// review's exclusion list is a handful of short paths a human typed, not a search index.
fn glob_match(pattern: &[u8], path: &[u8]) -> bool {
    match pattern.first() {
        None => path.is_empty(),
        Some(b'*') => {
            let crosses = pattern.get(1) == Some(&b'*');
            let rest = if crosses { &pattern[2..] } else { &pattern[1..] };
            // `a/**/b` also matches `a/b`, which is what people expect it to mean.
            if crosses && rest.first() == Some(&b'/') && glob_match(&rest[1..], path) {
                return true;
            }
            for skip in 0..=path.len() {
                if glob_match(rest, &path[skip..]) {
                    return true;
                }
                if !crosses && path.get(skip) == Some(&b'/') {
                    return false;
                }
            }
            false
        }
        Some(b'?') => {
            matches!(path.first(), Some(byte) if *byte != b'/')
                && glob_match(&pattern[1..], &path[1..])
        }
        Some(byte) => {
            matches!(path.first(), Some(first) if first == byte)
                && glob_match(&pattern[1..], &path[1..])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn luxes_own_directory_is_always_excluded() {
        let exclusions = ReviewExclusions::default();
        assert!(exclusions.excludes(".luxe"));
        assert!(exclusions.excludes(".luxe/.gitignore"));
        assert!(exclusions.excludes(".luxe/screenshots/shot-1.png"));
        assert!(exclusions.excludes(".luxe/sessions/abc/def.jsonl"));
        assert!(exclusions.is_empty(), "and it is not a configured pattern");
        // A project file that merely starts with the same letters is not ours.
        assert!(!exclusions.excludes(".luxerc"));
        assert!(!exclusions.excludes("src/luxe.rs"));
    }

    #[test]
    fn a_bare_path_matches_the_file_or_the_directory_it_names() {
        let exclusions = ReviewExclusions::new(&["PROGRESS.md".into(), "docs/design".into()]);
        assert!(exclusions.excludes("PROGRESS.md"));
        assert!(exclusions.excludes("docs/design"));
        assert!(exclusions.excludes("docs/design/phase-01.md"));
        assert!(!exclusions.excludes("docs/design.md"));
        assert!(!exclusions.excludes("src/PROGRESS.md"));
    }

    #[test]
    fn a_trailing_slash_reads_as_the_directory_people_mean() {
        let exclusions = ReviewExclusions::new(&[".pi/".into()]);
        assert!(exclusions.excludes(".pi"));
        assert!(exclusions.excludes(".pi/memory/memory.md"));
        assert!(!exclusions.excludes(".pilot/x.rs"));
    }

    #[test]
    fn stars_stay_inside_a_segment_and_double_stars_cross_them() {
        assert!(matches_glob("*.md", "README.md"));
        assert!(!matches_glob("*.md", "docs/README.md"), "one star never crosses a slash");
        assert!(matches_glob("docs/**/*.md", "docs/design/phase-01.md"));
        assert!(matches_glob("docs/**", "docs/design/phase-01.md"));
        assert!(matches_glob("**/*.snap", "crates/x/tests/a.snap"));
        assert!(!matches_glob("docs/**", "src/docs/x.md"));
        // `**/` matches zero segments too.
        assert!(matches_glob("docs/**/x.md", "docs/x.md"));
        assert!(matches_glob("shot?.png", "shot1.png"));
        assert!(!matches_glob("shot?.png", "shot12.png"));
    }

    #[test]
    fn blank_patterns_are_dropped_rather_than_matching_everything() {
        let exclusions = ReviewExclusions::new(&["".into(), "   ".into()]);
        assert!(exclusions.is_empty());
        assert!(!exclusions.excludes("src/lib.rs"));
    }
}
