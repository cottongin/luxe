//! Mention sources — a program the `@` picker lists references from.
//!
//! luxe knows files and GitHub. Everything else a person works from — Jira issues, Asana tasks, a
//! runbook index — is a program they point it at: it prints one JSON object a line, and each line
//! becomes a row in a tab of its own. A picked row goes into the composer as `@[kind:id]` and
//! reaches the model as the row's `text`, a reference and never the thing itself; the agent looks
//! further with whatever tools it has.
//!
//! ```text
//! {"id": "PROJ-12", "label": "PROJ-12 Login fails on Safari", "detail": "In progress",
//!  "text": "Jira issue PROJ-12 \"Login fails on Safari\" (In progress) https://…/PROJ-12"}
//! ```
//!
//! `id` and `label` are required; `detail` is shown beside the label, and `text` defaults to the
//! label. What the program prints is other people's words as much as the reader's, so every field
//! is scrubbed and bounded like any text from outside, and the whole run is bounded in time, rows
//! and bytes. A line that does not read is skipped rather than failing the rest.

use crate::config::MentionSource;
use serde::Deserialize;

/// Longest a source may take to answer.
pub const MENTION_SOURCE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(15);
/// Most rows a source lists.
pub const MAX_MENTION_ROWS: usize = 500;
/// Longest line read, in bytes; a longer one is skipped.
const MAX_LINE_BYTES: usize = 8 * 1024;
/// Longest label or detail kept, in characters.
const MAX_LABEL_CHARS: usize = 200;
/// Longest text sent to the model for one reference, in characters.
const MAX_TEXT_CHARS: usize = 600;
/// Longest id kept, in characters: it is written into the composer.
const MAX_ID_CHARS: usize = 120;

/// One reference a source listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MentionRow {
    /// What its reference names, as in `@[jira:PROJ-12]`
    pub id: String,
    /// What the picker shows
    pub label: String,
    /// Shown beside the label, quieter
    pub detail: Option<String>,
    /// What the model is sent in the reference's place
    pub text: String,
}

/// Why a source listed nothing.
#[derive(Debug, thiserror::Error)]
pub enum MentionSourceError {
    /// The program could not be started
    #[error("could not run `{command}`: {error}")]
    Spawn {
        /// The program
        command: String,
        /// Why
        error: std::io::Error,
    },
    /// The program took longer than the deadline, and was stopped
    #[error("`{command}` took longer than {}s, and was stopped", .after.as_secs())]
    Timeout {
        /// The program
        command: String,
        /// The deadline
        after: std::time::Duration,
    },
    /// The program exited unsuccessfully
    #[error("`{command}` failed: {said}")]
    Failed {
        /// The program
        command: String,
        /// Its last words on stderr, or its exit status
        said: String,
    },
}

/// Runs `source` in `cwd` and reads the rows it prints.
pub async fn list(
    source: &MentionSource,
    cwd: &std::path::Path,
) -> Result<Vec<MentionRow>, MentionSourceError> {
    let program = crate::program::resolve(&source.command)
        .unwrap_or_else(|| std::path::PathBuf::from(&source.command));
    let args: Vec<&str> = source.args.iter().map(String::as_str).collect();
    let command = crate::program::captured_command(program, &args, cwd);
    let named = || source.command.clone();
    let output =
        crate::program::run_bounded(command, MENTION_SOURCE_DEADLINE).await.map_err(|error| {
            match error {
                crate::program::BoundedRunError::Timeout { after } => {
                    MentionSourceError::Timeout { command: named(), after }
                }
                crate::program::BoundedRunError::Spawn(source)
                | crate::program::BoundedRunError::Wait(source) => {
                    MentionSourceError::Spawn { command: named(), error: source }
                }
            }
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let said = stderr
            .lines()
            .map(str::trim)
            .rfind(|line| !line.is_empty())
            .map(|line| crate::github::untrusted(line, MAX_LABEL_CHARS))
            .unwrap_or_else(|| format!("it exited with {}", output.status));
        return Err(MentionSourceError::Failed { command: named(), said });
    }
    Ok(parse(&String::from_utf8_lossy(&output.stdout)))
}

/// The rows in a source's output: one JSON object a line, up to [`MAX_MENTION_ROWS`].
pub fn parse(output: &str) -> Vec<MentionRow> {
    #[derive(Deserialize)]
    struct Line {
        id: serde_json::Value,
        label: String,
        #[serde(default)]
        detail: Option<String>,
        #[serde(default)]
        text: Option<String>,
    }
    output
        .lines()
        .filter(|line| line.len() <= MAX_LINE_BYTES)
        .filter_map(|line| serde_json::from_str::<Line>(line.trim()).ok())
        .filter_map(|line| {
            // An id may be a number, as issue trackers' often are
            let id = match line.id {
                serde_json::Value::String(id) => id,
                serde_json::Value::Number(id) => id.to_string(),
                _ => return None,
            };
            let id = one_line(&id, MAX_ID_CHARS);
            // It is written into the composer inside `@[kind:…]`, which a bracket would end
            if id.is_empty() || id.contains(']') {
                return None;
            }
            let label = one_line(&line.label, MAX_LABEL_CHARS);
            if label.is_empty() {
                return None;
            }
            let detail = line
                .detail
                .map(|detail| one_line(&detail, MAX_LABEL_CHARS))
                .filter(|detail| !detail.is_empty());
            let text = line
                .text
                .map(|text| one_line(&text, MAX_TEXT_CHARS))
                .filter(|text| !text.is_empty())
                .unwrap_or_else(|| label.clone());
            Some(MentionRow { id, label, detail, text })
        })
        .take(MAX_MENTION_ROWS)
        .collect()
}

/// `text` scrubbed, on one line — a reference sits inside a sentence — and cut with an ellipsis
/// past `max` characters.
fn one_line(text: &str, max: usize) -> String {
    let clean = crate::instructions::sanitize(text).text;
    let line = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= max {
        return line;
    }
    let kept: String = line.chars().take(max).collect();
    format!("{}…", kept.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each line is a row; what is missing takes its default, and what does not read — or would
    /// break the reference it is written into — is skipped without costing the others.
    #[test]
    fn each_line_is_a_row_and_a_bad_one_costs_only_itself() {
        let output = format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n",
            r#"{"id":"PROJ-12","label":"PROJ-12 Login fails","detail":"In progress","text":"Jira issue PROJ-12 (In progress)"}"#,
            r#"{"id":1204,"label":"Write the runbook"}"#,
            "not json at all",
            r#"{"id":"A]B","label":"a bracket would end the token"}"#,
            r#"{"id":"","label":"no id"}"#,
            r#"{"id":"X-1","label":"   "}"#,
            r#"{"id":["X"],"label":"an id that is a list"}"#,
            r#"  {"id":"X-2","label":"Two\nlines\u200b here","detail":"  ","text":"  "}  "#,
        );
        let rows = parse(&output);
        let seen: Vec<(&str, &str, Option<&str>, &str)> = rows
            .iter()
            .map(|row| {
                (row.id.as_str(), row.label.as_str(), row.detail.as_deref(), row.text.as_str())
            })
            .collect();
        assert_eq!(
            seen,
            [
                (
                    "PROJ-12",
                    "PROJ-12 Login fails",
                    Some("In progress"),
                    "Jira issue PROJ-12 (In progress)"
                ),
                ("1204", "Write the runbook", None, "Write the runbook"),
                ("X-2", "Two lines here", None, "Two lines here"),
            ]
        );
    }

    /// A source cannot flood the picker or the message: rows, lines and fields are all bounded.
    #[test]
    fn a_source_is_bounded_in_rows_lines_and_fields() {
        let many: String = (0..MAX_MENTION_ROWS + 20)
            .map(|n| format!("{{\"id\":\"R-{n}\",\"label\":\"Row {n}\"}}\n"))
            .collect();
        assert_eq!(parse(&many).len(), MAX_MENTION_ROWS);
        let huge = format!("{{\"id\":\"H\",\"label\":\"{}\"}}", "x".repeat(MAX_LINE_BYTES));
        assert!(parse(&huge).is_empty(), "a line past the limit is skipped");
        let long = format!(
            "{{\"id\":\"L\",\"label\":\"{}\",\"text\":\"{}\"}}",
            "y".repeat(MAX_LABEL_CHARS * 2),
            "z".repeat(MAX_TEXT_CHARS * 2)
        );
        let row = &parse(&long)[0];
        assert_eq!(row.label.chars().count(), MAX_LABEL_CHARS + 1);
        assert!(row.label.ends_with('…'));
        assert_eq!(row.text.chars().count(), MAX_TEXT_CHARS + 1);
        let exact = format!("{{\"id\":\"E\",\"label\":\"{}\"}}", "w".repeat(MAX_LABEL_CHARS));
        assert!(!parse(&exact)[0].label.ends_with('…'), "exactly at the limit is whole");
    }

    #[cfg(unix)]
    fn source(command: &str, args: &[&str]) -> MentionSource {
        MentionSource {
            name: "Test".into(),
            kind: None,
            command: command.into(),
            args: args.iter().map(|arg| arg.to_string()).collect(),
            icon: None,
        }
    }

    /// Run for real: its rows come back, a failure says what the program said last, and a program
    /// that is not there says so.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_source_is_run_and_its_failures_say_why() {
        let dir = tempfile::tempdir().unwrap();
        let printed = r#"printf '{"id":"T-1","label":"One"}\n'; printf 'noise\n'"#;
        let rows = list(&source("sh", &["-c", printed]), dir.path()).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "T-1");

        let failing = "echo 'first' >&2; echo 'Not signed in: run jira login' >&2; exit 3";
        match list(&source("sh", &["-c", failing]), dir.path()).await {
            Err(MentionSourceError::Failed { said, .. }) => {
                assert_eq!(said, "Not signed in: run jira login");
            }
            other => panic!("{other:?}"),
        }
        match list(&source("sh", &["-c", "exit 4"]), dir.path()).await {
            Err(MentionSourceError::Failed { said, .. }) => assert!(said.contains('4'), "{said}"),
            other => panic!("{other:?}"),
        }
        let missing = list(&source("luxe-no-such-program-here", &[]), dir.path()).await;
        assert!(matches!(missing, Err(MentionSourceError::Spawn { .. })), "{missing:?}");
    }
}
