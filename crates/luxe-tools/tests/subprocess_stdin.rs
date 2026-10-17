//! Every tool subprocess decides what its stdin is, and dies when the turn does.
//!
//! Source scans, because both guard a *default* — and a default cannot be caught by testing the
//! code that takes it.

use std::path::Path;

/// Every `Command::new` in this crate sets `stdin` explicitly.
///
/// The break this catches, and it shipped: `grep` let `Command::output()` supply the default, which
/// is an empty pipe. ripgrep handed a pattern, no path and a readable stdin searches *that pipe*, so
/// every default search returned "No matches found" — a wrong answer reported as a success. Every
/// other launch site in the crate already chose `Stdio::null()`; `grep` was the one that inherited.
///
/// The rule is "choose", not "choose null": an MCP transport or a formatter that is fed on stdin
/// wants a pipe. What is banned is saying nothing and taking whatever the runtime hands over.
#[test]
fn every_tool_subprocess_chooses_its_own_stdin() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut inherited = Vec::new();

    for file in walk(&src) {
        let text = std::fs::read_to_string(&file).expect("a readable source file");
        // Trim the test module: fixtures spawn helpers whose stdin is not a tool's concern.
        let body = text.split("\n#[cfg(test)]\nmod tests {").next().unwrap_or(&text);

        for (number, line) in body.lines().enumerate() {
            if !line.contains("Command::new(") {
                continue;
            }
            // A builder is often configured over the following lines rather than in one chain, so
            // the window is generous: this guards a convention, and a near miss that reads as a
            // pass is cheaper than a rule people learn to work around.
            let window: String = body.lines().skip(number).take(16).collect::<Vec<_>>().join("\n");
            if !window.contains(".stdin(") {
                let name = file.strip_prefix(&src).unwrap_or(&file).display();
                inherited.push(format!("{name}:{}: {}", number + 1, line.trim()));
            }
        }
    }

    assert!(
        inherited.is_empty(),
        "these launch a subprocess without choosing its stdin, and the default is an empty pipe \
         a search tool will happily search:\n  {}",
        inherited.join("\n  ")
    );
}

/// Every `Command::new` in this crate is `kill_on_drop`, or kills its own process group.
///
/// The engine cancels a tool by **dropping its future** — that is the whole mechanism, and the
/// obligation it buys is on the tool side. A spawn that opts out of neither outlives the turn that
/// was waiting on it: `grep`, `glob` and `ast_grep` all did, so cancelling a search on a large tree
/// left ripgrep, `fd` or ast-grep running with nothing left to receive their output.
///
/// `bash` is the exception the rule needs: it owns a process group and reaps it through a drop
/// guard, because `kill_on_drop` reaps the shell and orphans everything the shell started.
#[test]
fn every_tool_subprocess_dies_with_the_turn_that_started_it() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut survivors = Vec::new();

    for file in walk(&src) {
        let text = std::fs::read_to_string(&file).expect("a readable source file");
        let body = text.split("\n#[cfg(test)]\nmod ").next().unwrap_or(&text);

        for (number, line) in body.lines().enumerate() {
            // `std::process::Command` is synchronous: it cannot be dropped mid-wait, so there is no
            // future to cancel and nothing to reap.
            if !line.contains("Command::new(") || line.contains("std::process::Command::new(") {
                continue;
            }
            let window: String = body.lines().skip(number).take(16).collect::<Vec<_>>().join("\n");
            let reaped = window.contains(".kill_on_drop(")
                || window.contains("kill_on_drop(true)")
                || window.contains("detach_from_terminal(");
            if !reaped {
                let name = file.strip_prefix(&src).unwrap_or(&file).display();
                survivors.push(format!("{name}:{}: {}", number + 1, line.trim()));
            }
        }
    }

    assert!(
        survivors.is_empty(),
        "these spawn a process that outlives a cancelled turn — add `.kill_on_drop(true)`, or own \
         the process group and reap it:\n  {}",
        survivors.join("\n  ")
    );
}

/// Every `.rs` file under a directory.
fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else { return found };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(walk(&path));
        } else if path.extension().is_some_and(|ext| ext == "rs")
            && path.file_name().is_some_and(|name| name != "tests.rs")
        {
            // a sibling `tests.rs` is the test module, whose fixtures are not a tool's concern
            found.push(path);
        }
    }
    found
}
