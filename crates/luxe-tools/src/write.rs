//! `write` tool — write content to a file, creating parents as needed.
//!
//! Supports:
//! - Writing entire files (overwrite or create)
//! - Creating parent directories automatically
//! - Atomic writes (write to temp, then rename)

use async_trait::async_trait;
use luxe_core::read_tracker::{ReadStatus, ReadTracker};
use luxe_core::tool::guards::{CHANGED_ON_DISK, READ_BEFORE_WRITE};
use luxe_core::{ContentBlock, Tool, ToolContext, ToolResultData, ToolSchema};
use serde_json::json;
use std::path::Path;
use std::sync::Arc;
use tracing::{debug, warn};

/// JSON-schema for the write tool arguments.
fn write_schema() -> ToolSchema {
    ToolSchema {
        name: "write".to_string(),
        description: "Write a whole file, creating parent directories as needed. Best for NEW \
                      files; for targeted changes to an existing file use `edit`. Overwriting an \
                      existing file requires having `read` it in this Luxe run (and it not \
                      having changed since) so you don't clobber content you never saw."
            .to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to write (relative to cwd)"
                },
                "content": {
                    "type": "string",
                    "description": "Content to write to the file"
                }
            },
            "required": ["path", "content"]
        }),
    }
}

/// The `write` tool implementation.
#[derive(Default)]
pub struct WriteTool {
    /// Shared read tracker enforcing read-before-overwrite for existing files (skipped when absent).
    tracker: Option<Arc<ReadTracker>>,
}

impl WriteTool {
    /// Create a new write tool instance.
    pub fn new() -> Self {
        Self { tracker: None }
    }

    /// Attach the shared read tracker so overwriting an existing file requires a prior read.
    pub fn with_tracker(mut self, tracker: Arc<ReadTracker>) -> Self {
        self.tracker = Some(tracker);
        self
    }
}

impl WriteTool {
    /// The blind-overwrite guard, when a tracker is wired.
    ///
    /// Creating a NEW file is always fine, but overwriting an EXISTING file wholesale requires
    /// having read it first (and it not having changed since) so the model doesn't clobber content
    /// it never saw. `edit` is the right tool for targeted changes; this catches accidental
    /// full-file overwrites.
    fn check_blind_overwrite(
        &self,
        cx: &ToolContext,
        full_path: &Path,
        shown: &str,
    ) -> Result<(), ToolResultData> {
        let Some(tracker) = &self.tracker else { return Ok(()) };
        if !full_path.exists() {
            return Ok(());
        }
        match tracker.status(&cx.session_id, full_path) {
            ReadStatus::Unseen => Err(ToolResultData::error(format!(
                "Error: '{shown}' exists and has no read on record for this Luxe run \
                 ({READ_BEFORE_WRITE}), so overwriting it would be blind. \
                 The record is per process, so a resumed session starts empty \
                 even where the transcript shows an earlier read. Read it (or \
                 use `edit` for a targeted change), then retry — nothing was \
                 written."
            ))),
            ReadStatus::Stale => Err(ToolResultData::error(format!(
                "Error: '{shown}' {CHANGED_ON_DISK} you last read it (a formatter, \
                 another tool, or an external edit). Re-read it, then retry the \
                 write — nothing was written."
            ))),
            ReadStatus::Current => Ok(()),
        }
    }

    /// What happens once the bytes are down: format-on-save and diagnostics, the fingerprint, and
    /// the reply.
    async fn after_write(
        &self,
        cx: &ToolContext,
        full_path: &Path,
        content: &str,
        existed: bool,
    ) -> ToolResultData {
        // format-on-save and diagnostics run here, before the fingerprint below
        let injected = cx.file_changed(full_path, "write").await;

        // Format-on-save may have rewritten it; show what is there now
        let formatted = std::fs::read_to_string(full_path).ok().filter(|now| *now != *content);

        // Record the freshly-written file so a subsequent edit/write isn't blocked.
        if let Some(tracker) = &self.tracker {
            tracker.record(&cx.session_id, full_path);
        }

        // Show the written content as an all-additions block (the TUI colorizes it) so a
        // write reads like the edit diff rather than an opaque byte count.
        let verb = if existed { "Overwrote" } else { "Created" };
        let preview = crate::diff_preview::added_lines(formatted.as_deref().unwrap_or(content));
        let note = if formatted.is_some() {
            "\n  Format-on-save then reformatted the file; the lines below are as it is now."
        } else {
            ""
        };
        let mut result = vec![ContentBlock::Text(format!(
            "{verb} '{}' ({} bytes){note}\n\n{preview}",
            luxe_core::paths::display(full_path.strip_prefix(&cx.cwd).unwrap_or(full_path)),
            content.len()
        ))];
        if let Some(context) = injected {
            result.push(ContentBlock::Text(format!("\n--- Diagnostics ---\n{context}")));
        }
        ToolResultData { content: result, is_error: false }
    }
}

#[async_trait]
impl Tool for WriteTool {
    fn name(&self) -> &str {
        "write"
    }

    fn schema(&self) -> ToolSchema {
        write_schema()
    }

    async fn run(&self, args: serde_json::Value, cx: &ToolContext) -> ToolResultData {
        let path_str = match args.get("path").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => {
                return ToolResultData::error("Error: 'path' argument is required");
            }
        };

        let content = match args.get("content").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => {
                return ToolResultData::error("Error: 'content' argument is required");
            }
        };

        // Resolve path relative to cwd.
        let full_path = cx.cwd.join(path_str);

        if let Err(refusal) = self.check_blind_overwrite(cx, &full_path, path_str) {
            return refusal;
        }

        // Captured before the write so the result can say whether this created or replaced a file.
        let existed = full_path.exists();
        debug!(path = %full_path.display(), "writing file");

        // Keep the file's own line endings. A Windows checkout is CRLF and the model writes LF, so
        // a straight overwrite would silently convert every line in the file.
        let content = match std::fs::read_to_string(&full_path) {
            Ok(previous) => std::borrow::Cow::Owned(crate::line_endings::normalize(
                content,
                crate::line_endings::detect(&previous),
            )),
            Err(_) => std::borrow::Cow::Borrowed(content),
        };

        match luxe_core::fs_atomic::replace(&full_path, content.as_bytes()) {
            Ok(()) => {
                debug!(path = %full_path.display(), "file written successfully");
                self.after_write(cx, &full_path, &content, existed).await
            }
            Err(e) => {
                warn!(path = %full_path.display(), error = %e, "failed to write file");
                ToolResultData::error(format!(
                    "Error writing file '{}': {}",
                    luxe_core::paths::display(&full_path),
                    e
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests;
