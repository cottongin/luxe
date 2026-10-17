//! Read-only turns and modes — what is held to reading at the dispatch choke point.
//!
//! Two turns answer rather than act: a draft of a reply for GitHub, and the answer to a question
//! asked in Guided Review. Both are ordinary turns of the session, with its transcript and its
//! cache, and both must change nothing — a question asked to understand code the agent wrote is the
//! worst moment for that code to be rewritten. An instruction would only shape the turn, so the rule
//! is [`allows`], held in `dispatch` before the permission engine: reading and searching, no
//! writing, no shell, no posting.
//!
//! A mode that holds writes back ([`crate::mode::WriteScope`]) is the same rule for a whole stretch
//! of the session rather than one turn, with one opening: plan mode may write its plan files.
//! [`judge_for_mode`] is that rule.

/// What a read-only turn is for, which is what its refusals say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReadOnlyTurn {
    /// A reply drafted for the reader to send
    Drafting,
    /// Questions asked in Guided Review
    Answering,
}

/// Whether a read-only turn may run tool `name` with `args`: reading and searching only.
///
/// An allow-list, so a tool added later is refused here until someone decides it reads. Two
/// readers write when asked to — a rename or a code action applied, a structural replace applied —
/// and the `git` tool's verbs that change anything, or post, are not reading.
pub(super) fn allows(name: &str, args: &serde_json::Value) -> bool {
    let applies = args.get("apply").and_then(serde_json::Value::as_bool).unwrap_or(false);
    match name {
        "read"
        | "glob"
        | "grep"
        | "skill"
        | "web_fetch"
        | "web_search"
        | "question"
        | "symbol_search"
        | "read_symbol"
        | "module_report"
        | "impact"
        | "lsp_diagnostics"
        | crate::tool::ACTIVATE_TOOLS_NAME => true,
        "lsp_navigation" | "ast_grep" => !applies,
        "git" => {
            let action = args.get("action").and_then(serde_json::Value::as_str).unwrap_or("");
            let first = args
                .get("args")
                .and_then(serde_json::Value::as_array)
                .and_then(|args| args.first())
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            // `--output` makes git write the answer to a file instead of printing it; anything
            // shorter is ambiguous with `--output-indicator-*`, which git refuses
            let writes_a_file =
                args.get("args").and_then(serde_json::Value::as_array).is_some_and(|args| {
                    args.iter()
                        .filter_map(serde_json::Value::as_str)
                        .any(|arg| arg.starts_with("--output"))
                });
            match action {
                "status" | "log" | "diff" | "show" => !writes_a_file,
                "checks" => true,
                "pr" | "issue" => matches!(first, "view" | "list" | "diff" | "checks" | "status"),
                _ => false,
            }
        }
        _ => false,
    }
}

/// What a session's mode makes of one call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ModeVerdict {
    /// The mode has no objection
    Runs,
    /// A write to one of the mode's own files, by its project-relative path
    WritesPlan(String),
    /// The mode holds it back, and this is what the agent is told
    Refused(String),
}

/// The tools that write a file, with the argument keys their path may arrive under.
///
/// `edit` reads four spellings, and a call is judged by the same path the tool will act on, or a
/// call spelled `file_path` would get past a check that only read `path`.
const FILE_WRITERS: [(&str, &[&str]); 2] =
    [("write", &["path"]), ("edit", &["path", "file_path", "filePath", "file"])];

/// Whether `mode` lets `name` run with `args`, in the project at `root`.
///
/// Reading runs in every mode. So does `todo`, whose list belongs to the session rather than the
/// project. A mode that writes only some files lets `write` and `edit` through to those and nothing
/// else; one that writes nothing refuses them all. Anything else that changes things is refused,
/// the shell included: there is no telling what a command line will touch.
pub(crate) fn judge_for_mode(
    mode: &crate::mode::Mode,
    name: &str,
    args: &serde_json::Value,
    root: &std::path::Path,
) -> ModeVerdict {
    use crate::mode::WriteScope;
    if mode.writes == WriteScope::Anything || allows(name, args) || name == "todo" {
        return ModeVerdict::Runs;
    }
    let writer = FILE_WRITERS.iter().find(|(writer, _)| *writer == name);
    let (WriteScope::Only(files), Some((_, keys))) = (&mode.writes, writer) else {
        return ModeVerdict::Refused(mode_refusal(mode, name, args, None));
    };
    let raw = keys.iter().find_map(|key| args.get(*key).and_then(serde_json::Value::as_str));
    let Some(raw) = raw.filter(|raw| !raw.trim().is_empty()) else {
        return ModeVerdict::Refused(format!(
            "This session is in {} mode, which {}, and this `{name}` call names no file, so it \
             did not run. Name the file in `path`.",
            mode.name,
            mode.scope()
        ));
    };
    match project_relative(root, raw) {
        Some(path) if files.iter().any(|glob| crate::guided_review::matches_glob(glob, &path)) => {
            ModeVerdict::WritesPlan(path)
        }
        _ => ModeVerdict::Refused(mode_refusal(mode, name, args, Some(raw))),
    }
}

/// `raw` as a path inside `root`, with forward slashes, or `None` when it is not one.
///
/// Through the real directories rather than the spelling, so a plan file that is a symbolic link,
/// or sits in a linked directory, cannot carry a write somewhere the mode does not allow.
fn project_relative(root: &std::path::Path, raw: &str) -> Option<String> {
    let path = std::path::Path::new(raw);
    let joined = if path.is_absolute() { path.to_path_buf() } else { root.join(path) };
    let lexical = normalize(&joined);
    if std::fs::symlink_metadata(&lexical).is_ok_and(|meta| meta.file_type().is_symlink()) {
        return None;
    }
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let parent = lexical.parent()?;
    let parent = nearest_existing(parent)?;
    let file = lexical.strip_prefix(&parent.lexical).ok()?;
    let real = parent.real.join(file);
    if !crate::paths::contains(&root, &real) {
        return None;
    }
    real.strip_prefix(&root).ok().map(crate::paths::display).filter(|path| !path.is_empty())
}

/// An ancestor directory that exists, as spelled and as the filesystem resolves it.
struct Existing {
    lexical: std::path::PathBuf,
    real: std::path::PathBuf,
}

fn nearest_existing(path: &std::path::Path) -> Option<Existing> {
    let mut current = path;
    loop {
        if let Ok(real) = current.canonicalize() {
            return Some(Existing { lexical: current.to_path_buf(), real });
        }
        current = current.parent()?;
    }
}

/// `.` and `..` resolved without touching the filesystem.
fn normalize(path: &std::path::Path) -> std::path::PathBuf {
    use std::path::Component;
    let mut out = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// What the agent is told when its mode refuses a call.
fn mode_refusal(
    mode: &crate::mode::Mode,
    name: &str,
    args: &serde_json::Value,
    path: Option<&str>,
) -> String {
    let on = path.map(|path| format!(" on `{path}`")).unwrap_or_default();
    let said = format!(
        "This session is in {} mode, which {}, so `{name}`{on} did not run.",
        mode.name,
        mode.scope()
    );
    match mode.writes {
        crate::mode::WriteScope::Only(_) => format!(
            "{said} {} Put what should change in the plan; the user switches to build mode when it \
             is time to change things.",
            read_instead(name, args)
        ),
        _ => format!("{said} {}", read_instead(name, args)),
    }
}

/// What the agent is told when a read-only turn's call is refused.
///
/// A refusal that only says no costs a round trip and teaches nothing, and the call refused most is
/// a shell command that only meant to read. So a refused `bash` names the tool that reads the way it
/// tried to — the same table `bash`'s own description gives — and anything else says what this turn
/// allows.
pub(crate) fn refusal(turn: ReadOnlyTurn, name: &str, args: &serde_json::Value) -> String {
    let (purpose, then) = match turn {
        ReadOnlyTurn::Drafting => ("drafts a reply", "Then answer with the draft."),
        ReadOnlyTurn::Answering => (
            "answers the reviewer's questions",
            "Then answer. If the answer is that something should change, say what; the reviewer \
             decides whether to ask for it.",
        ),
    };
    let said = format!("This turn {purpose} and changes nothing, so `{name}` did not run.");
    format!("{said} {} {then}", read_instead(name, args))
}

/// The tool that reads the way a refused call tried to, said as an instruction.
///
/// A refusal that only says no costs a round trip and teaches nothing, and the call refused most is
/// a shell command that only meant to read. So a refused `bash` names the tool that reads the way it
/// tried to — the same table `bash`'s own description gives — and anything else names the readers.
fn read_instead(name: &str, args: &serde_json::Value) -> &'static str {
    let command = args.get("command").and_then(serde_json::Value::as_str).unwrap_or("");
    match (name, shell_program(command)) {
        ("bash", Some("cat" | "head" | "tail" | "nl" | "less" | "more" | "sed" | "awk")) => {
            "To read a file, use `read`, which takes several paths in one call and a line range."
        }
        ("bash", Some("ls" | "find" | "fd" | "tree")) => "To find files, use `glob`.",
        ("bash", Some("grep" | "rg" | "ag" | "ack")) => {
            "To search inside files, use `grep`, which can answer with files or counts."
        }
        ("bash", Some("git")) => "For history and changes, use the `git` tool's `log` and `diff`.",
        ("bash", Some("gh")) => "For a pull request or an issue, use the `git` tool's `pr view`.",
        _ => "Read with `read`, `grep` and `glob`, and the `git` tool's `log` and `diff`.",
    }
}

/// The program a shell command runs, past any `cd X &&` the model opened it with.
fn shell_program(command: &str) -> Option<&str> {
    let last = command.rsplit("&&").next().unwrap_or(command);
    last.split_whitespace().find(|word| !word.contains('='))
}

#[cfg(test)]
mod tests;
