//! `grep` tool — search file contents by regex.
//!
//! Searches in-process: candidates come from [`crate::walk`], every one passes read policy before
//! it is opened, and the approved files are scanned in parallel with `regex`. An external `rg`
//! could not do the middle step, which is the one that keeps denied content out of the answer.
//!
//! The result is bounded `file:line:text`, with optional context lines and a glob filter — or,
//! where the question is coarser than the lines, the files that match or a count per file.

use async_trait::async_trait;
use luxe_core::{ContentBlock, Tool, ToolContext, ToolResultData, ToolSchema};
use serde_json::json;
use std::path::{Path, PathBuf};
use tracing::warn;

mod guarded;
#[cfg(test)]
mod read_policy_tests;

/// JSON-schema for the grep tool arguments.
fn grep_schema() -> ToolSchema {
    ToolSchema {
        name: "grep".to_string(),
        description: "Search file *contents* by regex. Returns `file:line:text`, bounded \
                      — a broad pattern says it was capped instead of flooding your context. Case \
                      is sensitive unless the pattern says otherwise: prefix `(?i)`. Skips ignored \
                      paths (`.gitignore`, `.git/`, build output); set `all: true` to include \
                      them. Answer the coarse question coarsely: `outputMode: \"files\"` for \
                      which files match and `\"count\"` for how many lines each, both a fraction \
                      of the tokens of the lines themselves. Finding files by *name* is `glob`; \
                      finding a symbol's definition is `symbol_search`; finding everything that \
                      *uses* one \u{2014} callers, dependents, covering tests \u{2014} is `impact`, \
                      which reads a code graph rather than guessing at a pattern."
            .to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Regex pattern to search for. Prefix '(?i)' for a case-insensitive search."
                },
                "path": {
                    "type": "string",
                    "description": "File or directory to search (defaults to cwd)"
                },
                "glob": {
                    "type": "string",
                    "description": "Glob pattern to filter files (e.g., '*.rs')"
                },
                "outputMode": {
                    "type": "string",
                    "enum": ["content", "files", "count"],
                    "description": "What to return: 'content' (file:line:text, the default), \
                                    'files' (one path per matching file), or 'count' (file:count \
                                    of matching lines). Prefer 'files' when you only need to know \
                                    where to look."
                },
                "limit": {
                    "type": "integer",
                    "description": "Cap the rows returned (default and maximum 200). Lower it when \
                                    a sample will do — the equivalent of piping to 'head'."
                },
                "context": {
                    "type": "integer",
                    "description": "Context lines on both sides of each match (optional)"
                },
                "before": {
                    "type": "integer",
                    "description": "Context lines before each match; overrides 'context' on that side."
                },
                "after": {
                    "type": "integer",
                    "description": "Context lines after each match; overrides 'context' on that side."
                },
                "invert": {
                    "type": "boolean",
                    "description": "Return the lines that do NOT match the pattern. Default false."
                },
                "all": {
                    "type": "boolean",
                    "description": "Include ignored files (build output, vendored deps). Default false."
                }
            },
            "required": ["pattern"]
        }),
    }
}

/// Max rows returned to the model before the result is capped, and the ceiling on `limit`.
const GREP_MAX_LINES: usize = 200;

/// Character budget for one search, the second half of the bound the schema promises.
///
/// Sized against Luxe's own tree, where a broad pattern's 200 rows measure ~21 KB and the worst
/// measured — one reaching generated assets — came to 72 KB. So this leaves an ordinary search
/// untouched and cuts the pathological one to a third.
const GREP_MAX_CHARS: usize = 24_000;

/// Bytes of a file read before deciding it is binary and skipping it.
///
/// A NUL in the first few kilobytes is what every grep-alike uses, and it is right often enough:
/// the alternative is dumping a shared object's bytes into the context window.
const BINARY_SNIFF_BYTES: usize = 8192;

/// What a search answers with.
///
/// The lines are the expensive answer and often not the question: "which files mention this" and
/// "how many are there" each cost a fraction of the tokens, and a model that has to read 200 rows
/// to learn six filenames pays for the other 194.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum OutputMode {
    /// `file:line:text`
    #[default]
    Content,
    /// One path per file with at least one match
    Files,
    /// `file:count` of matching lines
    Count,
}

impl OutputMode {
    /// Parse the schema's spelling, falling back to the default for anything unrecognized.
    fn parse(value: Option<&str>) -> Self {
        match value {
            Some("files") => Self::Files,
            Some("count") => Self::Count,
            _ => Self::Content,
        }
    }

    /// What the cap counts, for the note it appends.
    fn unit(self) -> &'static str {
        match self {
            Self::Content => "matching lines",
            Self::Files | Self::Count => "files",
        }
    }

    /// How to see the rest, which for the expensive mode includes the cheaper ones.
    fn hint(self) -> &'static str {
        match self {
            Self::Content => {
                "Narrow the pattern, path, or glob, or use outputMode 'files' to see which files \
                 match."
            }
            Self::Files | Self::Count => "Narrow the pattern, path, or glob to see the rest.",
        }
    }
}

/// One parsed search.
///
/// `limit` is deliberately *not* consulted while searching. Stopping early would make the total the
/// result reports a count of what was collected rather than what exists — "5 more matching lines
/// not shown (6 total)" for a pattern with three hundred hits.
#[derive(Debug, Clone)]
struct Search<'a> {
    pattern: &'a str,
    path: &'a Path,
    glob: Option<&'a str>,
    before: usize,
    after: usize,
    all: bool,
    invert: bool,
    mode: OutputMode,
    limit: usize,
}

/// What `search` finds under `cwd`, unguarded, as the tests ask it.
#[cfg(test)]
fn native_search(cwd: &Path, search: &Search<'_>) -> Result<Vec<String>, String> {
    native_search_files(cwd, search, crate::walk::files(cwd, search.path, search.all), || false)
}

/// Scans `paths` (relative to `cwd`) for `search`, in parallel, rows in path order.
///
/// Each worker takes a contiguous run of the sorted paths and the runs are joined in order, so the
/// answer reads the same however many threads produced it. `stopped` is checked between files.
fn native_search_files(
    cwd: &Path,
    search: &Search<'_>,
    paths: Vec<PathBuf>,
    stopped: impl Fn() -> bool + Sync,
) -> Result<Vec<String>, String> {
    let regex = regex::Regex::new(search.pattern)
        .map_err(|error| format!("'{}' is not a valid regex: {error}", search.pattern))?;
    let prefilter = prefilter(search);
    let filter = search
        .glob
        .map(|glob| {
            crate::walk::matcher(glob)
                .map_err(|error| format!("'{glob}' is not a valid glob: {error}"))
        })
        .transpose()?;
    let paths: Vec<PathBuf> = paths
        .into_iter()
        .filter(|path| filter.as_ref().is_none_or(|filter| filter.is_match(path)))
        .collect();
    let workers = std::thread::available_parallelism().map_or(1, usize::from).min(16);
    let run = paths.len().div_ceil(workers).max(1);
    // a worker is `Err` when it panicked, `Ok(None)` when it was stopped
    let scanned: Vec<std::thread::Result<Option<Vec<String>>>> = std::thread::scope(|scope| {
        let handles: Vec<_> = paths
            .chunks(run)
            .map(|chunk| {
                let (regex, prefilter, stopped) = (&regex, prefilter.as_ref(), &stopped);
                scope.spawn(move || {
                    let mut rows = Vec::new();
                    for relative in chunk {
                        if stopped() {
                            return None;
                        }
                        rows.extend(search_file(cwd, relative, (regex, prefilter), search));
                    }
                    Some(rows)
                })
            })
            .collect();
        handles.into_iter().map(|handle| handle.join()).collect()
    });
    combine(scanned, stopped())
}

/// The workers' rows joined in order; a crashed worker is a failure, a stopped one a cancel.
fn combine(
    scanned: Vec<std::thread::Result<Option<Vec<String>>>>,
    stopped: bool,
) -> Result<Vec<String>, String> {
    if scanned.iter().any(Result::is_err) {
        return Err("a search worker failed; nothing it scanned is reported".into());
    }
    if stopped || scanned.iter().any(|worker| matches!(worker, Ok(None))) {
        return Err("grep cancelled".into());
    }
    Ok(scanned.into_iter().flatten().flatten().flatten().collect())
}

/// A whole-text test that can only rule a file out, never in: the pattern with `^`/`$` at every
/// line end, CRLF included, as matching line by line reads them.
///
/// `None` where it could rule out a file that matches: an inverted search; an anchor to the start
/// or end of the text, which a line has and a whole file has only once; and an inline flag that
/// turns a mode off, which can undo the line semantics. A file with a lone `\r` skips it too, in
/// [`search_file`], since CRLF mode keeps `.` off a `\r` that a line still holds.
fn prefilter(search: &Search<'_>) -> Option<regex::Regex> {
    let pattern = search.pattern;
    if search.invert
        || pattern.contains("\\A")
        || pattern.contains("\\z")
        || turns_off_a_flag(pattern)
    {
        return None;
    }
    regex::RegexBuilder::new(pattern).multi_line(true).crlf(true).build().ok()
}

/// Whether `pattern` has an inline group like `(?-m)` or `(?i-s:…)`.
fn turns_off_a_flag(pattern: &str) -> bool {
    pattern.match_indices("(?").any(|(at, _)| {
        let flags = &pattern[at + 2..];
        let end = flags.find([')', ':']).unwrap_or(flags.len());
        flags[..end].contains('-')
    })
}

/// Whether `text` holds a `\r` that does not end a line, which a line keeps inside it.
fn has_lone_carriage_return(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.iter().enumerate().any(|(at, byte)| *byte == b'\r' && bytes.get(at + 1) != Some(&b'\n'))
}

/// The rows one file contributes, in the shape ripgrep prints them: `file:line:text` for a match,
/// `file-line-text` for context, or the file's name or count for the coarser modes.
fn search_file(
    cwd: &Path,
    relative: &Path,
    (regex, prefilter): (&regex::Regex, Option<&regex::Regex>),
    search: &Search<'_>,
) -> Vec<String> {
    let Ok(bytes) = std::fs::read(cwd.join(relative)) else { return Vec::new() };
    if bytes.iter().take(BINARY_SNIFF_BYTES).any(|byte| *byte == 0) {
        return Vec::new();
    }
    let Ok(text) = String::from_utf8(bytes) else { return Vec::new() };
    // most files hold no match, and one pass over the whole text is far cheaper than per line
    let filtered = prefilter.filter(|_| !has_lone_carriage_return(&text));
    if filtered.is_some_and(|prefilter| !prefilter.is_match(&text)) {
        return Vec::new();
    }
    let name = luxe_core::paths::display(relative);
    let lines: Vec<&str> = text.lines().collect();
    let matched: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| regex.is_match(line) != search.invert)
        .map(|(index, _)| index)
        .collect();
    if matched.is_empty() {
        return Vec::new();
    }
    match search.mode {
        OutputMode::Files => vec![name],
        OutputMode::Count => vec![format!("{name}:{}", matched.len())],
        OutputMode::Content => {
            // Decide *which* lines to print and *what each one is* separately, then print each
            // once. Doing it per match instead conflates the two: a match inside an earlier
            // match's trailing context gets emitted as context and never as the answer.
            let mut shown = vec![false; lines.len()];
            for index in &matched {
                let first = index.saturating_sub(search.before);
                let last = (index + search.after + 1).min(lines.len());
                shown[first..last].iter_mut().for_each(|slot| *slot = true);
            }
            let is_match = |index: usize| matched.binary_search(&index).is_ok();
            lines
                .iter()
                .enumerate()
                .filter(|(index, _)| shown[*index])
                .map(|(index, line)| {
                    // `-` marks a context line; the caller's parsers and the model both read the
                    // `:` rows as the answer
                    let separator = if is_match(index) { ':' } else { '-' };
                    format!("{name}{separator}{}{separator}{line}", index + 1)
                })
                .collect()
        }
    }
}

/// The `grep` tool implementation.
pub struct GrepTool {
    /// Shared read tracker; a file whose lines a search printed is recorded as read
    tracker: Option<std::sync::Arc<luxe_core::read_tracker::ReadTracker>>,
}

impl GrepTool {
    /// Create a new grep tool instance.
    pub fn new() -> Self {
        Self { tracker: None }
    }

    /// Attach the shared read tracker, so the lines a search showed satisfy the edit guards the
    /// way a `read` of them would.
    pub fn with_tracker(
        mut self,
        tracker: std::sync::Arc<luxe_core::read_tracker::ReadTracker>,
    ) -> Self {
        self.tracker = Some(tracker);
        self
    }
}

impl Default for GrepTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn schema(&self) -> ToolSchema {
        grep_schema()
    }

    async fn run_with_read_hook(
        &self,
        args: serde_json::Value,
        cx: &ToolContext,
        read_hook: &dyn luxe_core::tool::FileReadHook,
    ) -> ToolResultData {
        guarded::run(self, args, cx, read_hook).await
    }

    async fn run(&self, args: serde_json::Value, cx: &ToolContext) -> ToolResultData {
        guarded::run(self, args, cx, &crate::NoReadPolicy).await
    }
}

#[cfg(test)]
mod search_tests;
#[cfg(test)]
mod tests;
