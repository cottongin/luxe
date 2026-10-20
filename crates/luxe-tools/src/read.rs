//! `read` tool — read a file's contents, or look at it when it is a picture.
//!
//! Text reads page with offset/limit and report their own truncation. An **image** file comes back
//! as a real image block beside a line of text describing it, so a vision-capable model sees the
//! picture and the TUI can draw it — the same path a `screenshot` takes. Before that, pointing
//! `read` at a PNG got "this is a binary file, there is nothing to read as source", which is true
//! and useless: the file is a design, a chart, a mockup, and looking at it together is the whole
//! job.

mod image_file;

use async_trait::async_trait;
use luxe_core::read_tracker::ReadTracker;
use luxe_core::{ContentBlock, Tool, ToolContext, ToolResultData, ToolSchema};
use serde_json::json;
use std::sync::Arc;
use tracing::{debug, warn};

/// Character ceiling for one read, so a minified single-line file cannot flood the context — the
/// same one every tool that answers promises.
const READ_MAX_CHARS: usize = luxe_core::output_limit::RESULT_MAX_CHARS;

/// Default line cap for a read with no explicit `limit`, so a huge file can't flood context.
const READ_DEFAULT_LIMIT: usize = 2000;

/// JSON-schema for the read tool arguments.
fn read_schema() -> ToolSchema {
    ToolSchema {
        name: "read".to_string(),
        description: "Read a file's contents, or several at once with `paths` — one call for what \
                      `cat a b c` would show, each file under its own header. Line numbers are \
                      1-based: `offset` is the first line \
                      to read and `limit` caps how many lines, matching the line numbers reported by \
                      diagnostics, navigation, and file:line references. That pair *is* the line \
                      range — reaching for a shell one-liner to slice one costs a round-trip and \
                      says nothing about where the slice sits in the file. Each line comes back \
                      prefixed with its number, `12: `, which is not part of the file: leave it \
                      out of an `edit`. A result that stops early says how many lines remain and \
                      the exact `offset` to continue from.\n\
                      An image file (PNG/JPEG/GIF/WebP) comes back as the image itself, downscaled \
                      for the model, so a design, chart, or mockup can simply be looked at \u{2014} \
                      offset/limit do not apply to one.\n\
                      Reading one function or type is `read_symbol`, which finds it by name instead \
                      of a line range you have to guess; a file's shape without its bodies is \
                      `module_report`."
            .to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to read (relative to cwd)"
                },
                "paths": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Several files to read in one call (up to 10); instead of `path`. `offset` and `limit` apply to each"
                },
                "offset": {
                    "type": "integer",
                    "description": "First line to read, 1-based (optional; default 1)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of lines to read (optional)"
                }
            },
            "required": []
        }),
    }
}

/// The `read` tool implementation.
#[derive(Default)]
pub struct ReadTool {
    /// Shared read tracker; records each read so `edit`/`write` can enforce read-before-mutate.
    tracker: Option<Arc<ReadTracker>>,
}

impl ReadTool {
    /// Create a new read tool instance.
    pub fn new() -> Self {
        Self { tracker: None }
    }

    /// Attach the shared read tracker so reads are recorded for the mutation guards.
    pub fn with_tracker(mut self, tracker: Arc<ReadTracker>) -> Self {
        self.tracker = Some(tracker);
        self
    }
}

#[async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &str {
        "read"
    }

    fn schema(&self) -> ToolSchema {
        read_schema()
    }

    async fn run(&self, args: serde_json::Value, cx: &ToolContext) -> ToolResultData {
        // 1-based line numbers (match diagnostics/nav/file:line); 0 or missing is treated as line 1.
        let offset = args.get("offset").and_then(|v| v.as_i64()).unwrap_or(1).max(1) as usize;
        let limit = args.get("limit").and_then(|v| v.as_i64()).map(|l| l.max(1) as usize);
        let range = (offset, limit);
        let paths: Vec<&str> = args
            .get("paths")
            .and_then(|v| v.as_array())
            .map(|paths| paths.iter().filter_map(|path| path.as_str()).collect())
            .unwrap_or_default();
        let path_str = match args.get("path").and_then(|v| v.as_str()) {
            Some(s) => s,
            None if !paths.is_empty() => return self.read_many(&paths, range, cx).await,
            None => {
                return ToolResultData::error(
                    "Error: 'path' (or 'paths') argument is required".to_string(),
                );
            }
        };
        if !paths.is_empty() {
            let mut every = vec![path_str];
            every.extend(paths.iter().filter(|path| **path != path_str));
            return self.read_many(&every, range, cx).await;
        }
        self.read_one(path_str, range.0, range.1, cx).await
    }
}

/// The most files one `read` takes. Enough for a look around a change; past it, a model is
/// reading a directory, which is `glob` and then a choice.
const READ_MANY_MAX: usize = 10;

impl ReadTool {
    /// Each of `paths`, under a header naming it, in one result.
    ///
    /// The point is the round trip: `cat a b c` answers in one call and three `read`s looked like
    /// three, which is a large part of why a model reached for the shell to look around. Each file
    /// is read exactly as alone — guards, images, truncation — and a file that cannot be read says
    /// why under its header without failing the rest. `range` is `(offset, limit)` for every file,
    /// since a caller who asked for twenty lines of each did not mean all of them.
    async fn read_many(
        &self,
        paths: &[&str],
        range: (usize, Option<usize>),
        cx: &ToolContext,
    ) -> ToolResultData {
        let mut content = Vec::new();
        let mut text = String::new();
        let mut failed = 0;
        for path in paths.iter().take(READ_MANY_MAX) {
            let one = self.read_one(path, range.0, range.1, cx).await;
            failed += usize::from(one.is_error);
            if !text.is_empty() {
                text.push_str("\n\n");
            }
            text.push_str(&format!("── {path} ──\n"));
            for block in one.content {
                match block {
                    ContentBlock::Text(body) => text.push_str(&body),
                    image => content.push(image),
                }
            }
        }
        if paths.len() > READ_MANY_MAX {
            text.push_str(&format!(
                "\n\n-- {} more not read: one call takes {READ_MANY_MAX} files --",
                paths.len() - READ_MANY_MAX
            ));
        }
        let text = crate::output_limit::cap_chars(
            &text,
            READ_MAX_CHARS,
            "Read fewer files at once, or a range of one with offset/limit.",
        );
        content.insert(0, ContentBlock::Text(text));
        ToolResultData { content, is_error: failed == paths.len().min(READ_MANY_MAX) }
    }

    /// One file, from `offset` for at most `limit` lines.
    async fn read_one(
        &self,
        path_str: &str,
        offset: usize,
        limit: Option<usize>,
        cx: &ToolContext,
    ) -> ToolResultData {
        let full_path = cx.cwd.join(&*luxe_core::paths::native(path_str));

        debug!(path = %full_path.display(), offset, limit = limit.unwrap_or(usize::MAX), "reading file");

        // A picture is not a failed text file. Sniffed by content rather than by extension: a
        // `.txt` holding a PNG is still a PNG, and a `.png` holding markup is still markup.
        match image_file::read(&full_path, path_str) {
            image_file::Outcome::NotAnImage => {}
            image_file::Outcome::Read(result) => {
                if let Some(tracker) = &self.tracker {
                    tracker.record(&cx.session_id, &full_path);
                }
                return result;
            }
        }

        // Read the file.
        let content = match std::fs::read_to_string(&full_path) {
            Ok(c) => c,
            Err(e) => {
                warn!(path = %full_path.display(), error = %e, "failed to read file");
                // "stream did not contain valid UTF-8" is true and useless. Say what it is and
                // what to do about it, so the model stops rather than retrying the same read.
                let message = if e.kind() == std::io::ErrorKind::InvalidData {
                    let size = std::fs::metadata(&full_path).map(|m| m.len()).unwrap_or(0);
                    format!(
                        "'{}' is a binary file ({size} bytes), not text — there is nothing to read \
                         as source. Use `bash` if you genuinely need its bytes.",
                        path_str
                    )
                } else {
                    format!("Error reading file '{}': {}", luxe_core::paths::display(&full_path), e)
                };
                return ToolResultData::error(message);
            }
        };

        // Apply offset/limit (offset is 1-based; convert to a 0-based slice index).
        let lines: Vec<&str> = content.lines().collect();
        let start = offset.saturating_sub(1).min(lines.len());
        // A read with no explicit limit is capped at a sane default so a huge file can't flood
        // context in one call; the truncation note steers the model to page with offset/limit.
        let end = match limit {
            Some(l) => (start + l).min(lines.len()),
            None => (start + READ_DEFAULT_LIMIT).min(lines.len()),
        };

        // An empty file is a fact, not a failure — reporting "offset exceeds file length" for one
        // sends the model hunting for a bad argument it never passed.
        if lines.is_empty() {
            return ToolResultData::text(format!("'{path_str}' is empty (0 lines)."));
        }

        if start >= lines.len() {
            return ToolResultData::error(format!(
                "Error: offset {offset} is past the end of '{path_str}' ({} lines).",
                lines.len()
            ));
        }

        // Record that this session has now seen the file (any successful read counts) so the
        // edit/write guards accept a subsequent mutation.
        if let Some(tracker) = &self.tracker {
            tracker.record(&cx.session_id, &full_path);
        }

        ToolResultData::text(window(&lines, start, end, limit.is_some()))
    }
}

/// `lines[start..end]` as a result, each line numbered, kept under [`READ_MAX_CHARS`] by cutting at
/// a line boundary.
///
/// Numbered because luxe's other tools take line numbers as input (`lsp_navigation` positions,
/// `debug` breakpoints, `read`'s own `offset`), and a line the agent has to count to is one it gets
/// wrong. A result that stops early closes with the exact `offset` to continue from. The line cap
/// cannot bound a minified file, one enormous line, so the character budget is counted as lines
/// are taken, and a single line too long to show says so rather than suggesting an offset that
/// cannot help.
fn window(lines: &[&str], start: usize, end: usize, explicit_limit: bool) -> String {
    let total = lines.len();
    let mut body = String::new();
    let mut budget = READ_MAX_CHARS;
    let mut shown_end = start;
    for (index, line) in lines[start..end].iter().enumerate() {
        let numbered = format!("{}: {line}\n", start + index + 1);
        let cost = numbered.chars().count();
        if cost > budget {
            break;
        }
        body.push_str(&numbered);
        budget -= cost;
        shown_end += 1;
    }
    // A first line too long to show: what fits of it, and the honest way to the rest
    if shown_end == start {
        let line = lines[start];
        let kept: String = line.chars().take(READ_MAX_CHARS).collect();
        return format!(
            "{}: {kept}\n-- line {} is {} characters, more than one read returns; `grep` the file \
             for what you need --",
            start + 1,
            start + 1,
            line.chars().count()
        );
    }
    let mut out = body.trim_end_matches('\n').to_string();
    if shown_end < total {
        let why = if shown_end < end {
            format!(" (stopped at the {READ_MAX_CHARS}-character cap)")
        } else if !explicit_limit {
            format!(" (a read without `limit` stops at {READ_DEFAULT_LIMIT} lines)")
        } else {
            String::new()
        };
        out.push_str(&format!(
            "\n-- {} more of {total} lines{why}; continue with offset {} --",
            total - shown_end,
            shown_end + 1
        ));
    }
    out
}

#[cfg(test)]
mod tests;
