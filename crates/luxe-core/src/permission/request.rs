//! Per-tool permission request extraction.
//!
//! Turns a `(tool, args)` call into the concrete [`Check`]s the [`PermissionEngine`] evaluates:
//! the tool's own key (`bash`/`edit`/`read`/…) against its primary input, plus the
//! `external_directory` guard when a path escapes the project root and a fork-bomb guard for the
//! one shell-syntax disaster plain glob matching can't catch. It also builds the "always"
//! approval rules a UI offers ("allow this command family for the session").

use std::path::{Component, Path, PathBuf};

use super::{rule, Rule, EXTERNAL_DIRECTORY};

mod execution;
mod search;

/// Guard key: fires on a shell fork-bomb signature (denied by default).
pub(super) const FORKBOMB: &str = "forkbomb";

/// One thing to evaluate: a permission key and the concrete input to match rules against.
#[derive(Debug, Clone)]
pub struct Check {
    pub permission: String,
    pub input: String,
}

/// A fully-extracted permission request for one tool call.
#[derive(Debug, Clone)]
pub struct PermissionRequest {
    /// Primary permission key (the tool's key), for display and grouping.
    pub permission: String,
    /// Raw tool name.
    pub tool: String,
    /// Every check that must pass; the engine takes the strictest decision across them.
    pub checks: Vec<Check>,
    /// Rules an "always approve" reply would add for the rest of the session.
    pub always: Vec<Rule>,
    /// Short human-readable summary for the approval prompt.
    pub summary: String,
}

/// Map a tool name to its permission key (`write`/`edit`/`patch` → `edit`, `read` → `read`,
/// everything else keeps its own name so users can key rules by tool).
pub fn permission_key(tool: &str) -> &str {
    match tool {
        "write" | "edit" | "patch" | "apply_patch" => "edit",
        other => other,
    }
}

/// Extract the primary input string a rule matches against for `tool`.
pub fn primary_input(tool: &str, args: &serde_json::Value) -> String {
    let get = |k: &str| args.get(k).and_then(|v| v.as_str()).map(str::to_string);
    match tool {
        "bash" => get("command").unwrap_or_default(),
        "read" | "write" | "edit" | "patch" | "apply_patch" => {
            get("path").or_else(|| get("file_path")).unwrap_or_default()
        }
        "glob" => get("pattern").or_else(|| get("glob")).unwrap_or_default(),
        "grep" => get("pattern").or_else(|| get("query")).unwrap_or_default(),
        "web_fetch" | "webfetch" => get("url").unwrap_or_default(),
        "web_search" | "websearch" => get("query").unwrap_or_default(),
        // `mode=url` renders a page, and the URL is what a rule should be about (`"screenshot":
        // { "http://*": "ask" }`). Every other mode photographs the *desktop*, where the mode is
        // the only thing worth naming — and has to be nameable, or no rule can say "do not
        // photograph my screen" without gating the browser along with it. Keying on the URL alone
        // left those modes with an *empty* target, which the permissive default allows outright.
        //
        // The mode comes from [`screenshot_mode`], the tool's own reading, so a call cannot be
        // checked as a page and run as a photograph of the desktop.
        "screenshot" => match screenshot_mode(args) {
            "url" => get("url").unwrap_or_default(),
            mode => mode.to_string(),
        },
        // The `mcp` proxy: only a *call* (a `tool` target with `args`) executes; discovery
        // (search/describe) has no target, so its input is empty (allowed by default). The target
        // is `server/name`, so rules like `"mcp": { "github/*": "ask" }` gate per server/tool.
        "mcp" => match (get("tool"), args.get("args").and_then(|v| v.as_str())) {
            (Some(target), Some(_)) => target,
            _ => String::new(),
        },
        "git" => {
            let action = get("action").unwrap_or_default();
            let extra = args
                .get("args")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(" "))
                .unwrap_or_default();
            format!("{action} {extra}").trim().to_string()
        }
        _ => args
            .as_object()
            .and_then(|o| o.values().find_map(|v| v.as_str()))
            .unwrap_or_default()
            .to_string(),
    }
}

/// The paths of a `read` that names several files with `paths`, in order; empty for anything else.
fn extra_read_paths(tool: &str, args: &serde_json::Value) -> Vec<String> {
    if tool != "read" {
        return Vec::new();
    }
    args.get("paths")
        .and_then(serde_json::Value::as_array)
        .map(|paths| paths.iter().filter_map(|path| path.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

/// Whether a tool takes a filesystem path as its primary input (so the external-dir guard applies).
fn is_path_tool(tool: &str) -> bool {
    matches!(tool, "read" | "write" | "edit" | "patch" | "apply_patch")
}

/// Lexically normalize a path (resolve `.`/`..` without touching the filesystem, so it works for
/// not-yet-created write targets). Relative paths are joined onto `root`. Translated by
/// [`crate::paths::native`] first, as the file tools translate it, so the guard judges the file the
/// tool will open.
fn normalize(input: &str, root: &Path) -> PathBuf {
    let input = &*crate::paths::native(input);
    let joined =
        if Path::new(input).is_absolute() { PathBuf::from(input) } else { root.join(input) };
    let mut out = PathBuf::new();
    for comp in joined.components() {
        match comp {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Whether `input` resolves outside `root`.
///
/// Containment is [`crate::paths::contains`], not `starts_with`: Windows filesystems are
/// case-insensitive, and `fs::canonicalize` hands back a verbatim `\\?\C:\...` root that would
/// otherwise share no prefix with the ordinary path a tool call carries — either way the guard would
/// fire on the user's own project, on every call.
fn is_external(input: &str, root: &Path) -> bool {
    let root = normalize(&root.to_string_lossy(), Path::new("/"));
    !crate::paths::contains(&root, &normalize(input, &root))
}

/// Whether a tool call *changes* the path it names, rather than only reading it.
pub fn is_mutating_path_tool(tool: &str) -> bool {
    matches!(tool, "write" | "edit" | "patch" | "apply_patch")
}

/// The absolute path a call targets when it escapes `root`, or `None` when it stays inside.
///
/// The same normalization the `external_directory` guard fires on, exposed so nothing else has to
/// re-derive "is this outside the project". A second implementation would drift, and the two
/// answers disagreeing is how a path gets guarded but not recorded — or recorded but not guarded.
pub fn external_target(tool: &str, args: &serde_json::Value, root: &Path) -> Option<PathBuf> {
    let input = primary_input(tool, args);
    (is_path_tool(tool) && !input.is_empty() && is_external(&input, root))
        .then(|| normalize(&input, root))
}

/// Detect the classic shell fork bomb (`:(){ :|:& };:` and close variants), which token/glob
/// matching can't reliably catch. Compares with whitespace removed.
fn is_fork_bomb(command: &str) -> bool {
    let compact: String = command.chars().filter(|c| !c.is_whitespace()).collect();
    compact.contains(":(){") || compact.contains("(){:|:&}") || compact.contains("(){:|:&};:")
}

fn bash_segment_checks(tool: &str, input: &str) -> Vec<Check> {
    if tool != "bash" {
        return Vec::new();
    }
    let segments = super::wildcard::command_segments(input);
    if segments.len() < 2 {
        return Vec::new();
    }
    segments
        .into_iter()
        .map(|segment| Check { permission: "bash".into(), input: segment.join(" ") })
        .collect()
}

fn bash_always_rules(input: &str) -> Vec<Rule> {
    let segments = super::wildcard::command_segments(input);
    let [segment] = segments.as_slice() else { return Vec::new() };
    let Some(head) = segment.first() else { return Vec::new() };
    vec![rule("bash", &format!("{head} *"), super::PermissionAction::Allow)]
}

fn applied_ast_grep_paths(tool: &str, args: &serde_json::Value) -> Vec<String> {
    if tool != "ast_grep"
        || args.get("action").and_then(serde_json::Value::as_str) != Some("replace")
        || args.get("apply").and_then(serde_json::Value::as_bool) != Some(true)
    {
        return Vec::new();
    }
    search::ast_grep_roots(args)
}

fn applied_lsp_paths(tool: &str, args: &serde_json::Value) -> Vec<String> {
    if tool != "lsp_navigation"
        || !matches!(
            args.get("action").and_then(serde_json::Value::as_str),
            Some("rename" | "code_action")
        )
        || args.get("apply").and_then(serde_json::Value::as_bool) != Some(true)
    {
        return Vec::new();
    }
    search::lsp_file_roots(args)
}

/// Build the permission request for a tool call, relative to the project `root`.
pub fn build_request(tool: &str, args: &serde_json::Value, root: &Path) -> PermissionRequest {
    if let Some(request) = execution::request(tool, args, root) {
        return request;
    }
    let key = permission_key(tool).to_string();
    // A `read` of several files is judged file by file: every path is a check of its own, so a
    // `.env` anywhere in the list denies the call by the same strictest-wins rule that denies it
    // alone. Judged by its first path only, the rest of the list would never have been looked at.
    let others = extra_read_paths(tool, args);
    let input = match primary_input(tool, args) {
        input if input.is_empty() => others.first().cloned().unwrap_or_default(),
        input => input,
    };
    let mut checks = vec![Check { permission: key.clone(), input: input.clone() }];
    checks.extend(bash_segment_checks(tool, &input));
    let mut edit_paths = applied_ast_grep_paths(tool, args);
    edit_paths.extend(applied_lsp_paths(tool, args));
    checks.extend(
        edit_paths.iter().map(|path| Check { permission: "edit".into(), input: path.clone() }),
    );
    let mut always = Vec::new();
    let search_paths = search::search_roots(tool, args);
    checks.extend(search::root_checks(&search_paths, root));
    checks.extend(search::grep_selector_checks(tool, args, &search_paths));

    // External-directory guard for path tools.
    let mut external_dir = None;
    if is_path_tool(tool) && !input.is_empty() && is_external(&input, root) {
        let absolute = normalize(&input, root);
        checks.push(Check {
            permission: EXTERNAL_DIRECTORY.to_string(),
            input: crate::paths::display(&absolute),
        });
        external_dir = absolute.parent().map(Path::to_path_buf);
    }
    for path in others.iter().filter(|path| **path != input) {
        checks.push(Check { permission: key.clone(), input: path.clone() });
        if is_external(path, root) {
            checks.push(Check {
                permission: EXTERNAL_DIRECTORY.to_string(),
                input: crate::paths::display(&normalize(path, root)),
            });
        }
    }

    // Fork-bomb guard for bash.
    if tool == "bash" && is_fork_bomb(&input) {
        checks.push(Check { permission: FORKBOMB.to_string(), input: input.clone() });
    }
    if tool == "bash" && super::defaults::is_catastrophic_rm(&input) {
        checks.push(Check {
            permission: super::defaults::CATASTROPHIC_RM.into(),
            input: input.clone(),
        });
    }

    // "Always approve" suggestions. These have to cover the check that actually *fired*, at a
    // scope worth approving — approving one exact path in someone else's directory just means the
    // next file there asks again.
    match (tool, &external_dir) {
        ("bash", _) => always.extend(bash_always_rules(&input)),
        // The git tool's input is `"<action> <flags>"`, so approving one call has to cover the
        // action rather than the exact flags it happened to carry — bash's family whitelist, for
        // the same reason. Scoped to the exact input, "always" meant *once more*: approving
        // `commit` left the `commit --amend` after it prompting all over again.
        ("git", _) => {
            if let Some(action) = input.split_whitespace().next() {
                always.push(rule("git", &format!("{action} *"), super::PermissionAction::Allow));
            }
        }
        // Outside the project: approve the containing directory (and its subtree) for both the
        // guard and the tool key, mirroring bash's family whitelist.
        //
        // The patterns are written with forward slashes, because that is the alphabet the glob
        // matcher and every check input speak. Rendered natively they came out mixed on Windows —
        // `C:\Users\dev\notes/*` against an input of `C:\Users\dev\notes\one.md` — so "always allow
        // this directory" matched nothing and the next file in it asked again.
        (_, Some(directory)) => {
            let absolute_glob = format!("{}/*", crate::paths::display(directory));
            always.push(rule(EXTERNAL_DIRECTORY, &absolute_glob, super::PermissionAction::Allow));
            // The tool key matches the input *as written*, which may be relative, so scope the
            // rule the same way the call did.
            let written_glob = Path::new(&input)
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .map(|parent| format!("{}/*", crate::paths::display(parent)))
                .unwrap_or_else(|| absolute_glob.clone());
            always.push(rule(&key, &written_glob, super::PermissionAction::Allow));
            if written_glob != absolute_glob {
                always.push(rule(&key, &absolute_glob, super::PermissionAction::Allow));
            }
        }
        (_, None) if !input.is_empty() => {
            always.push(rule(&key, &input, super::PermissionAction::Allow));
        }
        _ => {}
    }

    // One rule cannot honestly cover several unrelated paths, so a read of many offers no "always"
    if others.iter().any(|path| *path != input)
        || !edit_paths.is_empty()
        || !search_paths.is_empty()
    {
        always.clear();
    }
    if let ([scope], true) = (search_paths.as_slice(), edit_paths.is_empty()) {
        always.extend(search::external_scope_rules(scope, root));
    }
    let summary = match others.len() {
        count if count > 1 => format!("Read {input} and {} more", count - 1),
        _ => summarize(tool, &input, args),
    };
    PermissionRequest { permission: key, tool: tool.to_string(), checks, always, summary }
}

/// Longest commit subject carried into an approval prompt before it is elided.
const SUMMARY_SUBJECT_CHARS: usize = 60;

/// Longest argument preview carried into an approval prompt before it is elided.
///
/// Wider than a commit subject because it is structured rather than prose — `{"task":"…"` spends
/// its first cells on punctuation — and still short enough to sit on one line of the card.
const SUMMARY_ARGS_CHARS: usize = 96;

/// A short one-line description for the approval prompt.
fn summarize(tool: &str, input: &str, args: &serde_json::Value) -> String {
    match tool {
        "bash" => format!("Run: {input}"),
        "read" => format!("Read {input}"),
        "write" | "edit" | "patch" | "apply_patch" => format!("Modify {input}"),
        // A commit prompt is a decision about *this* change, and "git commit" describes every
        // commit ever made. The subject is the one line that tells them apart, so it comes along
        // — a prompt that says nothing is a prompt people learn to dismiss.
        "git" => match subject(args).filter(|_| input.starts_with("commit")) {
            Some(subject) => format!("git {input} \u{2014} {subject}"),
            None => format!("git {input}"),
        },
        // The arguments, not just the tool: approving `delegate/delegate` without them approves a
        // task nobody has read, and the "always" rule this card offers is scoped to the tool, so
        // the arguments are the only part of the decision that is about *this* call.
        "mcp" if !input.is_empty() => match call_preview(args) {
            Some(preview) => format!("MCP call: {input} {preview}"),
            None => format!("MCP call: {input}"),
        },
        "web_fetch" | "webfetch" => format!("Fetch {input}"),
        // Named for what it photographs, not for the argument that says so: "Capture fullscreen"
        // reads as a setting, and this is the one prompt where the reader has to understand that
        // their screen is about to be sent to a model.
        "screenshot" if input == "fullscreen" => "Photograph the whole screen".to_string(),
        "screenshot" if input == "region" => "Photograph a region of the screen".to_string(),
        "screenshot" if input == "window" => "Photograph a window".to_string(),
        "screenshot" if !input.is_empty() => format!("Capture {input}"),
        _ if input.is_empty() => format!("Use {tool}"),
        _ => format!("{tool}: {input}"),
    }
}

/// The commit subject as one bounded line, however the message was spelled.
fn subject(args: &serde_json::Value) -> Option<String> {
    let message = ["message", "commit_message", "msg"]
        .iter()
        .find_map(|key| args.get(*key).and_then(serde_json::Value::as_str))?;
    let first = message.trim().lines().next()?.trim();
    if first.is_empty() {
        return None;
    }
    Some(elide(first, SUMMARY_SUBJECT_CHARS))
}

/// An `mcp` call's arguments as one bounded, redacted line, or `None` when it carries none.
///
/// The proxy takes them as a JSON *string*, so they arrive however the model formatted them —
/// newlines included, which a one-line card cannot draw. Unparseable input is kept rather than
/// dropped: what the reader is approving is whatever the server is about to be sent, and a preview
/// that quietly shows nothing is the defect this exists to fix.
fn call_preview(args: &serde_json::Value) -> Option<String> {
    let raw = args.get("args").and_then(serde_json::Value::as_str)?.trim();
    if raw.is_empty() {
        return None;
    }
    let compact = match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(value) => value.to_string(),
        Err(_) => raw.split_whitespace().collect::<Vec<_>>().join(" "),
    };
    if compact == "{}" {
        // A call that takes no arguments has nothing to preview, and `{}` on the card is furniture.
        return None;
    }
    Some(elide(&crate::redact::redact_secrets(&compact, &[]), SUMMARY_ARGS_CHARS))
}

/// `text` cut to `max` characters, with an ellipsis when there was more.
fn elide(text: &str, max: usize) -> String {
    let mut line: String = text.chars().take(max).collect();
    if line.chars().count() < text.chars().count() {
        line.push('\u{2026}');
    }
    line
}

/// What a `screenshot` call captures: its `mode`, else a page when it names a `url`, else the
/// whole screen.
///
/// The tool and the permission check both read the call through this, so they cannot disagree:
/// keying on `url` whenever one is present would approve `mode=fullscreen` with a `url` as a page
/// and then photograph the desktop.
pub fn screenshot_mode(args: &serde_json::Value) -> &str {
    match args.get("mode").and_then(serde_json::Value::as_str) {
        Some(mode) => mode,
        None if args.get("url").and_then(serde_json::Value::as_str).is_some() => "url",
        None => "fullscreen",
    }
}

#[cfg(test)]
mod tests;
