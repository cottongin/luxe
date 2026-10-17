//! Hunks — a hunk's header, its ranges, and the lines it carries.

use super::*;

pub(super) fn parse_hunk(
    lines: &[&str],
    start: usize,
    file_id: &FileId,
) -> Result<(ReviewHunk, usize), GuidedReviewParseError> {
    let header = lines[start];
    let (old_range, new_range, heading) = parse_hunk_header(header)?;
    let mut old_line = old_range.start;
    let mut new_line = new_range.start;
    let mut index = start + 1;
    let mut review_lines = Vec::new();
    let mut changed_content = String::from(header);
    changed_content.push('\n');

    while index < lines.len() && !lines[index].starts_with("@@ ") {
        let line = lines[index];
        if line.starts_with("diff --git ") {
            break;
        }
        changed_content.push_str(line);
        changed_content.push('\n');
        let (review_line, old_next, new_next) = parse_hunk_line(line, old_line, new_line)?;
        review_lines.push(review_line);
        old_line = old_next;
        new_line = new_next;
        index += 1;
    }

    let actual_old = review_lines.iter().filter(|line| line.old_line.is_some()).count() as u32;
    let actual_new = review_lines.iter().filter(|line| line.new_line.is_some()).count() as u32;
    if actual_old != old_range.count || actual_new != new_range.count {
        return Err(GuidedReviewParseError::HunkLineCountMismatch {
            header: header.to_owned(),
            expected_old: old_range.count,
            actual_old,
            expected_new: new_range.count,
            actual_new,
        });
    }

    let normalized_context = heading.as_deref().unwrap_or_default();
    Ok((
        ReviewHunk {
            id: crate::guided_review::HunkId::from_content(
                file_id,
                normalized_context,
                &changed_content,
            ),
            old_range,
            new_range,
            heading,
            symbol: None,
            lines: review_lines,
            noise: NoiseKind::Semantic,
        },
        index,
    ))
}

/// One line of a hunk, with the line numbers after it.
///
/// Inside a hunk, position already told the file headers apart: a deletion of `-- text` arrives
/// as `--- text` and an addition of `++ text` as `+++ text`, and both are payload.
pub(super) fn parse_hunk_line(
    line: &str,
    old_line: u32,
    new_line: u32,
) -> Result<(ReviewLine, u32, u32), GuidedReviewParseError> {
    let numbered = |kind, old: Option<u32>, new: Option<u32>, text: &str| ReviewLine {
        old_line: old,
        new_line: new,
        kind,
        text: text.to_owned(),
    };
    Ok(match line.as_bytes().first() {
        Some(b' ') => (
            numbered(ReviewLineKind::Context, Some(old_line), Some(new_line), &line[1..]),
            old_line + 1,
            new_line + 1,
        ),
        Some(b'-') => (
            numbered(ReviewLineKind::Deletion, Some(old_line), None, &line[1..]),
            old_line + 1,
            new_line,
        ),
        Some(b'+') => (
            numbered(ReviewLineKind::Addition, None, Some(new_line), &line[1..]),
            old_line,
            new_line + 1,
        ),
        Some(b'\\') if line == "\\ No newline at end of file" => {
            (numbered(ReviewLineKind::NoNewlineMarker, None, None, line), old_line, new_line)
        }
        _ => return Err(GuidedReviewParseError::InvalidHunkLine { line: line.to_owned() }),
    })
}

pub(super) fn parse_hunk_header(
    header: &str,
) -> Result<(LineRange, LineRange, Option<String>), GuidedReviewParseError> {
    let Some(rest) = header.strip_prefix("@@ ") else {
        return Err(GuidedReviewParseError::InvalidHunkHeader { header: header.to_owned() });
    };
    let Some((ranges, trailing)) = rest.split_once(" @@") else {
        return Err(GuidedReviewParseError::InvalidHunkHeader { header: header.to_owned() });
    };
    let mut ranges = ranges.split_whitespace();
    let old = parse_range(ranges.next(), '-')?;
    let new = parse_range(ranges.next(), '+')?;
    if ranges.next().is_some() {
        return Err(GuidedReviewParseError::InvalidHunkHeader { header: header.to_owned() });
    }
    let heading = (!trailing.trim().is_empty()).then(|| trailing.trim().to_owned());
    Ok((old, new, heading))
}

pub(super) fn parse_range(
    value: Option<&str>,
    prefix: char,
) -> Result<LineRange, GuidedReviewParseError> {
    let value = value.ok_or_else(|| GuidedReviewParseError::InvalidHunkHeader {
        header: "missing range".into(),
    })?;
    let value = value
        .strip_prefix(prefix)
        .ok_or_else(|| GuidedReviewParseError::InvalidHunkHeader { header: value.to_owned() })?;
    let (start, count) =
        value.split_once(',').map_or((value, "1"), |(start, count)| (start, count));
    let start = start
        .parse()
        .map_err(|_| GuidedReviewParseError::InvalidHunkHeader { header: value.to_owned() })?;
    let count = count
        .parse()
        .map_err(|_| GuidedReviewParseError::InvalidHunkHeader { header: value.to_owned() })?;
    Ok(LineRange::new(start, count))
}
