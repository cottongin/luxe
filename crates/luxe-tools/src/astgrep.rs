//! ast-grep — structural search and replace through a verified CLI.
//!
//! Engine-run calls classify concrete file reads before handing paths to the matcher. Its JSON
//! output carries real files, positions and byte ranges. Binary discovery prefers `ast-grep` and
//! verifies its version banner because `/usr/bin/sg` can be the unrelated shadow-utils group tool.

use async_trait::async_trait;
use luxe_core::read_tracker::ReadTracker;
use luxe_core::{Tool, ToolContext, ToolResultData};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::process::Command as TokioCommand;
use tokio::sync::OnceCell;

mod guarded;
mod render;

#[cfg(test)]
use render::render_matches;
use render::search_outcome;

/// Candidate binary names, best first (`sg` is upstream's deprecated alias).
const BINARIES: [&str; 2] = ["ast-grep", "sg"];

/// What to do when no ast-grep is found.
///
/// The fallback comes first, deliberately. Leading with the install command invites the model to
/// install a build dependency mid-task and then fight its CLI — observed live: one missing binary
/// turned into eighteen `bash` calls. `grep` answers most structural searches well enough, and
/// installing is the *user's* call.
const INSTALL_HINT: &str = "use `grep` for this search instead. (To enable structural search, the \
                            user can install it with `cargo install ast-grep --locked` or \
                            `npm i -g @ast-grep/cli`; on Linux, `/usr/bin/sg` is the unrelated \
                            shadow-utils group tool, so the `ast-grep` binary must be on PATH. Do \
                            not install it yourself.)";

/// A compact pattern primer, appended when ast-grep rejects the pattern itself.
///
/// Upstream's message is correct but ends in a documentation URL, which a model cannot open — so a
/// rejected pattern is a dead end. Observed: two invalid patterns in a row, then the model
/// abandoned the tool for eight `bash` calls. The rules that actually matter fit in five lines.
const PATTERN_HELP: &str =
    "Pattern rules: a pattern must be ONE syntactically valid snippet of the \
     target language — not a multi-line template, and not a fragment that cannot stand alone. \
     `$NAME` matches one node, `$$$ARGS` matches a variadic list. Examples: python \
     `def $NAME($$$ARGS): $$$BODY` · python decorated call `@$DEC` · rust \
     `fn $NAME($$$ARGS) { $$$BODY }` · typescript `$OBJ.$METHOD($$$ARGS)`. If the shape cannot be \
     expressed as one snippet (for example \"a function that has any decorator above it\"), that \
     is not an ast-grep query — use `grep` instead.";

/// The primer, when the failure is about the pattern rather than the environment.
fn pattern_help(stderr: &str) -> Option<&'static str> {
    let lowered = stderr.to_ascii_lowercase();
    (lowered.contains("pattern") || lowered.contains("cannot parse")).then_some(PATTERN_HELP)
}

/// Most rendered lines returned for one search — the same bound `grep` and `glob` apply.
const MAX_MATCH_LINES: usize = 200;

/// Most source lines shown per individual match.
const MAX_MATCH_BODY_LINES: usize = 6;

/// Cached resolution of the ast-grep binary. Only a *successful* probe is cached, so installing
/// ast-grep mid-session is picked up on the next call.
static BINARY: OnceCell<String> = OnceCell::const_new();

/// One ast-grep match.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AstGrepMatch {
    /// File the match was found in.
    pub file: PathBuf,
    /// Byte range `(start, end)` of the matched node.
    pub range: (usize, usize),
    /// 1-based line of the match's start.
    pub line: usize,
    /// 0-based column of the match's start.
    pub column: usize,
    /// The matched source text.
    pub text: String,
    /// Rewritten text, when the run was a replace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replacement: Option<String>,
}

/// ast-grep search parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AstGrepSearchParams {
    /// SKELETON pattern (e.g., `fn $NAME() { $$$BODY }`).
    pub pattern: String,
    /// Language (e.g., "rust", "typescript", "python").
    pub lang: String,
    /// Paths to search (defaults to the current directory).
    #[serde(default, alias = "path", deserialize_with = "one_or_many")]
    pub paths: Vec<String>,
}

/// `paths` as an array, or as the single `path` string `grep`, `glob` and `read` take.
///
/// Without it a model writing `path`, as every sibling tool spells it, had its scope dropped in
/// silence and searched the whole project: a confident answer to a question it did not ask.
fn one_or_many<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(String),
        Many(Vec<String>),
    }
    Ok(match OneOrMany::deserialize(deserializer)? {
        OneOrMany::One(path) => vec![path],
        OneOrMany::Many(paths) => paths,
    })
}

/// ast-grep replace parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AstGrepReplaceParams {
    /// SKELETON pattern.
    pub pattern: String,
    /// Rewrite pattern (e.g., `fn $NAME() { /* TODO */ }`).
    pub rewrite: String,
    /// Language.
    pub lang: String,
    /// Paths to rewrite.
    #[serde(default, alias = "path", deserialize_with = "one_or_many")]
    pub paths: Vec<String>,
    /// Apply the changes; otherwise preview them (default).
    #[serde(default)]
    pub apply: bool,
}

/// ast-grep tool: structural search and replace via the `ast-grep` CLI.
#[derive(Default)]
pub struct AstGrepTool {
    tracker: Option<Arc<ReadTracker>>,
}

/// Error type for ast-grep operations.
#[derive(Debug)]
pub enum AstGrepError {
    /// No usable ast-grep binary on PATH.
    CliNotFound,
    /// The CLI ran but failed.
    CommandFailed(String),
    /// The CLI's JSON output could not be parsed.
    ParseError(String),
}

impl std::fmt::Display for AstGrepError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CliNotFound => write!(f, "ast-grep CLI not found — {INSTALL_HINT}"),
            Self::CommandFailed(msg) => match pattern_help(msg) {
                Some(help) => write!(f, "command failed: {msg}\n\n{help}"),
                None => write!(f, "command failed: {msg}"),
            },
            Self::ParseError(msg) => write!(f, "parse error: {msg}"),
        }
    }
}

impl std::error::Error for AstGrepError {}

/// Every path worth probing: each known name resolved through `PATH` and then the well-known
/// install directories, so a `cargo install`-ed binary is found even when its bin directory never
/// made it onto `PATH`.
fn candidate_paths() -> Vec<String> {
    BINARIES
        .iter()
        .filter_map(|name| luxe_core::program::resolve(name))
        .map(|path| path.to_string_lossy().into_owned())
        .collect()
}

/// Resolve a usable ast-grep binary, or `None` when none is installed.
///
/// A candidate must exit successfully *and* identify itself as ast-grep — spawning is not enough,
/// which is how the shadow-utils `sg` gets mistaken for it.
pub async fn resolve_binary() -> Option<String> {
    if let Some(found) = BINARY.get() {
        return Some(found.clone());
    }
    for candidate in candidate_paths() {
        let Ok(output) = TokioCommand::new(&candidate)
            .arg("--version")
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output()
            .await
        else {
            continue;
        };
        if identifies_itself(&output) {
            let _ = BINARY.set(candidate.clone());
            return Some(candidate);
        }
    }
    None
}

/// Whether what `--version` printed is ast-grep.
///
/// Both the real binary and its `sg` alias print `ast-grep <version>`; the shadow-utils `sg`
/// prints a usage line to stderr and exits non-zero, so it never passes.
fn identifies_itself(output: &std::process::Output) -> bool {
    output.status.success()
        && String::from_utf8_lossy(&output.stdout).to_ascii_lowercase().contains("ast-grep")
}

/// Whether a real ast-grep is installed, checked synchronously so registration can skip the tool.
///
/// Without the binary every call answers "ast-grep CLI not found", which is not worth a schema in
/// every request. This runs once at startup and seeds the same cache [`resolve_binary`] reads, so
/// the verification is not repeated. It applies the identity check for the same reason that one
/// does: the shadow-utils `sg` on `PATH` is not this tool.
pub fn is_available() -> bool {
    if BINARY.get().is_some() {
        return true;
    }
    for candidate in candidate_paths() {
        let Ok(output) = std::process::Command::new(&candidate)
            .arg("--version")
            .stdin(std::process::Stdio::null())
            .output()
        else {
            continue;
        };
        if identifies_itself(&output) {
            let _ = BINARY.set(candidate);
            return true;
        }
    }
    false
}

impl AstGrepTool {
    pub fn new() -> Self {
        Self::default()
    }

    /// Attach the shared read tracker, so a file an applied rewrite changed stays editable.
    pub fn with_tracker(mut self, tracker: Arc<ReadTracker>) -> Self {
        self.tracker = Some(tracker);
        self
    }

    /// Run an ast-grep `run` invocation and parse its streaming JSON output.
    ///
    /// ast-grep exits non-zero when nothing matched, which is not an error here — it's an empty
    /// result. A genuine failure (bad pattern, unknown language) also writes to stderr, which is
    /// what we surface.
    async fn run_json(args: &[String], cwd: &Path) -> Result<Vec<AstGrepMatch>, AstGrepError> {
        let binary = resolve_binary().await.ok_or(AstGrepError::CliNotFound)?;
        let output = TokioCommand::new(&binary)
            // relative paths and the default scope belong to the session, not the process
            .current_dir(cwd)
            // ast-grep searches the paths it is given, never stdin — but the choice is made here
            // rather than inherited, because a search tool handed a readable stdin can search that.
            .stdin(std::process::Stdio::null())
            // A cancelled turn drops this future; without this the scan outlives it.
            .kill_on_drop(true)
            .args(args)
            .output()
            .await
            .map_err(|e| AstGrepError::CommandFailed(format!("could not run {binary}: {e}")))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !output.status.success() && stdout.trim().is_empty() && !stderr.trim().is_empty() {
            return Err(AstGrepError::CommandFailed(stderr.trim().to_string()));
        }

        let mut matches = Vec::new();
        for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
            let value: serde_json::Value = serde_json::from_str(line)
                .map_err(|e| AstGrepError::ParseError(format!("{e}: {line}")))?;
            matches.push(parse_match(&value)?);
        }
        Ok(matches)
    }

    /// Structural search, run in `cwd`.
    pub async fn search(
        params: &AstGrepSearchParams,
        cwd: &Path,
    ) -> Result<Vec<AstGrepMatch>, AstGrepError> {
        let mut args = base_args(&params.pattern, &params.lang);
        args.push("--json=stream".to_string());
        // a file named like a flag stays a file
        args.push("--".to_string());
        args.extend(params.paths.iter().cloned());
        Self::run_json(&args, cwd).await
    }

    /// Structural replace, run in `cwd`: previews the rewrites, or applies them when `apply` is
    /// set, to exactly the files the preview matched.
    pub async fn replace(
        params: &AstGrepReplaceParams,
        cwd: &Path,
    ) -> Result<Vec<AstGrepMatch>, AstGrepError> {
        // Collect the matches (with their replacements) first, so preview and apply report the
        // same thing and an apply can say exactly what it changed.
        let mut args = base_args(&params.pattern, &params.lang);
        args.extend(["--rewrite".to_string(), params.rewrite.clone()]);
        args.push("--json=stream".to_string());
        // a file named like a flag stays a file
        args.push("--".to_string());
        args.extend(params.paths.iter().cloned());
        let matches = Self::run_json(&args, cwd).await?;

        if params.apply && !matches.is_empty() {
            let binary = resolve_binary().await.ok_or(AstGrepError::CliNotFound)?;
            let mut apply_args = base_args(&params.pattern, &params.lang);
            apply_args.extend(["--rewrite".to_string(), params.rewrite.clone()]);
            apply_args.push("--update-all".to_string());
            // the previewed files, not a fresh scan of the scope that could find something new
            let mut previewed: Vec<String> =
                matches.iter().map(|item| luxe_core::paths::display(&item.file)).collect();
            previewed.sort();
            previewed.dedup();
            apply_args.push("--".to_string());
            apply_args.extend(previewed);
            let output = TokioCommand::new(&binary)
                .current_dir(cwd)
                .args(&apply_args)
                .stdin(std::process::Stdio::null())
                // A cancelled turn drops this future; without this the rewrite outlives it.
                .kill_on_drop(true)
                .output()
                .await
                .map_err(|e| AstGrepError::CommandFailed(format!("could not run {binary}: {e}")))?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(AstGrepError::CommandFailed(stderr.trim().to_string()));
            }
        }
        Ok(matches)
    }
}

/// The shared leading arguments for a `run` invocation.
fn base_args(pattern: &str, lang: &str) -> Vec<String> {
    vec![
        "run".to_string(),
        "--pattern".to_string(),
        pattern.to_string(),
        "--lang".to_string(),
        lang.to_string(),
        "--color".to_string(),
        "never".to_string(),
    ]
}

/// Convert one ast-grep JSON match object into an [`AstGrepMatch`].
fn parse_match(value: &serde_json::Value) -> Result<AstGrepMatch, AstGrepError> {
    let missing = |field: &str| AstGrepError::ParseError(format!("match is missing `{field}`"));
    let file = value.get("file").and_then(|v| v.as_str()).ok_or_else(|| missing("file"))?;
    let text = value.get("text").and_then(|v| v.as_str()).unwrap_or_default();
    let range = value.get("range").ok_or_else(|| missing("range"))?;
    let byte = range.get("byteOffset");
    let start = byte.and_then(|b| b.get("start")).and_then(serde_json::Value::as_u64).unwrap_or(0);
    let end = byte.and_then(|b| b.get("end")).and_then(serde_json::Value::as_u64).unwrap_or(0);
    let position = range.get("start");
    let line =
        position.and_then(|p| p.get("line")).and_then(serde_json::Value::as_u64).unwrap_or(0);
    let column =
        position.and_then(|p| p.get("column")).and_then(serde_json::Value::as_u64).unwrap_or(0);
    Ok(AstGrepMatch {
        file: PathBuf::from(file),
        range: (start as usize, end as usize),
        // ast-grep reports 0-based lines; every other Luxe surface is 1-based.
        line: line as usize + 1,
        column: column as usize,
        text: text.to_string(),
        replacement: value
            .get("replacement")
            .and_then(|v| v.as_str())
            .map(std::string::ToString::to_string),
    })
}

#[async_trait]
impl Tool for AstGrepTool {
    fn name(&self) -> &str {
        "ast_grep"
    }

    fn schema(&self) -> luxe_core::ToolSchema {
        luxe_core::ToolSchema {
            name: "ast_grep".to_string(),
            description: "AST-aware structural search and replace via ast-grep. Match code by its \
                          syntax tree, not text — use it to find or rewrite code patterns (e.g. all \
                          calls to a function, a specific match arm shape) reliably across \
                          formatting. Patterns use metavariables: `$VAR` matches one node, `$$$ARGS` \
                          matches a variadic list. Prefer this over grep for code-structure edits; \
                          use grep for plain text. action=replace previews the rewrite unless \
                          apply=true."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["search", "replace"],
                        "description": "search = find matches; replace = rewrite matches (needs `rewrite`)"
                    },
                    "pattern": {
                        "type": "string",
                        "description": "ast-grep pattern with metavariables, e.g. `foo($$$ARGS)` or `if let Some($X) = $Y { $$$BODY }`"
                    },
                    "rewrite": {
                        "type": "string",
                        "description": "replace only: replacement pattern reusing the same metavariables, e.g. `bar($$$ARGS)`"
                    },
                    "lang": {
                        "type": "string",
                        "description": "language id: rust | python | typescript | tsx | javascript | go | c | cpp | java | …"
                    },
                    "paths": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "files or directories to scan (relative to cwd; default: the whole project). A single `path` string works too"
                    },
                    "apply": {
                        "type": "boolean",
                        "description": "replace only: write changes to disk (default false = preview the rewrite)"
                    }
                },
                "required": ["action", "pattern", "lang"]
            }),
        }
    }

    async fn run_with_read_hook(
        &self,
        params: serde_json::Value,
        cx: &ToolContext,
        read_hook: &dyn luxe_core::tool::FileReadHook,
    ) -> luxe_core::ToolResultData {
        guarded::run(self, params, cx, read_hook).await
    }

    async fn run(&self, params: serde_json::Value, cx: &ToolContext) -> luxe_core::ToolResultData {
        let action = params.get("action").and_then(|v| v.as_str()).unwrap_or("search");
        match action {
            "search" => match serde_json::from_value::<AstGrepSearchParams>(params) {
                Ok(search) => search_outcome(Self::search(&search, &cx.cwd).await),
                Err(e) => ToolResultData::error(format!("Invalid params: {e}")),
            },
            "replace" => match serde_json::from_value::<AstGrepReplaceParams>(params) {
                Ok(replace) => {
                    let found = Self::replace(&replace, &cx.cwd).await;
                    render::finish_replace(self, found, replace.apply, cx).await
                }
                Err(e) => ToolResultData::error(format!("Invalid params: {e}")),
            },
            other => ToolResultData::error(format!("Unknown action: {other}")),
        }
    }
}

#[cfg(test)]
mod tests;
