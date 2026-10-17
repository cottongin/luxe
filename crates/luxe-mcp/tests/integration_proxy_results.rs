//! What reaches the model from a real stdio MCP server: found by its server's name, described in
//! compact JSON, and answered with results that are compacted, capped and honest about what they
//! could not show. Skipped when `python3` is unavailable.

use std::sync::Arc;

use luxe_core::mcp_client::{McpServerConfig, McpTransport};
use luxe_core::tool::{Tool, ToolContext, ToolResultData};
use luxe_mcp::{McpProxyTool, ServerRegistry};
use tokio::sync::{mpsc, watch};

/// No tool's name or description says "acme", so a search for it can only succeed by the server.
const SERVER: &str = r#"
import sys, json
def send(m): sys.stdout.write(json.dumps(m)+"\n"); sys.stdout.flush()
SCHEMA={"type":"object","properties":{"project":{"type":"string","description":"The project id."}}}
TOOLS=[{"name":n,"description":d,"inputSchema":SCHEMA} for n,d in [
    ("list_tasks","List the tasks in a project."),("huge","Return a great deal."),
    ("mixed","Return a picture and a note."),("broken","Fail with details.")]]
PRETTY=json.dumps({"data":[{"gid":"1209","name":"Draft  the plan","done":False}]}, indent=2)
for line in sys.stdin:
    line=line.strip()
    if not line: continue
    msg=json.loads(line); mid=msg.get("id"); method=msg.get("method")
    if method=="initialize":
        send({"jsonrpc":"2.0","id":mid,"result":{"protocolVersion":"2024-11-05","capabilities":{},"serverInfo":{"name":"acme","version":"1"}}})
    elif method=="tools/list":
        send({"jsonrpc":"2.0","id":mid,"result":{"tools":TOOLS}})
    elif method=="tools/call":
        name=msg["params"]["name"]
        if name=="list_tasks": content=[{"type":"text","text":PRETTY}]; err=False
        elif name=="huge": content=[{"type":"text","text":"y"*70000}]; err=False
        elif name=="mixed": content=[{"type":"text","text":"a note"},{"type":"image","data":"AAAA","mimeType":"image/png"}]; err=False
        else: content=[{"type":"text","text":json.dumps({"error":"not found","project":"x"}, indent=2)}]; err=True
        send({"jsonrpc":"2.0","id":mid,"result":{"content":content,"isError":err}})
    elif mid is not None:
        send({"jsonrpc":"2.0","id":mid,"error":{"code":-32601,"message":"method not found"}})
"#;

fn python_available() -> bool {
    std::process::Command::new("python3").arg("--version").output().is_ok()
}

fn cx() -> ToolContext {
    let (event_tx, _rx) = mpsc::channel(8);
    let (_tx, cancel_rx) = watch::channel(false);
    ToolContext {
        cwd: std::path::PathBuf::from("."),
        session_id: "t".into(),
        cancelled: Arc::new(cancel_rx),
        event_tx,
        hook_ctx: None,
        question_broker: None,
    }
}

fn text_of(result: &ToolResultData) -> String {
    result
        .content
        .iter()
        .filter_map(|block| match block {
            luxe_core::message::ContentBlock::Text(text) => Some(text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

async fn proxy(dir: &tempfile::TempDir) -> McpProxyTool {
    let script = dir.path().join("acme.py");
    std::fs::write(&script, SERVER).unwrap();
    let config = McpServerConfig {
        id: "acme".into(),
        name: "Acme".into(),
        transport: McpTransport::Stdio {
            command: "python3".into(),
            args: vec![script.to_string_lossy().into_owned()],
            cwd: None,
            env: Default::default(),
        },
        enabled: true,
        ..Default::default()
    };
    let cache = dir.path().join("mcp-metadata.json");
    McpProxyTool::new(Arc::new(ServerRegistry::new(vec![config], Some(cache))))
}

async fn run(proxy: &McpProxyTool, args: serde_json::Value) -> (bool, String) {
    let result = proxy.run(args, &cx()).await;
    (result.is_error, text_of(&result))
}

#[tokio::test]
async fn a_server_is_found_by_its_name_and_described_compactly() {
    if !python_available() {
        eprintln!("python3 not available; skipping");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let proxy = proxy(&dir).await;
    let (_, hits) = run(&proxy, serde_json::json!({ "search": "acme" })).await;
    for tool in ["acme/list_tasks", "acme/huge", "acme/mixed", "acme/broken"] {
        assert!(hits.contains(tool), "{tool} is found by its server's name: {hits}");
    }
    let (_, described) = run(&proxy, serde_json::json!({ "tool": "acme/list_tasks" })).await;
    assert!(described.contains(r#""description":"The project id.""#), "{described}");
    assert!(!described.contains("\n  \""), "no indentation: {described}");
}

#[tokio::test]
async fn results_are_compacted_capped_and_say_what_they_could_not_show() {
    if !python_available() {
        eprintln!("python3 not available; skipping");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let proxy = proxy(&dir).await;
    let call = |tool: &str| serde_json::json!({ "tool": format!("acme/{tool}"), "args": "{}" });

    let (error, listed) = run(&proxy, call("list_tasks")).await;
    assert!(!error);
    assert_eq!(listed, r#"{"data":[{"gid":"1209","name":"Draft  the plan","done":false}]}"#);

    let (_, huge) = run(&proxy, call("huge")).await;
    assert!(huge.chars().count() < luxe_core::output_limit::RESULT_MAX_CHARS + 300);
    assert!(
        huge.contains("10000 more characters not shown (70000 total)"),
        "{}",
        &huge[huge.len() - 200..]
    );

    let (_, mixed) = run(&proxy, call("mixed")).await;
    assert_eq!(mixed, "a note\n(1 image not shown)");

    let (error, broken) = run(&proxy, call("broken")).await;
    assert!(error, "a server-reported error stays an error");
    assert!(broken.contains(r#"{"error":"not found","project":"x"}"#), "{broken}");
}
