//! Diff paths — reading the path tokens Git writes in a diff's headers, quoted or not.
//!
//! Shared by the parser, which reads a captured source, and by snapshot normalization, which has
//! to recognize a structural header's path before it rewrites it.

use std::path::{Component, PathBuf};

use super::parser::GuidedReviewParseError;

/// Reads one path token from the start of `input`: Git's C-quoted form, escapes and octal bytes
/// decoded, or a bare word. Returns the path and what follows it.
pub(super) fn parse_git_path_token<'a>(
    input: &'a str,
    source_line: &str,
) -> Result<(String, &'a str), GuidedReviewParseError> {
    if !input.starts_with('"') {
        let end = input.find(char::is_whitespace).unwrap_or(input.len());
        let path = &input[..end];
        if path.is_empty() {
            return Err(GuidedReviewParseError::InvalidFileHeader { line: source_line.to_owned() });
        }
        return Ok((path.to_owned(), &input[end..]));
    }

    let bytes = input.as_bytes();
    let mut decoded = Vec::new();
    let mut index = 1;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => {
                let path = String::from_utf8(decoded).map_err(|_| {
                    GuidedReviewParseError::InvalidFileHeader { line: source_line.to_owned() }
                })?;
                return Ok((path, &input[index + 1..]));
            }
            b'\\' => {
                index += 1;
                let escaped = *bytes.get(index).ok_or_else(|| {
                    GuidedReviewParseError::InvalidFileHeader { line: source_line.to_owned() }
                })?;
                match escaped {
                    b'\\' | b'"' => decoded.push(escaped),
                    b'n' => decoded.push(b'\n'),
                    b'r' => decoded.push(b'\r'),
                    b't' => decoded.push(b'\t'),
                    b'0'..=b'3' => {
                        let octal = bytes.get(index..index + 3).ok_or_else(|| {
                            GuidedReviewParseError::InvalidFileHeader {
                                line: source_line.to_owned(),
                            }
                        })?;
                        if !octal.iter().all(|digit| matches!(digit, b'0'..=b'7')) {
                            return Err(GuidedReviewParseError::InvalidFileHeader {
                                line: source_line.to_owned(),
                            });
                        }
                        decoded
                            .push((octal[0] - b'0') * 64 + (octal[1] - b'0') * 8 + octal[2] - b'0');
                        index += 2;
                    }
                    _ => {
                        return Err(GuidedReviewParseError::InvalidFileHeader {
                            line: source_line.to_owned(),
                        });
                    }
                }
            }
            byte => decoded.push(byte),
        }
        index += 1;
    }
    Err(GuidedReviewParseError::InvalidFileHeader { line: source_line.to_owned() })
}

/// The two paths a `diff --git` line names, each `None` for `/dev/null`.
pub(super) fn parse_git_header_paths(
    header: &str,
) -> Result<(Option<PathBuf>, Option<PathBuf>), GuidedReviewParseError> {
    let Some(paths) = header.strip_prefix("diff --git ") else {
        return Err(GuidedReviewParseError::InvalidFileHeader { line: header.to_owned() });
    };
    if let Some((old, new)) = same_name_halves(paths) {
        return Ok((normalize_path(old, header)?, normalize_path(new, header)?));
    }
    let (old, remainder) = parse_git_path_token(paths, header)?;
    let remainder = remainder
        .strip_prefix(' ')
        .ok_or_else(|| GuidedReviewParseError::InvalidFileHeader { line: header.to_owned() })?;
    let (new, remainder) = parse_git_path_token(remainder, header)?;
    if !remainder.is_empty() {
        return Err(GuidedReviewParseError::InvalidFileHeader { line: header.to_owned() });
    }
    Ok((normalize_path(&old, header)?, normalize_path(&new, header)?))
}

/// `a/<name> b/<name>` cut in the middle, the way `git apply` reads a header whose unquoted name
/// may hold a space: both halves name the same file, as they do for anything but a rename.
fn same_name_halves(paths: &str) -> Option<(&str, &str)> {
    let name_len = paths.len().checked_sub(5)? / 2;
    let split = 2 + name_len;
    let (old, new) = (paths.get(..split)?, paths.get(split + 1..)?);
    let same =
        old.strip_prefix("a/")? == new.strip_prefix("b/")? && paths.as_bytes()[split] == b' ';
    (same && !paths.starts_with('"')).then_some((old, new))
}

pub(super) fn parse_path(
    line: &str,
    prefix: &str,
) -> Result<Option<PathBuf>, GuidedReviewParseError> {
    let raw_path = line
        .strip_prefix(prefix)
        .ok_or_else(|| GuidedReviewParseError::InvalidFileHeader { line: line.to_owned() })?;
    if !raw_path.starts_with('"') {
        let (path, remainder) = raw_path.split_once('\t').unwrap_or((raw_path, ""));
        if path.is_empty() || (!remainder.is_empty() && !remainder.starts_with('\t')) {
            return Err(GuidedReviewParseError::InvalidFileHeader { line: line.to_owned() });
        }
        return normalize_path(path, line);
    }
    let (path, remainder) = parse_git_path_token(raw_path, line)?;
    if !remainder.is_empty() && !remainder.starts_with('\t') {
        return Err(GuidedReviewParseError::InvalidFileHeader { line: line.to_owned() });
    }
    normalize_path(&path, line)
}

fn normalize_path(
    raw_path: &str,
    source_line: &str,
) -> Result<Option<PathBuf>, GuidedReviewParseError> {
    if raw_path == "/dev/null" {
        return Ok(None);
    }
    let path =
        raw_path.strip_prefix("a/").or_else(|| raw_path.strip_prefix("b/")).unwrap_or(raw_path);
    let path = PathBuf::from(path);
    (!path.as_os_str().is_empty()
        && path.components().all(|part| matches!(part, Component::Normal(_))))
    .then_some(path)
    .map(Some)
    .ok_or_else(|| GuidedReviewParseError::InvalidFileHeader { line: source_line.to_owned() })
}
