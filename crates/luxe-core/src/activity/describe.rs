//! Describe — the one-line label a tool call shows on its card.
//!
//! Built from the call's arguments, live and on replay alike, so a card reads the same after a
//! restart. Each tool with a shape of its own gets a `<tool>_label`; the rest fall back to
//! `key=value`.

use super::partial_json::PartialObject;
use std::path::Path;

/// A short, human-readable description of a tool call, e.g. `src/main.rs`, `cargo test`,
/// `"needle" in src/`, `add · wire the dock`.
///
/// Falls back to a compact `key=value` rendering of the scalar arguments rather than dumping raw
/// JSON — a card should read like a sentence, not a payload.
///
/// **A blank argument is an absent one.** A model under a strict JSON schema fills every property
/// it was shown, so a `todo add` arrives carrying `subject: ""`, `subjects: []`, `id: 0` and a
/// `status` nobody asked for. Read naively, that turned `{action}` into `{action} · ` — a card
/// wearing a separator with nothing after it.
///
/// `root` is where a `bash` call's shell starts, so a `cd` into it can be left off the label.
///
/// Args: `(tool, arguments, root)`.
pub fn describe_call(tool: &str, arguments: &serde_json::Value, root: impl AsRef<Path>) -> String {
    described_call(tool, arguments, root.as_ref()).unwrap_or_else(|| summarize_arguments(arguments))
}

/// The label for a call the model is **still writing**, from the fragment of its argument JSON
/// that has arrived so far. `None` while nothing in it names the call yet.
///
/// A `write`'s content *is* its argument, so the model spends far longer composing the call than
/// the tool spends running it — and a card that can only be labeled once the whole call has landed
/// spends that entire stretch saying nothing but the tool's name. The path is one key, it arrives
/// first, and it is the one thing a reader wants while the file streams past.
///
/// Only **complete** members count: a half-arrived value would put `src/fo` on the header and then
/// correct itself to `src/foo.rs`, which reads as a glitch. The bespoke shapes only — a fallback
/// `key=value` rendering of whichever arguments happen to have landed describes the *fragment*
/// rather than the call, and changes as more of it arrives.
///
/// Args: `(tool, partial_arguments, root)`.
pub fn describe_partial_call(
    tool: &str,
    partial_arguments: &str,
    root: impl AsRef<Path>,
) -> Option<String> {
    let arrived = PartialObject(partial_arguments).complete_members()?;
    described_call(tool, &arrived, root.as_ref()).filter(|label| !label.trim().is_empty())
}

/// The bespoke one-line description for a tool that has one, before any fallback.
fn described_call(tool: &str, arguments: &serde_json::Value, root: &Path) -> Option<String> {
    let args = Args(arguments);
    let described = match tool {
        "read" => read_label(args),
        "write" => args.text("path").map(str::to_string),
        "edit" | "patch" | "apply_patch" => edit_label(args),
        "bash" => bash_label(args, root),
        "grep" => grep_label(args),
        "glob" => glob_label(args),
        "ast_grep" => ast_grep_label(args),
        "git" => git_label(args),
        "test" => Some(test_label(args)),
        "todo" => todo_label(args),
        "task" => task_label(args),
        "skill" => skill_label(args),
        "activate_tools" | "activate" => args.joined("names"),
        "loop_control" => loop_control_label(args),
        "question" => args.text("prompt").map(first_line),
        "web_fetch" | "webfetch" => args.text("url").map(str::to_string),
        "web_search" | "websearch" => web_search_label(arguments),
        "symbol_search" => args.text("query").map(str::to_string),
        "module_report" | "lsp_diagnostics" => args.text("file").map(str::to_string),
        "read_symbol" => read_symbol_label(args),
        "lsp_navigation" => lsp_navigation_label(args),
        "impact" => impact_label(args),
        "screenshot" => Some(screenshot_label(args)),
        "debug" => debug_label(args),
        "mcp" => mcp_label(args),
        _ => None,
    };
    described.filter(|text| !text.trim().is_empty())
}

/// A call's arguments as a card header reads them: a blank string or an empty list is absent.
#[derive(Clone, Copy)]
struct Args<'a>(&'a serde_json::Value);

impl<'a> Args<'a> {
    fn text(self, key: &str) -> Option<&'a str> {
        self.0.get(key).and_then(serde_json::Value::as_str).filter(|value| !value.trim().is_empty())
    }

    fn number(self, key: &str) -> Option<u64> {
        self.0.get(key).and_then(serde_json::Value::as_u64)
    }

    fn flag(self, key: &str) -> Option<bool> {
        self.0.get(key).and_then(serde_json::Value::as_bool)
    }

    /// The non-blank strings in an array argument, trimmed; `None` when there are none.
    fn strings(self, key: &str) -> Option<Vec<&'a str>> {
        let values: Vec<&str> = self
            .0
            .get(key)
            .and_then(serde_json::Value::as_array)?
            .iter()
            .filter_map(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .collect();
        (!values.is_empty()).then_some(values)
    }

    fn joined(self, key: &str) -> Option<String> {
        self.strings(key).map(|values| values.join(" "))
    }
}

/// `"{action} · {detail}"`, or the bare action when there is no detail.
fn action_with(action: &str, detail: Option<&str>) -> String {
    match detail {
        Some(detail) => format!("{action} · {detail}"),
        None => action.to_string(),
    }
}

// === Labels ===

fn read_label(args: Args) -> Option<String> {
    args.text("path")
        .map(|path| match (args.number("offset"), args.number("limit")) {
            (Some(offset), Some(limit)) => format!("{path}:{offset}-{}", offset + limit),
            (Some(offset), None) => format!("{path}:{offset}…"),
            _ => path.to_string(),
        })
        // Several at once: the first few by name, the rest counted
        .or_else(|| {
            args.strings("paths").map(|paths| match paths.len() {
                count if count > 3 => format!("{} +{}", paths[..3].join(", "), count - 3),
                _ => paths.join(", "),
            })
        })
}

fn edit_label(args: Args) -> Option<String> {
    args.text("path").map(|path| {
        match args.0.get("edits").and_then(serde_json::Value::as_array).map(Vec::len) {
            Some(count) if count > 1 => format!("{path} · {count} edits"),
            _ => path.to_string(),
        }
    })
}

/// What the command is for, in the model's words, when it gave some: a chained shell line is the
/// hardest thing in a transcript to read at speed, and the command itself is one line under it in
/// the card.
fn bash_label(args: Args, root: &Path) -> Option<String> {
    bash_description(args.0)
        .map(|description| clip(&first_line(description), MAX_DESCRIPTION_CHARS))
        .or_else(|| {
            let command = args.text("command")?;
            Some(first_line(super::bash_display::strip_redundant_cd(command, root)))
        })
}

/// Search is the most-run tool in a session, so its one header line is the line a user reads
/// most. It has to carry the arguments that change *what was searched* — a scoped glob and an
/// ignored-files sweep are different searches, and a header that hides them quietly misreports
/// what the agent did.
fn grep_label(args: Args) -> Option<String> {
    args.text("pattern").map(|pattern| {
        let mut label = match args.text("path") {
            Some(path) => format!("\"{pattern}\" in {path}"),
            None => format!("\"{pattern}\""),
        };
        label.push_str(&search_scope(args.0));
        label
    })
}

fn glob_label(args: Args) -> Option<String> {
    args.text("pattern").map(|pattern| {
        let mut label = match args.text("path") {
            Some(path) => format!("{pattern} in {path}"),
            None => pattern.to_string(),
        };
        label.push_str(&search_scope(args.0));
        label
    })
}

fn ast_grep_label(args: Args) -> Option<String> {
    args.text("pattern").map(|pattern| {
        let action = args.text("action").unwrap_or("search");
        match args.text("rewrite") {
            Some(rewrite) => format!("{action} {pattern} → {rewrite}"),
            None => format!("{action} {pattern}"),
        }
    })
}

/// A commit's subject is the one git argument that never rides in `args`, so a header built from
/// `args` alone showed `git commit` and nothing about the commit. Flags keep their command-line
/// position and the subject follows the separator, cut to one line — a message is the git argument
/// most likely to arrive with several.
fn git_label(args: Args) -> Option<String> {
    args.text("action").map(|action| {
        let head = match args.joined("args") {
            Some(rest) => format!("{action} {}", first_line(&rest)),
            None => action.to_string(),
        };
        match args.text("message") {
            Some(message) => format!("{head} · {}", first_line(message)),
            None => head,
        }
    })
}

fn test_label(args: Args) -> String {
    match (args.text("command"), args.text("filter")) {
        (Some(command), Some(filter)) => format!("{command} · {filter}"),
        (Some(command), None) => command.to_string(),
        (None, Some(filter)) => format!("filter {filter}"),
        (None, None) => "project tests".to_string(),
    }
}

/// No id on the header. The checklist the user reads never numbers its items, so `#3` is a handle
/// for the model and noise for everyone else; where an item is *going* is the part worth a slot,
/// and its name arrives with the result.
fn todo_label(args: Args) -> Option<String> {
    args.text("action").map(|action| {
        let detail = match action {
            "add" => todo_subject_detail(args.strings("subjects"), args.text("subject")),
            "update" => args.text("status").map(str::to_string),
            _ => None,
        };
        action_with(action, detail.as_deref())
    })
}

fn task_label(args: Args) -> Option<String> {
    let detail = args.text("title").or_else(|| args.text("id_or_slug"));
    args.text("action").map(|action| action_with(action, detail))
}

/// Loading is the only thing a *named* skill call does, so the action word adds nothing to the
/// card. An unnamed call (discover) has only its action to go on, and any future named action
/// still says which, so the label never implies the wrong one.
fn skill_label(args: Args) -> Option<String> {
    args.text("name")
        .map(|name| match args.text("action") {
            Some(action) if action != "load" => format!("{action} · {name}"),
            _ => name.to_string(),
        })
        .or_else(|| args.text("action").map(str::to_string))
}

fn loop_control_label(args: Args) -> Option<String> {
    let reason = args.text("reason").map(first_line);
    args.text("action").map(|action| action_with(action, reason.as_deref()))
}

/// The header names the first search and counts the rest; the card's body lists them all.
/// `queries=[3]` — what the generic fallback made of an array — said a number where the one thing
/// a reader wants is the words the agent went looking for.
fn web_search_label(arguments: &serde_json::Value) -> Option<String> {
    match web_search_queries(arguments).split_first() {
        Some((first, [])) => Some(first.clone()),
        Some((first, rest)) => Some(format!("{first} +{} more", rest.len())),
        None => None,
    }
}

fn read_symbol_label(args: Args) -> Option<String> {
    args.text("file")
        .map(|file| match args.text("symbol") {
            Some(symbol) => format!("{symbol} in {file}"),
            None => file.to_string(),
        })
        .or_else(|| args.text("symbol").map(str::to_string))
}

fn lsp_navigation_label(args: Args) -> Option<String> {
    args.text("file").map(|file| match (args.text("action"), args.number("line")) {
        (Some(action), Some(line)) => format!("{action} · {file}:{line}"),
        (Some(action), None) => format!("{action} · {file}"),
        (None, Some(line)) => format!("{file}:{line}"),
        (None, None) => file.to_string(),
    })
}

fn impact_label(args: Args) -> Option<String> {
    args.text("target").map(|target| match args.flag("verify") {
        Some(true) => format!("{target} · verified"),
        _ => target.to_string(),
    })
}

/// A capture's header names what was captured, and nothing else. The fallback lists every
/// argument the call carried, which for this tool means the four optional ones a model habitually
/// sends as empty strings — `output= · region= · window=` above a picture that already shows what
/// it is.
fn screenshot_label(args: Args) -> String {
    let mode = args.text("mode").unwrap_or("fullscreen");
    let subject = ["url", "window", "region", "output"].into_iter().find_map(|key| args.text(key));
    action_with(mode, subject.or_else(|| args.text("note")))
}

fn debug_label(args: Args) -> Option<String> {
    let detail = ["program", "session", "expression"].into_iter().find_map(|key| args.text(key));
    args.text("action").map(|action| action_with(action, detail))
}

fn mcp_label(args: Args) -> Option<String> {
    args.text("tool")
        .map(str::to_string)
        .or_else(|| args.text("search").map(|query| format!("search {query}")))
}

// === Helpers ===

/// Longest a `bash` description may run on a card's header.
pub(super) const MAX_DESCRIPTION_CHARS: usize = 80;

/// `text` cut to `max` characters at a word, marked when cut.
fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let head: String = text.chars().take(max).collect();
    let cut = head.rfind(' ').unwrap_or(head.len());
    format!("{}…", head[..cut].trim_end())
}

/// A `bash` call's own description of what it does, when it gave one that says anything.
pub(super) fn bash_description(arguments: &serde_json::Value) -> Option<&str> {
    arguments
        .get("description")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|description| !description.is_empty())
}

/// What a `todo add` is adding: the item, or the first of a whole plan and how much came with it.
fn todo_subject_detail(subjects: Option<Vec<&str>>, subject: Option<&str>) -> Option<String> {
    match subjects.as_deref() {
        Some([only]) => Some((*only).to_string()),
        Some([first, rest @ ..]) => Some(format!("{first} +{} more", rest.len())),
        _ => subject.map(str::to_string),
    }
}

/// The queries one `web_search` call will run: `queries` first, then `query` when it adds a new
/// one, each trimmed, with blanks and repeats dropped.
///
/// Shared with the tool that runs them rather than re-derived here, because a card that lists a
/// search the call never made is worse than one that lists nothing — and two copies of this would
/// disagree the first time either side grew an argument.
pub fn web_search_queries(arguments: &serde_json::Value) -> Vec<String> {
    let mut queries: Vec<String> = Vec::new();
    let mut push = |value: &str| {
        let value = value.trim();
        if !value.is_empty() && !queries.iter().any(|seen| seen == value) {
            queries.push(value.to_string());
        }
    };
    if let Some(list) = arguments.get("queries").and_then(serde_json::Value::as_array) {
        for entry in list.iter().filter_map(serde_json::Value::as_str) {
            push(entry);
        }
    }
    if let Some(single) = arguments.get("query").and_then(serde_json::Value::as_str) {
        push(single);
    }
    queries
}

/// The scope suffix a search header carries: the file filter it was narrowed to, whether the match
/// was inverted, what shape it asked for back, and whether it swept ignored files (build output,
/// vendored code) that a normal search skips.
///
/// Each of these makes it a different search, and a header that hides them misreports what the
/// agent did — a run of cards reading `"Tool"` three times over should not conceal that one of
/// them only asked which files matched.
fn search_scope(arguments: &serde_json::Value) -> String {
    let mut parts = Vec::new();
    if let Some(glob) = arguments.get("glob").and_then(serde_json::Value::as_str) {
        if !glob.trim().is_empty() {
            parts.push(glob.to_string());
        }
    }
    if arguments.get("invert").and_then(serde_json::Value::as_bool).unwrap_or(false) {
        parts.push("inverted".to_string());
    }
    // Both spellings, because the tool accepts both and a card that reads the advertised one only
    // would describe a resumed session's call as the search it was not.
    match crate::tool::arg(arguments, &["outputMode", "output_mode"])
        .and_then(serde_json::Value::as_str)
    {
        Some("files") => parts.push("files only".to_string()),
        Some("count") => parts.push("counts".to_string()),
        _ => {}
    }
    if arguments.get("all").and_then(serde_json::Value::as_bool).unwrap_or(false) {
        parts.push("+ignored".to_string());
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" · {}", parts.join(" · "))
    }
}

/// The first non-empty line of `text`, ellipsized if more follows — a card is one line.
fn first_line(text: &str) -> String {
    let mut lines = text.lines().map(str::trim).filter(|line| !line.is_empty());
    let first = lines.next().unwrap_or_default().to_string();
    if lines.next().is_some() {
        format!("{first} …")
    } else {
        first
    }
}

/// Fallback for a tool with no bespoke shape: `key=value` over its scalar arguments.
fn summarize_arguments(arguments: &serde_json::Value) -> String {
    let Some(fields) = arguments.as_object() else {
        return String::new();
    };
    let mut parts = Vec::new();
    for (key, value) in fields {
        let rendered = match value {
            serde_json::Value::String(text) => first_line(text),
            serde_json::Value::Number(number) => number.to_string(),
            serde_json::Value::Bool(flag) => flag.to_string(),
            serde_json::Value::Array(items) => format!("[{}]", items.len()),
            serde_json::Value::Object(_) => "{…}".to_string(),
            serde_json::Value::Null => continue,
        };
        parts.push(format!("{key}={rendered}"));
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests;
