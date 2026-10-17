//! Workspace edits — a language server's `WorkspaceEdit` applied to disk, all of it or none.
//!
//! The file list comes from the server, so it is planned in memory first: every file read, every
//! edit placed, and any file outside the project or inside a fetched repository refuses the whole
//! edit before a byte is written. Then each changed file is written, and a failed write puts back
//! the ones already done. The result counts what actually landed, says what was skipped and which
//! resource operations were not applied, and each written file goes through `FileChanged` and the
//! read tracker like an `edit` would.

use std::path::{Path, PathBuf};

use luxe_core::read_tracker::ReadTracker;
use luxe_core::{ToolContext, ToolResultData};
use serde_json::Value;

/// Where an applied edit reports to: the call's context and, when wired, the read tracker.
#[derive(Clone, Copy)]
pub(crate) struct Writes<'a> {
    pub(crate) cx: &'a ToolContext,
    pub(crate) tracker: Option<&'a ReadTracker>,
}

/// A single text edit as `(start_line, start_char, end_line, end_char, new_text)` in LSP coords.
type TextEdit = (u32, u32, u32, u32, String);

/// What a `WorkspaceEdit` asks for: text edits per file, and the resource operations it carried.
struct Requested {
    files: Vec<(String, Vec<TextEdit>)>,
    operations: Vec<String>,
}

/// One file's change, worked out in memory before anything is written.
struct FilePlan {
    path: PathBuf,
    display: String,
    original: String,
    updated: String,
    applied: usize,
    skipped: usize,
}

/// Apply a `WorkspaceEdit`, returning a summary plus any context `FileChanged` hooks injected.
pub(crate) async fn apply_workspace_edit(edit: &Value, writes: Writes<'_>) -> ToolResultData {
    let requested = collect_workspace_edits(edit);
    let plans = match plan(requested.files, &writes.cx.cwd) {
        Ok(plans) => plans,
        Err(message) => return ToolResultData::error(&message),
    };
    let changed: Vec<&FilePlan> =
        plans.iter().filter(|file| file.updated != file.original).collect();
    if let Err(message) = write_all(&changed) {
        return ToolResultData::error(&message);
    }
    let mut injected = Vec::new();
    for file in &changed {
        injected.extend(writes.cx.file_changed(&file.path, "edit").await);
        // after the hook, so a format-on-save rewrite is what counts as read
        if let Some(tracker) = writes.tracker {
            tracker.record(&writes.cx.session_id, &file.path);
        }
    }
    ToolResultData::text(summary(&plans, &requested.operations, &injected))
}

fn plan(files: Vec<(String, Vec<TextEdit>)>, cwd: &Path) -> Result<Vec<FilePlan>, String> {
    // one plan per file, or a second entry for it would be placed against the unedited text
    let mut merged: Vec<(PathBuf, Vec<TextEdit>)> = Vec::new();
    for (uri, edits) in files {
        // through the URI parser: a Windows drive or a percent-encoded space breaks a prefix strip
        let path = crate::uri::to_path(&uri);
        match merged.iter_mut().find(|(known, _)| *known == path) {
            Some((_, known)) => known.extend(edits),
            None => merged.push((path, edits)),
        }
    }
    merged.into_iter().map(|(path, edits)| plan_file(path, edits, cwd)).collect()
}

fn plan_file(path: PathBuf, edits: Vec<TextEdit>, cwd: &Path) -> Result<FilePlan, String> {
    let display = luxe_core::paths::display(&path);
    if let Some(reason) = refusal(&path, cwd) {
        return Err(format!("Error: the edit names {display}, {reason}; nothing was written"));
    }
    let original = std::fs::read_to_string(&path)
        .map_err(|e| format!("Error: read {display}: {e}; nothing was written"))?;
    let requested = edits.len();
    let (updated, applied) = apply_text_edits(&original, edits);
    Ok(FilePlan { path, display, original, updated, applied, skipped: requested - applied })
}

/// Why `path` may not be written from here, if it may not. Canonical on both sides, so a symlink
/// cannot lead out of the project.
fn refusal(path: &Path, cwd: &Path) -> Option<&'static str> {
    let canonical =
        |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let target = canonical(path);
    if !luxe_core::paths::contains(&canonical(cwd), &target) {
        return Some("outside the project");
    }
    luxe_core::fetched::contains(&target).then_some("inside a fetched repository")
}

/// Write every changed file, or put back the ones already written and say what could not be.
fn write_all(changed: &[&FilePlan]) -> Result<(), String> {
    for (index, file) in changed.iter().enumerate() {
        let Err(error) = luxe_core::fs_atomic::replace(&file.path, file.updated.as_bytes()) else {
            continue;
        };
        let unrestored: Vec<&str> = changed[..index]
            .iter()
            .filter(|done| {
                luxe_core::fs_atomic::replace(&done.path, done.original.as_bytes()).is_err()
            })
            .map(|done| done.display.as_str())
            .collect();
        let after = match unrestored.as_slice() {
            [] => "the files already written were restored".to_string(),
            files => format!("could not restore {}", files.join(", ")),
        };
        return Err(format!("Error: write {}: {error}; {after}", file.display));
    }
    Ok(())
}

fn summary(plans: &[FilePlan], operations: &[String], injected: &[String]) -> String {
    let changed: Vec<&FilePlan> = plans.iter().filter(|file| file.applied > 0).collect();
    let total: usize = changed.iter().map(|file| file.applied).sum();
    let mut out = if total == 0 {
        "Rename/edit produced no changes\n".to_string()
    } else {
        format!("Applied {total} edit(s) across {} file(s):\n", changed.len())
    };
    for file in &changed {
        out.push_str(&format!("  {} ({} edit(s))\n", file.display, file.applied));
    }
    let skipped: Vec<String> = plans
        .iter()
        .filter(|file| file.skipped > 0)
        .map(|file| format!("{} ({})", file.display, file.skipped))
        .collect();
    if !skipped.is_empty() {
        let count: usize = plans.iter().map(|file| file.skipped).sum();
        out.push_str(&format!(
            "skipped {count} edit(s) outside their file or overlapping another: {}\n",
            skipped.join(", ")
        ));
    }
    if !operations.is_empty() {
        out.push_str(&format!(
            "not applied: {} (file operations are not supported here; use write or bash)\n",
            operations.join(", ")
        ));
    }
    if !injected.is_empty() {
        out.push_str(&format!("\n--- Diagnostics ---\n{}", injected.join("\n")));
    }
    out
}

/// Collect a `WorkspaceEdit`'s per-file edits and resource operations.
///
/// `documentChanges` wins when both are present, as the spec says, so an edit sent both ways
/// lands once.
fn collect_workspace_edits(edit: &Value) -> Requested {
    let mut requested = Requested { files: Vec::new(), operations: Vec::new() };
    if let Some(document_changes) = edit.get("documentChanges").and_then(Value::as_array) {
        for change in document_changes {
            match change.get("textDocument").and_then(|d| d.get("uri")).and_then(Value::as_str) {
                Some(uri) => {
                    let edits = change.get("edits").map(parse_text_edits).unwrap_or_default();
                    requested.files.push((uri.to_string(), edits));
                }
                None => requested.operations.push(describe_operation(change)),
            }
        }
    } else if let Some(changes) = edit.get("changes").and_then(Value::as_object) {
        for (uri, edits) in changes {
            requested.files.push((uri.clone(), parse_text_edits(edits)));
        }
    }
    requested
}

fn describe_operation(change: &Value) -> String {
    let path = |key: &str| {
        change
            .get(key)
            .and_then(Value::as_str)
            .map(|uri| luxe_core::paths::display(&crate::uri::to_path(uri)))
            .unwrap_or_default()
    };
    match change.get("kind").and_then(Value::as_str).unwrap_or("unknown") {
        "rename" => format!("rename {} to {}", path("oldUri"), path("newUri")),
        kind => format!("{kind} {}", path("uri")),
    }
}

fn parse_text_edits(edits: &Value) -> Vec<TextEdit> {
    let Some(array) = edits.as_array() else { return Vec::new() };
    array
        .iter()
        .filter_map(|edit| {
            let range = edit.get("range")?;
            let start = range.get("start")?;
            let end = range.get("end")?;
            let num = |v: &Value, k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0) as u32;
            Some((
                num(start, "line"),
                num(start, "character"),
                num(end, "line"),
                num(end, "character"),
                edit.get("newText").and_then(Value::as_str).unwrap_or("").to_string(),
            ))
        })
        .collect()
}

/// Apply LSP text edits to `content`, returning the text and how many edits landed.
///
/// Descending start order, so an edit never shifts a range still to come. An edit on a line the
/// file does not have, or overlapping one already applied, is skipped rather than guessed at.
/// `character` offsets are UTF-16 code units (the LSP default).
fn apply_text_edits(content: &str, edits: Vec<TextEdit>) -> (String, usize) {
    let line_starts = line_start_bytes(content);
    let mut spans: Vec<(usize, usize, usize, String)> = edits
        .into_iter()
        .enumerate()
        .filter_map(|(index, (sl, sc, el, ec, new_text))| {
            let start = position_to_byte(content, &line_starts, sl, sc)?;
            let end = position_to_byte(content, &line_starts, el, ec)?;
            Some((start.min(end), index, start.max(end), new_text))
        })
        .collect();
    // edits sharing a start apply in the order sent, so the later one goes in first
    spans.sort_by_key(|span| std::cmp::Reverse((span.0, span.1)));
    let (mut out, mut applied, mut limit) = (content.to_string(), 0, content.len());
    for (start, _, end, new_text) in spans {
        if end <= limit {
            out.replace_range(start..end, &new_text);
            applied += 1;
            limit = start;
        }
    }
    (out, applied)
}

/// Byte offset of the start of each line (line 0 at 0, then after each `\n`).
fn line_start_bytes(content: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (index, byte) in content.bytes().enumerate() {
        if byte == b'\n' {
            starts.push(index + 1);
        }
    }
    starts
}

/// Convert an LSP `(line, character_utf16)` position to a byte offset, clamped to the line's end;
/// `None` for a line the file does not have.
fn position_to_byte(
    content: &str,
    line_starts: &[usize],
    line: u32,
    character: u32,
) -> Option<usize> {
    let line_start = *line_starts.get(line as usize)?;
    let mut byte = line_start;
    let mut utf16 = 0u32;
    for ch in content[line_start..].chars() {
        if ch == '\n' || utf16 >= character {
            break;
        }
        utf16 += ch.len_utf16() as u32;
        byte += ch.len_utf8();
    }
    Some(byte)
}

#[cfg(test)]
mod tests;
