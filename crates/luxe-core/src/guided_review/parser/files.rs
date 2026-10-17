//! Files — one file's section of a diff: its paths, mode, status, and text or non-text content.

use super::*;

pub(super) fn parse_text_file(
    lines: &[&str],
) -> Result<Option<ReviewFile>, GuidedReviewParseError> {
    let Some(old_index) = lines.iter().position(|line| line.starts_with("--- ")) else {
        return Ok(None);
    };
    let Some(new_line) = lines.get(old_index + 1).filter(|line| line.starts_with("+++ ")) else {
        return Err(GuidedReviewParseError::InvalidFileHeader {
            line: lines.get(old_index + 1).copied().unwrap_or_default().to_owned(),
        });
    };

    let old_path = parse_path(lines[old_index], "--- ")?;
    let new_path = parse_path(new_line, "+++ ")?;
    let file_id = FileId::from_path_lineage(old_path.as_deref(), new_path.as_deref());
    let mut hunks = Vec::new();
    let mut index = old_index + 2;

    while index < lines.len() {
        if lines[index].starts_with("@@ ") {
            let (hunk, next) = parse_hunk(lines, index, &file_id)?;
            hunks.push(hunk);
            index = next;
        } else {
            index += 1;
        }
    }

    // Only an addition or a deletion may be empty; a modification with no hunk changed nothing
    if hunks.is_empty() && old_path.is_some() && new_path.is_some() {
        return Err(GuidedReviewParseError::MissingTextHunk { line: lines[old_index].to_owned() });
    }

    let generated =
        hunks.iter().flat_map(|hunk| &hunk.lines).any(|line| line.text.contains("@generated"));
    let path = new_path.as_deref().or(old_path.as_deref());
    let file_noise = if generated {
        Some(NoiseKind::Generated)
    } else if path.is_some_and(is_lockfile_path) {
        Some(NoiseKind::Lockfile)
    } else {
        None
    };
    for hunk in &mut hunks {
        hunk.noise = classify_hunk_noise(hunk, path, file_noise);
    }

    let additions = hunks
        .iter()
        .map(|hunk| {
            hunk.lines.iter().filter(|line| line.kind == ReviewLineKind::Addition).count() as u64
        })
        .sum();
    let deletions = hunks
        .iter()
        .map(|hunk| {
            hunk.lines.iter().filter(|line| line.kind == ReviewLineKind::Deletion).count() as u64
        })
        .sum();
    let status = text_file_status(lines, &old_path, &new_path)?;

    Ok(Some(ReviewFile {
        id: file_id,
        language: new_path.as_deref().or(old_path.as_deref()).and_then(language_for_path),
        old_path,
        new_path,
        status,
        additions: Some(additions),
        deletions: Some(deletions),
        hunks,
        binary: false,
        generated,
        non_text: None,
    }))
}

/// A file added or deleted empty, which Git writes as its mode alone: no `---`/`+++`, no hunk.
pub(super) fn parse_empty_file(
    header: &str,
    lines: &[&str],
) -> Result<Option<ReviewFile>, GuidedReviewParseError> {
    let created = lines.iter().any(|line| line.starts_with("new file mode "));
    let deleted = lines.iter().any(|line| line.starts_with("deleted file mode "));
    if created == deleted || lines.iter().any(|line| line.starts_with("@@ ")) {
        return Ok(None);
    }
    let (old_path, new_path) = existence_from_mode(lines, parse_git_header_paths(header)?);
    let status = text_file_status(lines, &old_path, &new_path)?;
    Ok(Some(ReviewFile {
        id: FileId::from_path_lineage(old_path.as_deref(), new_path.as_deref()),
        language: new_path.as_deref().or(old_path.as_deref()).and_then(language_for_path),
        old_path,
        new_path,
        status,
        additions: Some(0),
        deletions: Some(0),
        hunks: Vec::new(),
        binary: false,
        generated: false,
        non_text: None,
    }))
}

/// `paths` with the side a `new file mode` or `deleted file mode` line says is absent taken away.
pub(super) fn existence_from_mode(
    lines: &[&str],
    (old_path, new_path): (Option<PathBuf>, Option<PathBuf>),
) -> (Option<PathBuf>, Option<PathBuf>) {
    let mode = |prefix: &str| lines.iter().any(|line| line.starts_with(prefix));
    match (mode("new file mode "), mode("deleted file mode ")) {
        (true, false) => (None, new_path),
        (false, true) => (old_path, None),
        _ => (old_path, new_path),
    }
}

pub(super) fn text_file_status(
    lines: &[&str],
    old_path: &Option<PathBuf>,
    new_path: &Option<PathBuf>,
) -> Result<ReviewFileStatus, GuidedReviewParseError> {
    if let Some((status, _, _)) = path_change_details(lines)? {
        return Ok(status);
    }
    match (old_path, new_path) {
        (None, Some(_)) => Ok(ReviewFileStatus::Added),
        (Some(_), None) => Ok(ReviewFileStatus::Deleted),
        (Some(_), Some(_)) => Ok(ReviewFileStatus::Modified),
        (None, None) => Err(GuidedReviewParseError::InvalidFileHeader {
            line: "both file paths are /dev/null".into(),
        }),
    }
}

pub(super) fn path_change_details(
    lines: &[&str],
) -> Result<Option<(ReviewFileStatus, NonTextChangeKind, String)>, GuidedReviewParseError> {
    let renamed = (
        lines.iter().find_map(|line| line.strip_prefix("rename from ")),
        lines.iter().find_map(|line| line.strip_prefix("rename to ")),
    );
    let copied = (
        lines.iter().find_map(|line| line.strip_prefix("copy from ")),
        lines.iter().find_map(|line| line.strip_prefix("copy to ")),
    );
    if renamed.0.is_some() != renamed.1.is_some()
        || copied.0.is_some() != copied.1.is_some()
        || (renamed.0.is_some() && copied.0.is_some())
    {
        return Err(GuidedReviewParseError::InvalidFileHeader {
            line: "incomplete or conflicting rename/copy metadata".into(),
        });
    }
    if let (Some(from), Some(_to)) = renamed {
        return Ok(Some((
            ReviewFileStatus::Renamed,
            NonTextChangeKind::Rename,
            format!("renamed from {from}"),
        )));
    }
    if let (Some(from), Some(_to)) = copied {
        return Ok(Some((
            ReviewFileStatus::Copied,
            NonTextChangeKind::Copy,
            format!("copied from {from}"),
        )));
    }
    Ok(None)
}

pub(super) fn parse_non_text_file(
    header: &str,
    lines: &[&str],
) -> Result<Option<ReviewFile>, GuidedReviewParseError> {
    let has_hunk = lines.iter().any(|line| line.starts_with("@@ "));
    // Git's own line is `Binary files /dev/null and b/shots/00.png differ`, which repeats the
    // path the row already carries and says nothing a reviewer can use. The status letter beside
    // it already distinguishes added from modified.
    let detail = if lines.iter().any(|line| line.starts_with("Binary files "))
        || lines.contains(&"GIT binary patch")
    {
        Some((
            ReviewFileStatus::Modified,
            NonTextChangeKind::Binary,
            "binary content".to_owned(),
            true,
        ))
    } else if lines.iter().any(|line| {
        line.starts_with("-Subproject commit ") || line.starts_with("+Subproject commit ")
    }) {
        Some((
            ReviewFileStatus::Modified,
            NonTextChangeKind::Submodule,
            "submodule commit changed".to_owned(),
            false,
        ))
    } else if !has_hunk {
        if let Some((status, kind, summary)) = path_change_details(lines)? {
            Some((status, kind, summary, false))
        } else {
            let old_mode = lines.iter().find_map(|line| line.strip_prefix("old mode "));
            let new_mode = lines.iter().find_map(|line| line.strip_prefix("new mode "));
            match (old_mode, new_mode) {
                (Some(old_mode), Some(new_mode)) => Some((
                    ReviewFileStatus::Modified,
                    NonTextChangeKind::ModeOnly,
                    format!("mode {old_mode} \u{2192} {new_mode}"),
                    false,
                )),
                _ => None,
            }
        }
    } else {
        None
    };
    let Some((status, kind, summary, binary)) = detail else {
        return Ok(None);
    };

    let (old_path, new_path) = non_text_paths(header, lines, kind)?;
    let (old_path, new_path, status) = if kind == NonTextChangeKind::Binary {
        // Without `---`/`+++`, both paths came from `diff --git`; the mode says which exists
        let (old_path, new_path) = existence_from_mode(lines, (old_path, new_path));
        let status = text_file_status(lines, &old_path, &new_path)?;
        (old_path, new_path, status)
    } else {
        (old_path, new_path, status)
    };
    let file_id = FileId::from_path_lineage(old_path.as_deref(), new_path.as_deref());
    Ok(Some(ReviewFile {
        id: file_id,
        language: new_path.as_deref().or(old_path.as_deref()).and_then(language_for_path),
        old_path,
        new_path,
        status,
        additions: None,
        deletions: None,
        hunks: Vec::new(),
        binary,
        generated: false,
        non_text: Some(NonTextChange { kind, summary }),
    }))
}

pub(super) fn non_text_paths(
    header: &str,
    lines: &[&str],
    kind: NonTextChangeKind,
) -> Result<(Option<PathBuf>, Option<PathBuf>), GuidedReviewParseError> {
    if kind == NonTextChangeKind::Binary {
        if let Some(old_index) = lines.iter().position(|line| line.starts_with("--- ")) {
            let new_line = lines
                .get(old_index + 1)
                .filter(|line| line.starts_with("+++ "))
                .ok_or_else(|| GuidedReviewParseError::InvalidFileHeader {
                    line: lines.get(old_index + 1).copied().unwrap_or_default().to_owned(),
                })?;
            return Ok((parse_path(lines[old_index], "--- ")?, parse_path(new_line, "+++ ")?));
        }
    }
    parse_git_header_paths(header)
}
