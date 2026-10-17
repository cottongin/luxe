//! Summary — what the summarizer reads, and the shape its reply is filed in.

use crate::message::{ContentBlock, ToolOutput};
use crate::session::Entry;
use std::collections::HashSet;

/// Characters of real prose a summary has to carry to be allowed to stand for a transcript.
///
/// Deliberately near the floor: judging a summary's *quality* from here would throw away terse but
/// perfectly good ones, and the failure this exists for is not a thin summary but an absent one.
const SUMMARY_MUST_CARRY_CHARS: usize = 40;

/// Whether a formatted summary carries enough to replace what it summarizes.
///
/// The headings are added by [`format_structured_summary`] whether or not the summarizer wrote
/// them, and the file list is derived from the transcript rather than from the model — so a reply
/// that came back empty still formats into a full-looking document of empty sections, and would
/// replace the session's whole memory with it.
///
/// Counted over the prose only: headings and the file list are structure, not content.
pub fn summary_carries_content(formatted: &str) -> bool {
    let mut in_files = false;
    let mut prose = 0usize;
    for line in formatted.lines() {
        if let Some(heading) = line.trim_end().strip_prefix("## ") {
            in_files = FILE_SECTION_ALIASES.contains(&heading.trim());
            continue;
        }
        if !in_files {
            prose += line.trim().chars().count();
        }
    }
    prose >= SUMMARY_MUST_CARRY_CHARS
}

/// The heading the summary lists touched files under.
const FILES_SECTION: &str = "Files";

/// Headings a file list may be read back from.
///
/// [`FILES_SECTION`] was two sections, `Read Files` and `Modified Files`, in older builds. A
/// session compacted then still has them on disk, and dropping its list on resume would lose
/// exactly the history the summary exists to keep.
const FILE_SECTION_ALIASES: [&str; 3] = [FILES_SECTION, "Read Files", "Modified Files"];

/// Tools whose `path` argument names a file the session actually touched.
///
/// Deliberately not every tool with a path: `glob` and `grep` take one as a *scope*, and a search
/// that swept the repository has not touched the repository.
const FILE_TOOLS: [&str; 3] = ["read", "write", "edit"];

/// The files a previous summary listed, so a chain of compactions keeps the record.
///
/// Scoped to the file headings: over the whole document, any bullet in `Progress` or `Next Steps`
/// with a dot in it would be filed as a filename, one compaction at a time.
pub(super) fn files_listed_in(summary: &str) -> HashSet<String> {
    let mut files = HashSet::new();
    let mut inside = false;
    for line in summary.lines() {
        let line = line.trim();
        if let Some(heading) = line.strip_prefix("## ") {
            inside = FILE_SECTION_ALIASES.contains(&heading.trim());
            continue;
        }
        let listed =
            inside.then(|| line.strip_prefix("- ").or_else(|| line.strip_prefix("* "))).flatten();
        if let Some(file) = listed.map(str::trim).filter(|file| !file.is_empty()) {
            files.insert(file.to_string());
        }
    }
    files
}

/// The files a stretch of conversation opened or changed, read off the calls that named them.
///
/// The transcript is the only honest source: the summarizer is asked to list files and writes
/// prose, and the previous summary can only ever repeat what some earlier segment found. Reading
/// the tool calls is what makes the list accumulate at all.
pub(super) fn files_touched(entries: &[Entry]) -> HashSet<String> {
    let mut files = HashSet::new();
    for entry in entries {
        let crate::session::EntryKind::Message(message) = &entry.kind else { continue };
        for block in &message.content {
            let ContentBlock::ToolCall { name, arguments, .. } = block else { continue };
            if !FILE_TOOLS.contains(&name.as_str()) {
                continue;
            }
            if let Some(path) = arguments.get("path").and_then(serde_json::Value::as_str) {
                let path = path.trim();
                if !path.is_empty() {
                    files.insert(path.to_string());
                }
            }
        }
    }
    files
}

/// How much of any one block the summarizer is shown.
///
/// Its input is a whole context window by construction, so one `cargo build` log left whole would
/// spend the entire summary budget on a build that succeeded.
const SUMMARY_BLOCK_CHARS: usize = 2_000;

/// Render a stretch of transcript as the text the summarizer reads.
///
/// Tool activity goes in, because in a coding session it *is* the session: a summary written from
/// the prose alone cannot say what was read, what changed, or what the tests did. Reasoning stays
/// out — it is the model talking to itself on the way to the answer, and the answer is already
/// here.
pub fn serialize_for_summary(entries: &[Entry]) -> String {
    let mut output = String::new();
    for entry in entries {
        match &entry.kind {
            crate::session::EntryKind::Message(message) => {
                let role = match message.role {
                    crate::message::Role::User => "User",
                    crate::message::Role::Assistant => "Assistant",
                    crate::message::Role::Tool => "Tool",
                    crate::message::Role::System => "System",
                };
                for block in &message.content {
                    if let Some(line) = summary_line(block) {
                        output.push_str(&format!("[{role}]: {line}\n"));
                    }
                }
            }
            crate::session::EntryKind::Compaction { summary, .. } => {
                output.push_str(&format!("[Compaction]: {summary}\n"));
            }
            _ => {}
        }
    }
    output
}

/// Whether an entry puts anything into [`serialize_for_summary`]'s output.
///
/// The mirror of that function's `match`, and the reason it is a predicate rather than a length
/// check on the serialized string: the decision to compact is made before the input is built.
pub(super) fn contributes_to_summary(entry: &Entry) -> bool {
    match &entry.kind {
        crate::session::EntryKind::Message(message) => {
            message.content.iter().any(|block| summary_line(block).is_some())
        }
        crate::session::EntryKind::Compaction { .. } => true,
        _ => false,
    }
}

/// One content block as a line of summarizer input, or `None` when it has nothing to contribute.
fn summary_line(block: &ContentBlock) -> Option<String> {
    match block {
        ContentBlock::Text(text) => Some(clip_for_summary(text)),
        ContentBlock::ToolCall { name, arguments, .. } => {
            Some(format!("called {name} {}", clip_for_summary(&arguments.to_string())))
        }
        ContentBlock::ToolResult { content, is_error, .. } => {
            let body = content
                .iter()
                .map(|output| match output {
                    ToolOutput::Text(text) => text.as_str(),
                    ToolOutput::Image { .. } => "<image>",
                })
                .collect::<Vec<_>>()
                .join("\n");
            let label = if *is_error { "failed" } else { "returned" };
            Some(format!("{label} {}", clip_for_summary(&body)))
        }
        ContentBlock::Thinking { .. }
        | ContentBlock::RedactedThinking { .. }
        | ContentBlock::Image { .. } => None,
    }
}

/// Cap one block at [`SUMMARY_BLOCK_CHARS`], counted in characters.
///
/// Counted in characters rather than bytes because slicing a byte offset panics the moment it lands
/// inside a multi-byte one — which for a transcript carrying an accent or an emoji is a crash in
/// the middle of a compaction, on a turn the user never asked for.
fn clip_for_summary(text: &str) -> String {
    let mut kept: String = text.chars().take(SUMMARY_BLOCK_CHARS).collect();
    if kept.chars().count() == SUMMARY_BLOCK_CHARS
        && text.chars().nth(SUMMARY_BLOCK_CHARS).is_some()
    {
        kept.push_str("...");
    }
    kept
}

/// The sections every summary carries, whether or not the summarizer wrote them.
///
/// An empty heading is deliberate: it tells the next model that the question was asked and had no
/// answer, which reads very differently from the section having been forgotten.
const REQUIRED_SECTIONS: [&str; 6] = [
    "Goal",
    "Constraints & Preferences",
    "Progress",
    "Key Decisions",
    "Next Steps",
    "Critical Context",
];

/// Shape the summarizer's prose into the sections a resumed session expects, and append the file
/// record it cannot write for itself.
///
/// `files` is the running list from [`super::CompactionPlan::file_ops`] — already the union of what
/// the last summary recorded and what this segment touched — so this only has to render it.
pub fn format_structured_summary(summarizer_output: &str, files: &HashSet<String>) -> String {
    let mut sections: Vec<(String, String)> = Vec::new();
    let mut current: Option<(String, String)> = None;

    for line in summarizer_output.lines() {
        match line.trim_end().strip_prefix("## ") {
            Some(heading) => {
                sections.extend(current.take());
                current = Some((heading.trim().to_string(), String::new()));
            }
            None => {
                if let Some((_, content)) = current.as_mut() {
                    content.push_str(line);
                    content.push('\n');
                }
            }
        }
    }
    sections.extend(current);

    for required in REQUIRED_SECTIONS {
        if !sections.iter().any(|(heading, _)| heading == required) {
            sections.push((required.to_string(), String::new()));
        }
    }
    // Replaced rather than appended, so a summarizer that wrote its own file list does not leave
    // the summary carrying two of them that disagree.
    sections.retain(|(heading, _)| !FILE_SECTION_ALIASES.contains(&heading.as_str()));
    sections.push((FILES_SECTION.to_string(), format_files(files)));

    let mut output = String::new();
    for (heading, content) in &sections {
        output.push_str(&format!("## {heading}\n"));
        let content = content.trim();
        if !content.is_empty() {
            output.push_str(&format!("\n{content}\n"));
        }
        output.push('\n');
    }
    output
}

/// Format a list of files, sorted so two summaries of the same work read the same.
fn format_files(files: &HashSet<String>) -> String {
    let mut sorted: Vec<&String> = files.iter().collect();
    sorted.sort();
    sorted.iter().map(|file| format!("- {file}")).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests;
