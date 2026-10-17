//! Execution requests — `test` commands and `debug` launches judged by the Bash rules.
//!
//! Both tools start a process the model chose, so they get the same deny/ask rules and guards a
//! `bash` call would. The tool's own key stays checked too, so a rule written for `test` or
//! `debug` still holds.

use std::path::Path;

use super::{build_request, permission_key, primary_input, rule, Check, PermissionRequest};
use crate::permission::wildcard::command_segments;
use crate::permission::PermissionAction;

/// Shells whose `-c` argument is a script of its own.
const SHELLS: [&str; 6] = ["sh", "bash", "zsh", "dash", "ksh", "fish"];

/// Programs that run their first argument, so the shell may be the word after them.
const WRAPPERS: [&str; 5] = ["env", "busybox", "exec", "command", "nohup"];

/// The request for a call that runs a command, or `None` when this call runs nothing new.
pub(super) fn request(
    tool: &str,
    args: &serde_json::Value,
    root: &Path,
) -> Option<PermissionRequest> {
    let (command, script, rememberable) = match tool {
        "test" => {
            let (command, rememberable) = test_command(args)?;
            (command, None, rememberable)
        }
        "debug" if args.get("action").and_then(serde_json::Value::as_str) == Some("launch") => {
            let (command, script) = launch_command(args)?;
            let rememberable = script.is_none();
            (command, script, rememberable)
        }
        _ => return None,
    };
    let mut request = build_request("bash", &serde_json::json!({"command": command}), root);
    if let Some(script) = &script {
        let inner = build_request("bash", &serde_json::json!({"command": script}), root);
        request.checks.extend(inner.checks);
    }
    request.permission = permission_key(tool).into();
    request.tool = tool.into();
    let input = primary_input(tool, args);
    request
        .checks
        .insert(0, Check { permission: request.permission.clone(), input: input.clone() });
    // Exact rules only: approving `cargo test` here must not allow `cargo publish` through bash
    request.always = match (rememberable && !request.always.is_empty(), exact(&command)) {
        (true, Some(command)) => vec![
            rule("bash", command, PermissionAction::Allow),
            rule(&request.permission, &input, PermissionAction::Allow),
        ],
        _ => Vec::new(),
    };
    Some(request)
}

/// The command a `test` call runs, and whether approving it may be remembered.
///
/// The filter is appended unquoted, so `unit; rm -rf /` is two commands. Without an explicit
/// command the detected one is not known here, so only what follows the filter's first segment is
/// judged, and never remembered; a plain filter adds nothing.
fn test_command(args: &serde_json::Value) -> Option<(String, bool)> {
    let filter = args
        .get("filter")
        .and_then(serde_json::Value::as_str)
        // untrimmed, as the runner appends it: a leading newline is a separator
        .filter(|filter| !filter.trim().is_empty());
    match (args.get("command").and_then(serde_json::Value::as_str), filter) {
        (Some(command), Some(filter)) => Some((format!("{command} {filter}"), true)),
        (Some(command), None) => Some((command.into(), true)),
        (None, Some(filter)) => smuggled(filter).map(|tail| (tail, false)),
        (None, None) => None,
    }
}

fn smuggled(filter: &str) -> Option<String> {
    // behind a stand-in head, as it runs: a filter that opens with `;` still starts a new command
    let segments = command_segments(&format!("test {filter}"));
    let tail = segments.get(1..).filter(|tail| !tail.is_empty())?;
    let tail: Vec<String> = tail.iter().map(|segment| segment.join(" ")).collect();
    // the fork bomb is spotted on the raw text, which splitting into segments would destroy
    Some(if super::is_fork_bomb(filter) { filter.into() } else { tail.join(" ; ") })
}

/// The command line a `debug launch` runs, and the script a shell launched with `-c` would run.
fn launch_command(args: &serde_json::Value) -> Option<(String, Option<String>)> {
    let program = args.get("program")?.as_str()?;
    let arguments: Vec<&str> = args
        .get("args")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .collect();
    let command = std::iter::once(program).chain(arguments.iter().copied()).collect::<Vec<_>>();
    Some((command.join(" "), shell_script(program, &arguments)))
}

fn shell_script(program: &str, arguments: &[&str]) -> Option<String> {
    let words: Vec<&str> = std::iter::once(program).chain(arguments.iter().copied()).collect();
    let mut index = 0;
    while WRAPPERS.contains(&program_name(words.get(index)?)) {
        index += 1;
        // a wrapper's own flags and `env`'s assignments come before the program it runs
        while words.get(index).is_some_and(|word| word.starts_with('-') || word.contains('=')) {
            index += 1;
        }
    }
    if !SHELLS.contains(&program_name(words[index])) {
        return None;
    }
    let rest = &words[index + 1..];
    let flag = rest
        .iter()
        .position(|arg| arg.starts_with('-') && !arg.starts_with("--") && arg.contains('c'))?;
    rest.get(flag + 1).map(|script| (*script).to_string())
}

/// A program's bare name, whichever separator its path uses and with any `.exe` dropped.
fn program_name(word: &str) -> &str {
    let name = word.rsplit(['/', '\\']).next().unwrap_or(word);
    name.strip_suffix(".exe").unwrap_or(name)
}

/// `command` as a rule matching only itself, or `None` when a wildcard in it would widen the rule.
fn exact(command: &str) -> Option<&str> {
    (!command.contains(['*', '?'])).then_some(command)
}
