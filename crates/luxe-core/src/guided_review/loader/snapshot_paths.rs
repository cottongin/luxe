//! Snapshot paths — taking the private snapshot roots back out of a `--no-index` diff's headers.

use super::GuidedReviewError;

/// Rewrites only Git's structural path headers from private snapshot roots to repository paths.
///
/// Headers are the lines between `diff --git` and a file's first hunk or binary patch, so payload
/// that happens to read like one, a removed `-- a/<root>/x`, keeps its bytes. A quoted old path on
/// `diff --git` is checked with the parser's own token reader, and every header path has its root
/// stripped from just after an opening quote, if any. Quoting stays as Git wrote it: the root is a
/// plain name, so it sits inside the quotes unescaped.
pub(super) fn normalize_no_index_snapshot_paths(
    source: &str,
    before_root: &str,
    after_root: &str,
) -> Result<String, GuidedReviewError> {
    let roots = [before_root, after_root];
    let mut normalized = String::with_capacity(source.len());
    let mut in_header = false;
    for segment in source.split_inclusive('\n') {
        let (line, ending) = segment.strip_suffix('\n').map_or((segment, ""), |line| (line, "\n"));
        let rewritten = if line.starts_with("diff --git ") {
            in_header = true;
            if !names_snapshot_roots(line, roots) {
                return Err(GuidedReviewError::MalformedGitOutput {
                    operation: "diff retained turn snapshots",
                    reason: "diff header does not identify two snapshot roots",
                });
            }
            unroot_header(line, roots)
        } else if !in_header {
            line.to_owned()
        } else if line.starts_with("@@ ") || line == "GIT binary patch" {
            in_header = false;
            line.to_owned()
        } else {
            unroot_header(line, roots)
        };
        normalized.push_str(&rewritten);
        normalized.push_str(ending);
    }
    Ok(normalized)
}

/// Whether a `diff --git` line's two paths, quoted or not, sit under the snapshot roots.
///
/// An unquoted path may hold a space Git leaves bare, so the new side is found by its prefix
/// rather than by splitting the line; a quoted old side is read as a token.
fn names_snapshot_roots(line: &str, roots: [&str; 2]) -> bool {
    let under = |path: &str, side: &str| {
        roots.iter().any(|root| path.starts_with(&format!("{side}/{root}/")))
    };
    let paths = line.strip_prefix("diff --git ").unwrap_or_default();
    let old_under = match paths.starts_with('"') {
        true => super::super::diff_paths::parse_git_path_token(paths, line)
            .is_ok_and(|(old, _)| under(&old, "a")),
        false => under(paths, "a"),
    };
    let new_under = roots.iter().any(|root| {
        line.contains(&format!(" b/{root}/")) || line.contains(&format!(" \"b/{root}/"))
    });
    old_under && new_under
}

/// A structural header line with the snapshot root taken off the start of each path it names:
/// two on `diff --git` and `Binary files`, one on `---` and `+++`. Anything else is kept.
fn unroot_header(line: &str, roots: [&str; 2]) -> String {
    if let Some(paths) = line.strip_prefix("diff --git ") {
        return format!("diff --git {}", unroot_pair(paths, " ", roots));
    }
    if let Some(paths) = line.strip_prefix("Binary files ").and_then(|p| p.strip_suffix(" differ"))
    {
        return format!("Binary files {} differ", unroot_pair(paths, " and ", roots));
    }
    match (line.strip_prefix("--- "), line.strip_prefix("+++ ")) {
        (Some(path), _) => format!("--- {}", unroot_path(path, "a", roots)),
        (_, Some(path)) => format!("+++ {}", unroot_path(path, "b", roots)),
        _ => line.to_owned(),
    }
}

/// `path` with `<side>/<root>/` taken back to `<side>/` at its start, quoted or not.
fn unroot_path(path: &str, side: &str, roots: [&str; 2]) -> String {
    for root in roots {
        for quote in ["", "\""] {
            if let Some(rest) = path.strip_prefix(&format!("{quote}{side}/{root}/")) {
                return format!("{quote}{side}/{rest}");
            }
        }
    }
    path.to_owned()
}

/// Two paths joined by `separator`, each unrooted at its own start.
///
/// An unquoted path may hold the separator itself, so where the new path begins is the candidate
/// that names the same file as the old one, as it does for anything but a rename; failing that,
/// the first.
fn unroot_pair(paths: &str, separator: &str, roots: [&str; 2]) -> String {
    let old_side = unroot_path(paths, "a", roots);
    let bare = |path: &str| {
        let path = path.strip_prefix('"').unwrap_or(path);
        path.strip_prefix("a/").or_else(|| path.strip_prefix("b/")).unwrap_or(path).to_owned()
    };
    let starts: Vec<usize> = old_side
        .match_indices(separator)
        .map(|(at, _)| at)
        .filter(|&at| {
            unroot_path(&old_side[at + separator.len()..], "b", roots)
                != old_side[at + separator.len()..]
        })
        .collect();
    let at = starts
        .iter()
        .copied()
        .find(|&at| {
            bare(&old_side[..at])
                == bare(&unroot_path(&old_side[at + separator.len()..], "b", roots))
        })
        .or(starts.first().copied());
    match at {
        Some(at) => {
            let new_side = unroot_path(&old_side[at + separator.len()..], "b", roots);
            format!("{}{separator}{new_side}", &old_side[..at])
        }
        None => old_side,
    }
}
