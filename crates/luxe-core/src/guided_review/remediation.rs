//! Remediation reports — the agent's word on each comment it was asked to fix.
//!
//! Reports are parsed as data, not instructions. A valid `fixed` claim is retained as evidence
//! for the reviewer; it never resolves a finding on its own.
//!
//! A report is recognized by what it says, not by how it is wrapped. Models asked for a
//! ` ```luxe-remediation ` block write ` ```json `, or no fence at all, around JSON that is
//! otherwise exactly right, and a reader keyed on the fence threw those away. So identity is strict
//! — only the comments that were sent, and never another review or revision — and everything else
//! is forgiving: each entry stands on its own, and a status reads however it is capitalized.
//!
//! The review and revision come from the turn, which is pinned to one submission, so the template
//! does not ask the model to copy them: one miscopied character of a 64-digit hash refused a report
//! that was otherwise right. A report that names them anyway is still held to them.

use super::{FindingId, ReviewId, ReviewRevisionId};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

/// Maximum accepted agent remediation-report payload size.
pub const MAX_REMEDIATION_REPORT_BYTES: usize = 16 * 1024;
/// Maximum individual remediation results accepted from one agent turn.
pub const MAX_REMEDIATION_REPORT_ITEMS: usize = 128;
/// Maximum display-safe note size attached to one remediation result.
pub const MAX_REMEDIATION_NOTE_BYTES: usize = 2_048;
const REMEDIATION_FENCE_OPEN: &str = "```luxe-remediation\n";
/// The keys a report opens with, in whatever order a model writes them.
const REPORT_KEYS: [&str; 3] = ["review_id", "revision_id", "findings"];
/// What the template puts where a status goes; left as it is, it reads as no status at all.
const STATUS_PLACEHOLDER: &str = "fixed | not_fixed | needs_clarification";
/// What the template puts where a note goes; left as it is, it is no note.
const NOTE_PLACEHOLDER: &str = "what you did, in one sentence";

/// One bounded result the agent reports for a submitted finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemediationResult {
    /// Durable finding identity from the submitted feedback.
    pub finding_id: FindingId,
    /// The agent's claimed outcome, retained as evidence only.
    pub status: RemediationStatus,
    /// Optional concise explanation for the reviewer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Agent-provided remediation status; review state remains user-controlled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemediationStatus {
    /// The agent claims the finding was addressed.
    Fixed,
    /// The agent could not address the finding.
    NotFixed,
    /// The agent requires reviewer input before proceeding.
    NeedsClarification,
}

/// A source-pinned result set returned by an agent turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemediationReport {
    /// Logical review that issued the feedback.
    pub review_id: ReviewId,
    /// Immutable reviewed revision that the report addresses.
    pub revision_id: ReviewRevisionId,
    /// Per-finding results keyed by durable IDs.
    pub findings: Vec<RemediationResult>,
}

/// Why a report the agent wrote cannot be used.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum RemediationReportError {
    /// The model output exceeds the fixed parsing budget.
    #[error("remediation report exceeds {MAX_REMEDIATION_REPORT_BYTES} bytes")]
    TooLarge,
    /// Not JSON, or not shaped like a report.
    #[error("invalid remediation report: {0}")]
    InvalidJson(String),
    /// The report belongs to another logical review.
    #[error("remediation report has a different review ID")]
    WrongReview,
    /// The report describes a different immutable source revision.
    #[error("remediation report has a different source revision")]
    WrongRevision,
    /// Not one entry names a comment that was sent with a status Luxe reads.
    #[error("remediation report names no submitted finding with a known status")]
    NothingUsable,
}

impl RemediationReportError {
    /// What went wrong, as the tail of a sentence: told to the agent when Luxe asks again, and to
    /// the reviewer on each comment the report did not cover.
    pub fn reason(&self) -> &'static str {
        match self {
            Self::TooLarge => "its report was too long to read",
            Self::InvalidJson(_) => "its report was not valid JSON",
            Self::WrongReview => "its report was for a different review",
            Self::WrongRevision => "its report was for a different version of the code",
            Self::NothingUsable => {
                "its report named none of these comments with a status of fixed, not_fixed or \
                 needs_clarification"
            }
        }
    }
}

/// The report in a turn's assistant text, wherever and however it was written.
///
/// Every JSON object that opens with one of a report's keys is a candidate, fenced or not, and
/// the last one that reads wins, so a model that corrects itself is believed. `Ok(None)` when the
/// text holds nothing that looks like a report; otherwise the last candidate's error.
pub fn extract_remediation_report(
    assistant_text: &str,
    review_id: &ReviewId,
    revision_id: &ReviewRevisionId,
    known_findings: impl IntoIterator<Item = FindingId>,
) -> Result<Option<RemediationReport>, RemediationReportError> {
    let known = known_findings.into_iter().collect::<Vec<_>>();
    let mut outcome = Ok(None);
    for start in report_starts(assistant_text) {
        let read = match first_json_value(&assistant_text[start..]) {
            Ok((_, len)) if len > MAX_REMEDIATION_REPORT_BYTES => {
                Err(RemediationReportError::TooLarge)
            }
            Ok((value, _)) => read_report(&value, review_id, revision_id, &known),
            Err(error) => Err(RemediationReportError::InvalidJson(error.to_string())),
        };
        match read {
            Ok(report) => outcome = Ok(Some(report)),
            Err(error) if !matches!(outcome, Ok(Some(_))) => outcome = Err(error),
            Err(error) => tracing::debug!(%error, "ignored a report-like block after a valid one"),
        }
    }
    outcome
}

/// The block a fix request ends with: each comment's ID filled in, a placeholder where its status
/// and note go, one line per comment in the order they were listed.
pub fn remediation_report_template(finding_ids: &[FindingId]) -> String {
    let entries: Vec<String> = finding_ids
        .iter()
        .map(|finding_id| {
            serde_json::json!({
                "finding_id": finding_id,
                "status": STATUS_PLACEHOLDER,
                "note": NOTE_PLACEHOLDER,
            })
            .to_string()
        })
        .collect();
    format!("{REMEDIATION_FENCE_OPEN}{{\"findings\":[\n{}\n]}}\n```", entries.join(",\n"))
}

/// `assistant_text` without its remediation report, for showing to a person.
///
/// The report is the agent talking to the review, not to the reader, and the review already shows
/// what it says, finding by finding. A report still streaming in — its opening fence arrived, its
/// closing one not yet, or not even the whole opening line — goes too, so it never flashes up.
///
/// Whatever wrapped it: the named fence, any other fence, or none, along with a bare
/// `luxe-remediation` label line in front of it. This runs on every assistant message, so anything
/// but the named fence has to be unmistakably a report — a `findings` list naming a finding, or a
/// `review_id` that is a review's — and JSON a reader asked for in conversation never is.
pub fn without_remediation_report(assistant_text: &str) -> std::borrow::Cow<'_, str> {
    let mut text = without_fence(assistant_text, REMEDIATION_FENCE_OPEN);
    while let Some((start, end)) = report_span(&text) {
        text = std::borrow::Cow::Owned(joined(&text[..start], &text[end..]));
    }
    text
}

/// `assistant_text` without the fenced block that opens with `open` (its opening line, newline
/// included), whole or still arriving.
pub(crate) fn without_fence<'a>(assistant_text: &'a str, open: &str) -> std::borrow::Cow<'a, str> {
    let opener = open.trim_end_matches('\n');
    let fence = match assistant_text.find(open) {
        Some(start) => {
            let body = start + open.len();
            let end = match assistant_text[body..].find("\n```") {
                Some(close) => {
                    let after = body + close + "\n```".len();
                    assistant_text[after..]
                        .find('\n')
                        .map_or(assistant_text.len(), |eol| after + eol)
                }
                None => assistant_text.len(),
            };
            Some((start, end))
        }
        None => {
            // The opening line arriving a few characters at a time
            let tail_start = assistant_text.rfind('\n').map_or(0, |eol| eol + 1);
            let tail = &assistant_text[tail_start..];
            (tail.len() > "```".len() && opener.starts_with(tail))
                .then_some((tail_start, assistant_text.len()))
        }
    };
    let Some((start, end)) = fence else {
        return std::borrow::Cow::Borrowed(assistant_text);
    };
    std::borrow::Cow::Owned(joined(&assistant_text[..start], &assistant_text[end..]))
}

// === Reading ===

/// Where each JSON object that opens with one of a report's keys starts, in order.
fn report_starts(text: &str) -> impl Iterator<Item = usize> + '_ {
    text.match_indices('{').map(|(at, _)| at).filter(|&at| opens_like_a_report(&text[at + 1..]))
}

/// Whether what follows an opening brace is one of a report's keys.
fn opens_like_a_report(after_brace: &str) -> bool {
    let Some(key) = after_brace.trim_start().strip_prefix('"') else {
        return false;
    };
    REPORT_KEYS.iter().any(|name| key.strip_prefix(name).is_some_and(|rest| rest.starts_with('"')))
}

/// The JSON value `text` opens with, and how many bytes it took; whatever follows is ignored.
fn first_json_value(text: &str) -> Result<(Value, usize), serde_json::Error> {
    let mut values = serde_json::Deserializer::from_str(text).into_iter::<Value>();
    match values.next() {
        Some(Ok(value)) => Ok((value, values.byte_offset())),
        Some(Err(error)) => Err(error),
        None => Err(serde::de::Error::custom("empty")),
    }
}

/// One report out of a JSON object, keeping the entries that stand on their own.
///
/// A review or revision the report names must be the turn's, prefix or no prefix, or the whole
/// report is refused: it is about something else. One it leaves out is the turn's. Inside it, an entry for a comment that was not sent, a second entry for
/// the same comment, or a status Luxe cannot read is dropped and the rest are kept. A report left
/// with nothing is [`RemediationReportError::NothingUsable`].
fn read_report(
    value: &Value,
    review_id: &ReviewId,
    revision_id: &ReviewRevisionId,
    known: &[FindingId],
) -> Result<RemediationReport, RemediationReportError> {
    let invalid = |what: &str| RemediationReportError::InvalidJson(what.to_string());
    let object = value.as_object().ok_or_else(|| invalid("not an object"))?;
    let id = |key: &str| object.get(key).and_then(Value::as_str).map(str::trim);
    if id("review_id").is_some_and(|found| !same_id(found, review_id.as_str(), "review:")) {
        return Err(RemediationReportError::WrongReview);
    }
    if id("revision_id").is_some_and(|found| !same_id(found, revision_id.as_str(), "revision:")) {
        return Err(RemediationReportError::WrongRevision);
    }
    let entries = object
        .get("findings")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("findings is not a list"))?;
    let mut findings: Vec<RemediationResult> = Vec::new();
    for entry in entries.iter().take(MAX_REMEDIATION_REPORT_ITEMS) {
        match read_result(entry, known) {
            Some(result) if findings.iter().all(|kept| kept.finding_id != result.finding_id) => {
                findings.push(result);
            }
            Some(_) => tracing::debug!("dropped a second report entry for one comment"),
            None => tracing::debug!(%entry, "dropped a report entry Luxe cannot use"),
        }
    }
    if findings.is_empty() {
        return Err(RemediationReportError::NothingUsable);
    }
    Ok(RemediationReport {
        review_id: review_id.clone(),
        revision_id: revision_id.clone(),
        findings,
    })
}

/// One entry, when it names a comment that was sent and has a status Luxe reads.
fn read_result(entry: &Value, known: &[FindingId]) -> Option<RemediationResult> {
    let named = entry.get("finding_id")?.as_str()?.trim();
    // the known ID, never one built out of what the model wrote
    let finding_id = known.iter().find(|id| same_id(named, id.as_str(), "finding:"))?.clone();
    let status = match entry
        .get("status")?
        .as_str()?
        .trim()
        .to_ascii_lowercase()
        .replace([' ', '-'], "_")
        .as_str()
    {
        "fixed" => RemediationStatus::Fixed,
        "not_fixed" => RemediationStatus::NotFixed,
        "needs_clarification" => RemediationStatus::NeedsClarification,
        _ => return None,
    };
    let note = entry
        .get("note")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|note| !note.is_empty() && *note != NOTE_PLACEHOLDER)
        .map(|note| clipped(note, MAX_REMEDIATION_NOTE_BYTES));
    Some(RemediationResult { finding_id, status, note })
}

/// Whether `found` is `expected`, written with its `prefix` or without it.
fn same_id(found: &str, expected: &str, prefix: &str) -> bool {
    found == expected || expected.strip_prefix(prefix) == Some(found)
}

/// `note` cut to at most `max` bytes on a character boundary, marked where it was cut.
fn clipped(note: &str, max: usize) -> String {
    if note.len() <= max {
        return note.to_string();
    }
    let mut end = max - '…'.len_utf8();
    while !note.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &note[..end])
}

// === Hiding ===

/// The span of the first report not wrapped in the named fence, whole or still arriving, with the
/// label line in front of it.
fn report_span(text: &str) -> Option<(usize, usize)> {
    let lines = line_spans(text);
    let mut at = 0;
    while at < lines.len() {
        let (start, end) = lines[at];
        let line = text[start..end].trim();
        if let Some(marker) = fence_marker(line) {
            let close = (at + 1..lines.len()).find(|&next| {
                let candidate = text[lines[next].0..lines[next].1].trim();
                candidate.starts_with(marker) && candidate.trim_start_matches(marker).is_empty()
            });
            let body_start = lines.get(at + 1).map_or(text.len(), |line| line.0);
            let body_end = close.map_or(text.len(), |close| lines[close].0);
            let body = &text[body_start..body_end.max(body_start)];
            let report = match close {
                Some(_) => is_a_report(body.trim()),
                None => body.trim_start().strip_prefix('{').is_some_and(opens_like_a_report),
            };
            if report {
                let end = close.map_or(text.len(), |close| lines[close].1);
                return Some((with_label(text, &lines, at), end));
            }
            at = close.map_or(lines.len(), |close| close + 1);
            continue;
        }
        if line.strip_prefix('{').is_some_and(opens_like_a_report) {
            let from = start + text[start..].find('{').unwrap_or(0);
            match first_json_value(&text[from..]) {
                Ok((value, len)) if is_a_report_value(&value) => {
                    let end =
                        text[from + len..].find('\n').map_or(text.len(), |eol| from + len + eol);
                    return Some((with_label(text, &lines, at), end));
                }
                // still arriving
                Err(error) if error.is_eof() => {
                    return Some((with_label(text, &lines, at), text.len()))
                }
                _ => {}
            }
        }
        at += 1;
    }
    None
}

/// Each line's byte span, newline excluded.
fn line_spans(text: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start = 0;
    for line in text.split_inclusive('\n') {
        let end = start + line.trim_end_matches(['\n', '\r']).len();
        spans.push((start, end));
        start += line.len();
    }
    spans
}

/// The fence a line opens or closes, ` ``` ` or ` ~~~ `.
fn fence_marker(line: &str) -> Option<&'static str> {
    ["```", "~~~"].into_iter().find(|marker| line.starts_with(marker))
}

/// Where a report starting on line `at` begins, taking a `luxe-remediation` label line above it.
fn with_label(text: &str, lines: &[(usize, usize)], at: usize) -> usize {
    let label = |line: &str| {
        line.trim_matches(|c: char| c.is_whitespace() || matches!(c, '`' | '*' | ':'))
            .eq_ignore_ascii_case("luxe-remediation")
    };
    match at.checked_sub(1) {
        Some(above) if label(&text[lines[above].0..lines[above].1]) => lines[above].0,
        _ => lines[at].0,
    }
}

fn is_a_report(body: &str) -> bool {
    serde_json::from_str::<Value>(body).is_ok_and(|value| is_a_report_value(&value))
}

/// A `findings` list naming a finding, or one beside a `review_id` that is a review's: never JSON a
/// reader asked for.
fn is_a_report_value(value: &Value) -> bool {
    let Some(findings) = value.get("findings").and_then(Value::as_array) else {
        return false;
    };
    let names_a_finding = findings.iter().any(|entry| {
        entry.get("finding_id").and_then(Value::as_str).is_some_and(|id| id.starts_with("finding:"))
    });
    let of_a_review =
        value.get("review_id").and_then(Value::as_str).is_some_and(|id| id.starts_with("review:"));
    names_a_finding || of_a_review
}

/// `before` and `after` with the cut between them closed up to one blank line.
fn joined(before: &str, after: &str) -> String {
    let before = before.trim_end();
    let after = after.trim_start_matches(['\n', '\r']);
    match (before.is_empty(), after.is_empty()) {
        (_, true) => before.to_owned(),
        (true, false) => after.to_owned(),
        (false, false) => format!("{before}\n\n{after}"),
    }
}

#[cfg(test)]
mod tests;
