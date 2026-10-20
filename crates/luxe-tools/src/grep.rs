//! `grep` tool — search file contents by regex.
//!
//! Searches in-process: candidates come from [`crate::walk`], every one passes read policy before
//! it is opened, and the approved files are scanned in parallel with `regex`. An external `rg`
//! could not do the middle step, which is the one that keeps denied content out of the answer.
//!
//! The rows are bounded `file:line:text`, with optional context lines and a glob filter — or,
//! where the question is coarser than the lines, the files that match or a count per file. Lines
//! reach the model grouped under their file: the path once rather than on every line. Asked to
//! (`scopes`), each run of matches is labeled with the function or type around it, where an
//! outline can say, so where a match sits needs no read to find out.

use async_trait::async_trait;
use luxe_core::hooks::file_changed::{OutlineSource, Scope};
use luxe_core::{ContentBlock, Tool, ToolContext, ToolResultData, ToolSchema};
use serde_json::json;
use std::path::{Path, PathBuf};
use tracing::warn;

mod guarded;
mod present;
#[cfg(test)]
mod read_policy_tests;

/// JSON-schema for the grep tool arguments.
fn grep_schema() -> ToolSchema {
    ToolSchema {
        name: "grep".to_string(),
        description: "Search file *contents* by regex. Returns each matching file's path once \
                      with its `line:text` rows under it; `scopes: true` also names the function \
                      or type around each match, as `[QuerySet › delete]`. Bounded \
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
                    "description": "Glob pattern to filter files (e.g., '*.rs'); a leading '!' excludes (e.g., '!tests/**')"
                },
                "outputMode": {
                    "type": "string",
                    "enum": ["content", "files", "count"],
                    "description": "What to return: 'content' (matching lines under each file, the default), \
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
                "scopes": {
                    "type": "boolean",
                    "description": "Name the function or type each match sits in, for code a \
                                    parser reads. Costs a row per change of scope; worth it \
                                    when *where* a match is matters, as for call sites. \
                                    Default false."
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
/// Sized against luxe's own tree, where a broad pattern's 200 rows measure ~21 KB and the worst
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

/// One row a search found, kept whole until the answer is laid out.
#[derive(Debug, Clone)]
struct Hit {
    /// The file, relative to where the search ran
    file: std::sync::Arc<str>,
    kind: HitKind,
    /// 1-based line, for a match or a context row
    line: usize,
    /// The line itself, or the number of matching lines for [`HitKind::Count`]
    text: String,
    /// The scopes around a match, outermost first, `QuerySet › delete`, where an outline can say
    scope: Option<String>,
}

/// What a [`Hit`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HitKind {
    Match,
    Context,
    /// A file with a match, in the files mode
    File,
    /// A file's number of matching lines, in the count mode
    Count,
}

impl Hit {
    /// The row as ripgrep prints it, which the caps measure: `file:line:text`, `file-line-text`
    /// for context, a bare path, or `file:count`.
    fn flat(&self) -> String {
        match self.kind {
            HitKind::Match => format!("{}:{}:{}", self.file, self.line, self.text),
            HitKind::Context => format!("{}-{}-{}", self.file, self.line, self.text),
            HitKind::File => self.file.to_string(),
            HitKind::Count => format!("{}:{}", self.file, self.text),
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
    /// Label each match with the scopes around it, where an outline can read the file
    scopes: bool,
    mode: OutputMode,
    limit: usize,
}

/// What `search` finds under `cwd`, unguarded and as ripgrep would print it, as the tests ask it.
#[cfg(test)]
fn native_search(cwd: &Path, search: &Search<'_>) -> Result<Vec<String>, String> {
    let paths = crate::walk::files(cwd, search.path, search.all);
    let hits = native_search_files(cwd, search, paths, None, || false)?;
    Ok(hits.iter().map(Hit::flat).collect())
}

/// Scans `paths` (relative to `cwd`) for `search`, in parallel, rows in path order, each match
/// labeled with its scopes where `outline` can parse the file.
///
/// Each worker takes a contiguous run of the sorted paths and the runs are joined in order, so the
/// answer reads the same however many threads produced it. `stopped` is checked between files.
fn native_search_files(
    cwd: &Path,
    search: &Search<'_>,
    paths: Vec<PathBuf>,
    outline: Option<&dyn OutlineSource>,
    stopped: impl Fn() -> bool + Sync,
) -> Result<Vec<Hit>, String> {
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
    let scanned: Vec<std::thread::Result<Option<Vec<Hit>>>> = std::thread::scope(|scope| {
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
                        let matcher = (regex, prefilter);
                        rows.extend(search_file(cwd, relative, matcher, search, outline));
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
    scanned: Vec<std::thread::Result<Option<Vec<Hit>>>>,
    stopped: bool,
) -> Result<Vec<Hit>, String> {
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

/// The rows one file contributes: its matching and context lines, or its name or count for the
/// coarser modes. A match is labeled with the scopes around it where `outline` parses the file.
fn search_file(
    cwd: &Path,
    relative: &Path,
    (regex, prefilter): (&regex::Regex, Option<&regex::Regex>),
    search: &Search<'_>,
    outline: Option<&dyn OutlineSource>,
) -> Vec<Hit> {
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
    let file: std::sync::Arc<str> = luxe_core::paths::display(relative).into();
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
    let row = |kind, line, text: String| Hit { file: file.clone(), kind, line, text, scope: None };
    match search.mode {
        OutputMode::Files => vec![row(HitKind::File, 0, String::new())],
        OutputMode::Count => vec![row(HitKind::Count, 0, matched.len().to_string())],
        OutputMode::Content => {
            let outline = outline.filter(|_| search.scopes);
            let scopes = outline.and_then(|outline| outline.scopes(relative, &text));
            line_hits(&lines, &matched, search)
                .map(|(index, is_match)| {
                    let kind = if is_match { HitKind::Match } else { HitKind::Context };
                    let mut hit = row(kind, index + 1, lines[index].to_string());
                    hit.scope =
                        scopes.as_deref().filter(|_| is_match).and_then(|s| scope_label(s, index));
                    hit
                })
                .collect()
        }
    }
}

/// The lines to show and whether each is a match, in order.
///
/// Decides *which* lines to print and *what each one is* separately, then yields each once. Doing
/// it per match instead conflates the two: a match inside an earlier match's trailing context gets
/// emitted as context and never as the answer.
fn line_hits<'a>(
    lines: &'a [&str],
    matched: &'a [usize],
    search: &Search<'_>,
) -> impl Iterator<Item = (usize, bool)> + 'a {
    let mut shown = vec![false; lines.len()];
    for index in matched {
        let first = index.saturating_sub(search.before);
        let last = (index + search.after + 1).min(lines.len());
        shown[first..last].iter_mut().for_each(|slot| *slot = true);
    }
    (0..lines.len())
        .filter(move |index| shown[*index])
        .map(move |index| (index, matched.binary_search(&index).is_ok()))
}

/// The scopes around 0-based line `line`, outermost first, as `QuerySet › delete`.
///
/// A scope whose declaration starts on the line is left out: the line itself names it, so
/// `def delete(self):` is labeled `QuerySet`, not `QuerySet › delete`.
fn scope_label(scopes: &[Scope], line: usize) -> Option<String> {
    let mut around: Vec<&Scope> =
        scopes.iter().filter(|scope| scope.start_line < line && line <= scope.end_line).collect();
    around.sort_by_key(|scope| (scope.start_line, std::cmp::Reverse(scope.end_line)));
    let names: Vec<&str> = around.iter().map(|scope| scope.name.as_str()).collect();
    (!names.is_empty()).then(|| names.join(" › "))
}

/// The `grep` tool implementation.
pub struct GrepTool {
    /// Shared read tracker; a file whose lines a search printed is recorded as read
    tracker: Option<std::sync::Arc<luxe_core::read_tracker::ReadTracker>>,
    /// What names the scopes a match sits in; without it lines come unlabeled
    outline: Option<std::sync::Arc<dyn OutlineSource>>,
}

impl GrepTool {
    /// Create a new grep tool instance.
    pub fn new() -> Self {
        Self { tracker: None, outline: None }
    }

    /// Attach the outline that labels each match with the function or type around it, a parser
    /// that lives outside this crate.
    pub fn with_outline(mut self, outline: std::sync::Arc<dyn OutlineSource>) -> Self {
        self.outline = Some(outline);
        self
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
mod scope_tests;
#[cfg(test)]
mod search_tests;
#[cfg(test)]
mod tests;
