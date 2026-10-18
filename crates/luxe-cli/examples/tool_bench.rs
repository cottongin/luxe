//! Tool bench — each tool against the shell command it replaces, measured.
//!
//! Runs both in the working tree it is started from and writes what each returned, in bytes and
//! lines, to the JSON the site's Tools page draws its bars from. A recording, like the captures:
//! the numbers move with the repository, nothing checks them, and the file names the commit they
//! came from. `scripts/tool-bench.sh` runs it.
//!
//! The tools run the way a session registers them, so the sizes are what a model would be sent.
//! The shell side captures stdout and stderr together, since an agent's shell sees both — and is
//! then put through each other harness's own shell-output limit ([`forwarded`]), because nobody's
//! model is sent a raw 900 KB `git log`: the honest comparison is with what a harness forwards.
//!
//! `TOOL_BENCH_CASES` names a JSON file of cases (`id`, `shell`, `tool`, `args`) for measuring in a
//! repository other than this one; without it, the cases below measure Luxe itself.

use std::process::Command;
use std::sync::Arc;

/// One comparison: a shell command, and the tool call that answers the same question.
#[derive(serde::Deserialize)]
struct Case {
    id: String,
    shell: String,
    tool: String,
    args: serde_json::Value,
}

/// What another harness sends its model of a shell command's output, in bytes.
///
/// Each limit is the harness's own, read from its source and checked against what crossed the
/// wire in the harness evaluation: Pi and OpenCode keep the last 2,000 lines
/// or 50 KB, Codex keeps the head and tail of about 10 KB, and Claude Code keeps 30,000
/// characters. A limit is all this models; a harness that also saves the rest to a file says so
/// in a line or two this leaves out.
fn forwarded(text: &str) -> serde_json::Value {
    let tail = |max_lines: usize, max_bytes: usize| {
        let lines: Vec<&str> = text.lines().collect();
        let kept = &lines[lines.len().saturating_sub(max_lines)..];
        kept.join("\n").len().min(max_bytes)
    };
    serde_json::json!({
        "pi": tail(2000, 50 * 1024),
        "opencode": tail(2000, 50 * 1024),
        "codex": text.len().min(10 * 1024),
        "claude": text.chars().take(30_000).map(char::len_utf8).sum::<usize>(),
    })
}

fn cases() -> Vec<Case> {
    if let Ok(path) = std::env::var("TOOL_BENCH_CASES") {
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        return serde_json::from_str(&text).unwrap_or_else(|e| panic!("{path}: {e}"));
    }
    let engine = "crates/luxe-core/src/engine.rs";
    let case = |id: &str, shell: &str, tool: &str, args: serde_json::Value| Case {
        id: id.to_string(),
        shell: shell.to_string(),
        tool: tool.to_string(),
        args,
    };
    vec![
        case(
            "glob",
            "find . -name '*.json'",
            "glob",
            serde_json::json!({ "pattern": "**/*.json" }),
        ),
        case("grep", "rg -n render", "grep", serde_json::json!({ "pattern": "render" })),
        case(
            "bash",
            "cargo tree -p luxe-cli",
            "bash",
            serde_json::json!({ "command": "cargo tree -p luxe-cli" }),
        ),
        case(
            "test",
            "cargo test -p luxe-tools",
            "test",
            serde_json::json!({ "command": "cargo test -p luxe-tools" }),
        ),
        case("git", "git log", "git", serde_json::json!({ "action": "log" })),
        case(
            "read_symbol",
            "cat crates/luxe-core/src/engine.rs",
            "read_symbol",
            serde_json::json!({ "path": engine, "symbol": "set_cancelled" }),
        ),
        case(
            "module_report",
            "cat crates/luxe-core/src/engine.rs",
            "module_report",
            serde_json::json!({ "path": engine }),
        ),
        case(
            "web_fetch",
            "curl -sL https://docs.rs/tokio/latest/tokio/",
            "web_fetch",
            serde_json::json!({ "url": "https://docs.rs/tokio/latest/tokio/" }),
        ),
    ]
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let out = std::env::args().nth(1).unwrap_or_else(|| "web/src/data/tool-bench.json".into());
    let cwd = std::env::current_dir()?;
    let registry = registry(&cwd);
    // held for the whole run: a dropped sender reads as a cancel to a tool waiting on it
    let (_cancel, cx) = context(&cwd);

    let mut rows = Vec::new();
    for case in cases() {
        let shell = Command::new("sh").arg("-c").arg(&case.shell).current_dir(&cwd).output()?;
        let shell_text = [shell.stdout, shell.stderr].concat();
        let shell_text = String::from_utf8_lossy(&shell_text);

        let tool = registry
            .get(&case.tool)
            .ok_or_else(|| anyhow::anyhow!("no tool named {}", case.tool))?;
        let result = tool.run(case.args.clone(), &cx).await;
        // A failing test run is the case worth measuring; any other error is a broken case
        anyhow::ensure!(
            !result.is_error || case.tool == "test",
            "{} failed: {:?}",
            case.id,
            result.content
        );
        let tool_text: String = result
            .content
            .iter()
            .filter_map(|block| match block {
                luxe_core::ContentBlock::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        eprintln!("{:<14} shell {:>9} B  tool {:>7} B", case.id, shell_text.len(), tool_text.len());
        rows.push(serde_json::json!({
            "id": case.id,
            "shell": case.shell,
            "tool": case.tool,
            "args": case.args,
            "shellBytes": shell_text.len(),
            "shellLines": shell_text.lines().count(),
            "toolBytes": tool_text.len(),
            "toolLines": tool_text.lines().count(),
            "forwarded": forwarded(&shell_text),
        }));
    }

    let commit = Command::new("git").args(["rev-parse", "--short", "HEAD"]).output()?;
    let repository = cwd.file_name().map(|name| name.to_string_lossy().into_owned());
    let report = serde_json::json!({
        "repository": repository,
        "commit": String::from_utf8_lossy(&commit.stdout).trim(),
        "rows": rows,
    });
    std::fs::write(&out, serde_json::to_string_pretty(&report)? + "\n")?;
    eprintln!("wrote {out}");
    Ok(())
}

/// The tools a session registers, minus the ones that wait on a person or a debugger.
fn registry(cwd: &std::path::Path) -> luxe_core::ToolRegistry {
    let mut registry = luxe_core::ToolRegistry::new();
    let web = luxe_tools::web::WebConfig::from_env();
    let outline = Arc::new(luxe_lsp::syntax::SyntaxOutline);
    let tracker = luxe_tools::register_builtins(&mut registry, None, web, Some(outline));
    let lsp = Arc::new(luxe_lsp::WorkspaceLspClient::with_servers(Default::default()));
    let graph = Arc::new(luxe_lsp::graph::GraphService::new(cwd));
    registry.register(Arc::new(
        luxe_lsp::ModuleReportTool::new(lsp.clone())
            .with_tracker(tracker.clone())
            .with_graph(graph),
    ));
    registry.register(Arc::new(luxe_lsp::ReadSymbolTool::new(lsp).with_tracker(tracker)));
    registry
}

fn context(cwd: &std::path::Path) -> (tokio::sync::watch::Sender<bool>, luxe_core::ToolContext) {
    let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(1024);
    tokio::spawn(async move { while event_rx.recv().await.is_some() {} });
    let cx = luxe_core::ToolContext {
        cwd: cwd.to_path_buf(),
        session_id: "tool-bench".into(),
        cancelled: Arc::new(cancel_rx),
        event_tx,
        hook_ctx: None,
        question_broker: None,
    };
    (cancel_tx, cx)
}
