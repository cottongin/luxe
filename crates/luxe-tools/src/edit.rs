//! `edit` tool — search/replace with a read guard, atomic multi-edit, and a tolerant edge.
//!
//! The tool is a UX for a model, and the model only ever sees the schema and the errors. Four
//! things follow from that, and they are what this module is shaped around:
//!
//! - **The advertised spelling is `oldText`/`newText`**, and which spelling is advertised matters
//!   more than everything else here combined. See `args`.
//! - **Argument dialects are accepted** (`args`). Rejecting `old_string` because the schema says
//!   `oldText` costs a turn and teaches nothing.
//! - **Near misses are tolerated, and named** (`locate`). Whitespace and indentation are how a
//!   model's copy of a block differs from the file's; neither changes which code was meant. Every
//!   relaxation still demands a unique match.
//! - **Failures are correctable from their own text**: what arrived, what was expected, and where
//!   the closest thing actually is.
//!
//! What stays strict: uniqueness (unless `replaceAll`), read-before-edit, staleness, and
//! atomicity — all edits land or none do.

mod args;
mod locate;

use async_trait::async_trait;
use luxe_core::read_tracker::{ReadStatus, ReadTracker};
use luxe_core::tool::guards::{CHANGED_ON_DISK, READ_BEFORE_EDIT};
use luxe_core::{ContentBlock, Tool, ToolContext, ToolResultData, ToolSchema};
use serde_json::json;
use std::path::Path;
use std::sync::Arc;
use tracing::{debug, warn};

use args::{normalize, EditSpec};
use locate::{line_of, locate, LocateError, MatchKind};

/// JSON-schema for the edit tool arguments.
///
/// Advertises exactly one spelling. The tolerated aliases are a recovery path, not a second
/// contract — documenting them would invite their use and make every one of them permanent.
fn edit_schema() -> ToolSchema {
    ToolSchema {
        name: "edit".to_string(),
        description: "Edit a file by replacing exact text. Each edit's `oldText` must match a \
                      UNIQUE region of the current file — include enough surrounding context to \
                      disambiguate, or set `replaceAll` to change every occurrence. Multiple \
                      edits apply atomically (all-or-nothing) and must not overlap. Trailing \
                      whitespace and a uniformly shifted indent are tolerated and reported. You \
                      must have seen the file in this Luxe run — with `read`, lines of it from \
                      `grep`, or printed by `cat`, `sed -n` or a search in `bash` — and it must \
                      not have changed \
                      on disk since (read-before-edit + staleness guards); otherwise read it \
                      again and retry. The `12: ` numbers `read` puts on each line are not part of \
                      the file: leave them out of `oldText` and `newText`."
            .to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to edit (relative to cwd)"
                },
                "edits": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "oldText": {
                                "type": "string",
                                "description": "Exact text to find"
                            },
                            "newText": {
                                "type": "string",
                                "description": "Replacement text (empty string deletes)"
                            },
                            "replaceAll": {
                                "type": "boolean",
                                "description": "Replace every occurrence instead of requiring a unique match (default false)"
                            }
                        },
                        "required": ["oldText", "newText"]
                    },
                    "description": "Array of edit operations to apply atomically"
                }
            },
            "required": ["path", "edits"]
        }),
    }
}

/// The `edit` tool implementation.
#[derive(Default)]
pub struct EditTool {
    /// Shared read tracker enforcing read-before-edit + staleness (guards skipped when absent).
    tracker: Option<Arc<ReadTracker>>,
}

impl EditTool {
    /// Create a new edit tool instance.
    pub fn new() -> Self {
        Self { tracker: None }
    }

    /// Attach the shared read tracker so edits are gated on a prior, still-current read.
    pub fn with_tracker(mut self, tracker: Arc<ReadTracker>) -> Self {
        self.tracker = Some(tracker);
        self
    }
}

/// One edit resolved against the file: where it lands, and what to write.
struct Planned {
    ranges: Vec<std::ops::Range<usize>>,
    replacement: String,
    kind: MatchKind,
}

/// Resolves every edit against `content`, or explains the first one that cannot be placed.
///
/// Every edit is located against the **original** text, so their uniqueness checks all describe
/// the same file. Applying them is a separate step, and applies by offset in reverse — the
/// previous implementation validated uniqueness up front and then ran `str::replace` in sequence,
/// so an earlier edit whose replacement happened to contain a later edit's search text quietly
/// changed both.
fn plan(content: &str, edits: &[EditSpec]) -> Result<Vec<Planned>, String> {
    let mut planned: Vec<Planned> = Vec::with_capacity(edits.len());
    for (index, edit) in edits.iter().enumerate() {
        match locate(content, &edit.old_text, &edit.new_text, edit.replace_all) {
            Ok(found) if writes_back_what_is_there(content, &found.ranges, &found.replacement) => {
                return Err(format!(
                    "Error: edit[{index}] would write back exactly the text it matched, so it \
                     changes nothing. Check that `newText` really differs from `oldText` — an \
                     escape or an invisible character surviving the round trip unchanged is the \
                     usual cause, and so is an edit that was already applied. Nothing was written."
                ))
            }
            Ok(found) => planned.push(Planned {
                ranges: found.ranges,
                replacement: found.replacement,
                kind: found.kind,
            }),
            Err(LocateError::NotFound { nearest }) => {
                let mut message = format!(
                    "Error: edit[{index}] 'oldText' not found in the file. It must match the \
                     current contents exactly (trailing whitespace and a uniform indent shift are \
                     tolerated)."
                );
                if let Some(nearest) = nearest {
                    message.push_str(&format!(
                        "\nClosest match is line {}:\n  have: {}\n  want: {}",
                        nearest.line,
                        nearest.text.trim_end(),
                        edit.old_text.lines().next().unwrap_or_default().trim_end()
                    ));
                }
                message.push_str("\nNothing was written. Re-read the file and retry.");
                return Err(message);
            }
            Err(LocateError::Ambiguous { lines }) => {
                let places = lines.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ");
                return Err(format!(
                    "Error: edit[{index}] 'oldText' matched {} times (lines {places}). Add \
                     surrounding context to make it unique, or set `replaceAll: true` to change \
                     every occurrence. Nothing was written.",
                    lines.len()
                ));
            }
        }
    }
    if let Some(clash) = overlap(&planned) {
        return Err(clash);
    }
    Ok(planned)
}

/// The edits again with `read`'s `12: ` line numbers taken off, planned, when that is unmistakably
/// what went wrong.
///
/// A model copies the numbered lines `read` returned straight into `oldText` often enough to be
/// worth catching, and the alternative is a refusal it has to puzzle out. Strict on purpose, so it
/// can never change what an edit means: it runs only when the edits failed as written, only on an
/// `oldText` whose every line carries a number counting up by one, and the result stands only if
/// each stripped edit is found at exactly the line its first number names.
fn plan_without_read_numbers(content: &str, edits: &[EditSpec]) -> Option<Vec<Planned>> {
    let mut stripped = Vec::with_capacity(edits.len());
    let mut expected_lines = Vec::with_capacity(edits.len());
    let mut any = false;
    for edit in edits {
        match strip_read_numbers(&edit.old_text, true) {
            Some((first, old_text)) if !edit.replace_all => {
                let new_text = strip_read_numbers(&edit.new_text, false)
                    .map_or_else(|| edit.new_text.clone(), |(_, text)| text);
                stripped.push(EditSpec { old_text, new_text, replace_all: false });
                expected_lines.push(Some(first));
                any = true;
            }
            _ => {
                stripped.push(edit.clone());
                expected_lines.push(None);
            }
        }
    }
    if !any {
        return None;
    }
    let planned = plan(content, &stripped).ok()?;
    let lands_where_numbered = planned.iter().zip(&expected_lines).all(|(plan, expected)| {
        expected.is_none_or(|line| {
            plan.ranges.first().is_some_and(|range| line_of(content, range.start) == line)
        })
    });
    lands_where_numbered.then_some(planned)
}

/// `text` without a `12: ` prefix on each line, and the first line's number, when every line has
/// one. `consecutive` also requires the numbers to count up by one, as `read` writes them.
fn strip_read_numbers(text: &str, consecutive: bool) -> Option<(usize, String)> {
    let (body, trailing_newline) = match text.strip_suffix('\n') {
        Some(body) => (body, true),
        None => (text, false),
    };
    let mut first = None;
    let mut lines = Vec::new();
    for (index, line) in body.split('\n').enumerate() {
        let (number, rest) = line.split_once(':')?;
        if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        let number: usize = number.parse().ok()?;
        let first_number = *first.get_or_insert(number);
        if consecutive && number != first_number + index {
            return None;
        }
        // `read` writes `N: text`, and an empty line as `N: `, which a model may trim to `N:`
        lines.push(rest.strip_prefix(' ').unwrap_or(rest));
    }
    let mut out = lines.join("\n");
    if trailing_newline {
        out.push('\n');
    }
    Some((first?, out))
}

/// Whether every place this edit lands already holds exactly what it would write.
///
/// Compared against the file's own bytes rather than `oldText` against `newText`, because the two
/// are not the same question: a relaxed match re-indents the replacement, and line endings are
/// normalized on both sides before this runs, so an edit can arrive looking like a change and
/// resolve to none. What matters is whether the file moves.
fn writes_back_what_is_there(
    content: &str,
    ranges: &[std::ops::Range<usize>],
    replacement: &str,
) -> bool {
    ranges.iter().all(|range| content.get(range.clone()) == Some(replacement))
}

/// The first pair of edits that would write over each other, as an error.
fn overlap(planned: &[Planned]) -> Option<String> {
    let mut spans: Vec<(usize, std::ops::Range<usize>)> = planned
        .iter()
        .enumerate()
        .flat_map(|(index, plan)| plan.ranges.iter().cloned().map(move |range| (index, range)))
        .collect();
    spans.sort_by_key(|(_, range)| range.start);
    spans.windows(2).find_map(|pair| {
        let ((left, first), (right, second)) = (&pair[0], &pair[1]);
        (first.end > second.start).then(|| {
            format!(
                "Error: edit[{left}] and edit[{right}] cover the same text, so applying both \
                 would corrupt it. Nothing was written — split them or widen one so they \
                 do not overlap."
            )
        })
    })
}

/// Applies planned edits by byte offset, last first, so earlier offsets stay valid.
fn apply(content: &str, planned: &[Planned]) -> String {
    let mut spans: Vec<(&std::ops::Range<usize>, &str)> = planned
        .iter()
        .flat_map(|plan| plan.ranges.iter().map(move |range| (range, plan.replacement.as_str())))
        .collect();
    spans.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
    let mut result = content.to_string();
    for (range, replacement) in spans {
        result.replace_range(range.clone(), replacement);
    }
    result
}

impl EditTool {
    /// Read-before-edit and the staleness guard, when a tracker is wired.
    ///
    /// Recoverable by design: the model reads (or re-reads) the file and retries.
    fn check_read_guard(
        &self,
        cx: &ToolContext,
        full_path: &Path,
        shown: &str,
    ) -> Result<(), ToolResultData> {
        let Some(tracker) = &self.tracker else { return Ok(()) };
        match tracker.status(&cx.session_id, full_path) {
            ReadStatus::Unseen => Err(ToolResultData::error(format!(
                "Error: no read of '{shown}' is on record for this Luxe run \
                 ({READ_BEFORE_EDIT}). The record is per process, so a resumed \
                 session starts empty even where the transcript shows an earlier read. \
                 Use the `read` tool, then retry — nothing was written."
            ))),
            ReadStatus::Stale => Err(ToolResultData::error(format!(
                "Error: '{shown}' {CHANGED_ON_DISK} you last read it (a formatter, \
                 another tool, or an external edit). Re-read it, then retry the edit — \
                 nothing was written."
            ))),
            ReadStatus::Current => Ok(()),
        }
    }
}

/// What the edit says it did: the count, any tolerated matches, and what the model should know
/// before its next edit.
fn render_summary(
    planned: &[Planned],
    content: &str,
    shown_path: String,
    reformatted: bool,
    unnumbered: bool,
) -> String {
    let occurrences: usize = planned.iter().map(|plan| plan.ranges.len()).sum();
    let mut summary = format!(
        "Applied {} edit(s){} to '{shown_path}'",
        planned.len(),
        match occurrences.saturating_sub(planned.len()) {
            0 => String::new(),
            extra => format!(" ({extra} extra occurrence(s) via replaceAll)"),
        },
    );
    if reformatted {
        summary.push_str(
            "\n  Format-on-save then reformatted the file; the diff shows it as it is now.",
        );
    }
    // A tolerated match is never a silent one: the model asked for text that is not what is
    // in the file, and it needs to know which of the two the next edit should be based on.
    for (index, plan) in planned.iter().enumerate() {
        if let Some(note) = plan.kind.note() {
            let line = plan.ranges.first().map_or(0, |range| line_of(content, range.start));
            summary.push_str(&format!("\n  edit[{index}] at line {line}: {note}"));
        }
    }
    if unnumbered {
        summary.push_str(
            "\n  The `12: ` line numbers `read` shows were left out: they are not part of the \
             file, so leave them out of the next edit.",
        );
    }
    summary
}

#[async_trait]
impl Tool for EditTool {
    fn name(&self) -> &str {
        "edit"
    }

    fn argument_aliases(&self) -> &'static [&'static str] {
        &args::ALIASES
    }

    fn schema(&self) -> ToolSchema {
        edit_schema()
    }

    async fn run(&self, arguments: serde_json::Value, cx: &ToolContext) -> ToolResultData {
        let request = match normalize(&arguments) {
            Ok(request) => request,
            Err(problem) => return ToolResultData::error(problem.message()),
        };

        let full_path = cx.cwd.join(&request.path);

        if let Err(refusal) = self.check_read_guard(cx, &full_path, &request.path) {
            return refusal;
        }

        debug!(path = %full_path.display(), edits = request.edits.len(), "editing file");

        let content = match std::fs::read_to_string(&full_path) {
            Ok(content) => content,
            Err(problem) => {
                warn!(path = %full_path.display(), error = %problem, "failed to read file for edit");
                return ToolResultData::error(format!(
                    "Error reading file '{}': {problem}",
                    luxe_core::paths::display(&full_path)
                ));
            }
        };

        // Speak the file's line endings on both sides. The whole-line relaxation trims a stray `\r`
        // and would match a CRLF checkout anyway, then write LF back — the edited lines become the
        // only LF lines in the file and git reports the whole thing as changed.
        let ending = crate::line_endings::detect(&content);
        let edits: Vec<EditSpec> = request
            .edits
            .iter()
            .map(|edit| EditSpec {
                old_text: crate::line_endings::normalize(&edit.old_text, ending),
                new_text: crate::line_endings::normalize(&edit.new_text, ending),
                replace_all: edit.replace_all,
            })
            .collect();

        // Plan everything against the original text before writing anything: atomicity is the
        // promise that a rejected call leaves the file exactly as the model last read it.
        let (planned, unnumbered) = match plan(&content, &edits) {
            Ok(planned) => (planned, false),
            Err(message) => match plan_without_read_numbers(&content, &edits) {
                Some(planned) => (planned, true),
                None => return ToolResultData::error(message),
            },
        };
        // Previewed from the resolved ranges, not by re-finding the search text: a tolerated
        // match is not a substring of the file, and a `replaceAll` has several homes. Built
        // against the pre-edit content, so the line numbers match the file the model read.
        let preview: Vec<(std::ops::Range<usize>, &str)> = planned
            .iter()
            .flat_map(|plan| {
                plan.ranges.iter().map(move |range| (range.clone(), plan.replacement.as_str()))
            })
            .collect();
        let diff = crate::diff_preview::ranges_diff(&content, &preview);
        let result = apply(&content, &planned);

        if let Err(problem) = luxe_core::fs_atomic::replace(&full_path, result.as_bytes()) {
            warn!(path = %full_path.display(), error = %problem, "failed to write edited file");
            return ToolResultData::error(format!(
                "Error writing file '{}': {problem}",
                luxe_core::paths::display(&full_path)
            ));
        }
        debug!(path = %full_path.display(), edits = planned.len(), "file edited successfully");

        // format-on-save and diagnostics run here, before the fingerprint below
        let injected = cx.file_changed(&full_path, "edit").await;

        // Format-on-save may have rewritten what was just written, and the next edit is matched
        // against the file, not against the call: preview what is there now
        let formatted = std::fs::read_to_string(&full_path).ok().filter(|now| *now != result);
        let diff = match &formatted {
            Some(now) => crate::diff_preview::texts_diff(&content, now),
            None => diff,
        };

        // Refresh the fingerprint AFTER format-on-save so a follow-up edit to the same file
        // doesn't false-trip the staleness guard on the formatter's own change.
        if let Some(tracker) = &self.tracker {
            tracker.record(&cx.session_id, &full_path);
        }

        let summary = render_summary(
            &planned,
            &content,
            luxe_core::paths::display(full_path.strip_prefix(&cx.cwd).unwrap_or(&full_path)),
            formatted.is_some(),
            unnumbered,
        );

        let mut blocks = vec![ContentBlock::Text(format!("{summary}\n\n{diff}"))];
        if let Some(context) = injected {
            blocks.push(ContentBlock::Text(format!("\n--- Diagnostics ---\n{context}")));
        }
        ToolResultData { content: blocks, is_error: false }
    }
}

#[cfg(test)]
pub(crate) mod tests;

#[cfg(test)]
mod ergonomics_tests;
