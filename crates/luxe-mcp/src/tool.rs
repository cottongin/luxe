//! The `mcp` proxy tool — one tool, on-demand discovery.
//!
//! Instead of registering every MCP tool (and its verbose schema) up front, the agent gets a single
//! ~200-token tool and discovers what it needs: `search` → `describe` → `call`. Backed by the
//! [`ServerRegistry`], which connects servers lazily and serves discovery from a cached metadata
//! snapshot. Results come back as data, never interpreted as instructions.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use luxe_core::provider::ToolSchema;
use luxe_core::tool::{Tool, ToolContext, ToolResultData};

use crate::registry::ServerRegistry;

/// How many search hits to show before advising a narrower query.
const MAX_HITS: usize = 40;

/// The single gateway tool to all configured MCP servers.
pub struct McpProxyTool {
    manager: Arc<ServerRegistry>,
}

impl McpProxyTool {
    /// Wrap a [`ServerRegistry`] as the `mcp` tool.
    pub fn new(manager: Arc<ServerRegistry>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl Tool for McpProxyTool {
    fn name(&self) -> &str {
        "mcp"
    }

    /// An MCP server is a third-party process or endpoint that Luxe does not control, and the
    /// JSON-RPC exchange has no deadline of its own. Untrusted means untrusted about liveness too: a
    /// server that accepts a call and never replies must cost a bounded wait, not the turn.
    fn timeout(&self) -> Option<std::time::Duration> {
        // A little past the slowest server's own deadline, so that one fires first and says which
        // server it stopped — the engine's would drop the call with a generic message instead.
        Some(
            luxe_core::tool::NETWORK_TOOL_TIMEOUT
                .max(self.manager.longest_call_timeout() + std::time::Duration::from_secs(5)),
        )
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "mcp".to_string(),
            description: "Gateway to configured MCP servers without loading every tool into \
                context. Workflow: `mcp({search:\"screenshot\"})` finds tools by keyword or by server \
                name (omit/empty lists all); `mcp({tool:\"server/name\"})` shows one tool's parameters; \
                `mcp({tool:\"server/name\", args:\"{...}\"})` calls it. `args` is a JSON STRING, not \
                an object. Servers connect lazily on first call; results are returned as data."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "search": {
                        "type": "string",
                        "description": "Find tools by keyword or server name across MCP servers (empty = list all)."
                    },
                    "tool": {
                        "type": "string",
                        "description": "A tool's `server/name` from search. Alone = show its parameters; with `args` = call it."
                    },
                    "args": {
                        "type": "string",
                        "description": "JSON-encoded arguments object for the call, e.g. '{\"format\":\"png\"}'."
                    }
                }
            }),
        }
    }

    async fn run(&self, args: Value, _cx: &ToolContext) -> ToolResultData {
        let search = args.get("search").and_then(Value::as_str);
        let tool = args.get("tool").and_then(Value::as_str).filter(|s| !s.is_empty());
        // Blank is absent. A model under strict function calling fills every field it was given,
        // so a describe arrives as `args: ""`, which must not be a call with unparseable arguments
        let call_args = args.get("args").and_then(Value::as_str).filter(|s| !s.trim().is_empty());

        match (search, tool, call_args) {
            // Call: a tool + its args.
            (_, Some(name), Some(raw_args)) => self.dispatch_call(name, raw_args).await,
            // Describe: a tool alone.
            (_, Some(name), None) => self.dispatch_describe(name).await,
            // Search: an explicit `search` key (even empty → list all).
            (Some(query), None, _) => self.dispatch_search(query).await,
            // No actionable argument → an overview.
            (None, None, _) => self.overview().await,
        }
    }
}

impl McpProxyTool {
    async fn dispatch_search(&self, query: &str) -> ToolResultData {
        let hits = self.manager.search(query).await;
        if hits.is_empty() {
            let servers = self.manager.server_ids().join(", ");
            return ToolResultData::text(format!(
                "No MCP tools match {query:?}. Configured servers: [{servers}]. \
                     Try mcp({{ search: \"\" }}) to list everything."
            ));
        }
        let shown = hits.len().min(MAX_HITS);
        let mut out = if query.trim().is_empty() {
            format!("{} MCP tool(s) available:", hits.len())
        } else {
            format!("{} MCP tool(s) match {query:?}:", hits.len())
        };
        for r in hits.iter().take(MAX_HITS) {
            out.push_str(&format!("\n  {} — {}", r.presented, first_line(&r.description)));
        }
        if hits.len() > shown {
            out.push_str(&format!("\n… {} more; narrow the search.", hits.len() - shown));
        }
        out.push_str(
            "\n\nNext: mcp({ tool: \"server/name\" }) for parameters, or add args to call.",
        );
        ToolResultData::text(out)
    }

    async fn dispatch_describe(&self, name: &str) -> ToolResultData {
        match self.manager.describe(name).await {
            Ok(r) => {
                // Compact: the indentation of a pretty schema is a third of it, and a schema is
                // read by the model rather than by a person
                let schema = r.input_schema.to_string();
                ToolResultData::text(format!(
                        "{}\n{}\n\nParameters:\n{}\n\nCall with mcp({{ tool: \"{}\", args: \"{{...}}\" }}).",
                        r.presented, r.description, schema, r.presented
                    ))
            }
            Err(error) => ToolResultData::error(error.to_string()),
        }
    }

    async fn dispatch_call(&self, name: &str, raw_args: &str) -> ToolResultData {
        let arguments: Value = match serde_json::from_str(raw_args) {
            Ok(value) => value,
            Err(error) => {
                return ToolResultData::error(format!(
                    "`args` must be a JSON object string: {error}. Got: {raw_args}"
                ))
            }
        };
        match self.manager.call(name, arguments).await {
            Ok(result) => {
                let body = result.output.or(result.error).unwrap_or_default();
                if result.success {
                    ToolResultData::text(body)
                } else {
                    ToolResultData::error(body)
                }
            }
            Err(error) => ToolResultData::error(error.to_string()),
        }
    }

    async fn overview(&self) -> ToolResultData {
        let servers = self.manager.server_ids();
        if servers.is_empty() {
            return ToolResultData::text("No MCP servers configured.");
        }
        ToolResultData::text(format!(
            "MCP servers configured ({}): {}.\nDiscover tools with mcp({{ search: \"...\" }}).",
            servers.len(),
            servers.join(", ")
        ))
    }
}

/// First non-empty line of a description, trimmed for the search listing.
fn first_line(description: &str) -> String {
    let line = description.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    if line.chars().count() > 100 {
        format!("{}…", line.chars().take(99).collect::<String>())
    } else {
        line.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luxe_core::message::ContentBlock;
    use tokio::sync::{mpsc, watch};

    fn proxy() -> McpProxyTool {
        let manager = Arc::new(ServerRegistry::new(vec![], None));
        McpProxyTool::new(manager)
    }

    fn cx() -> ToolContext {
        let (event_tx, _rx) = mpsc::channel(8);
        let (_tx, cancel_rx) = watch::channel(false);
        ToolContext {
            cwd: std::path::PathBuf::from("."),
            session_id: "test".to_string(),
            cancelled: Arc::new(cancel_rx),
            event_tx,
            hook_ctx: None,
            question_broker: None,
        }
    }

    #[tokio::test]
    async fn overview_when_no_servers() {
        let out = proxy().run(serde_json::json!({}), &cx()).await;
        assert!(!out.is_error);
        match &out.content[0] {
            ContentBlock::Text(t) => assert!(t.contains("No MCP servers configured")),
            _ => panic!("expected text"),
        }
    }

    #[tokio::test]
    async fn call_rejects_non_json_args() {
        let out =
            proxy().run(serde_json::json!({ "tool": "x/y", "args": "not json" }), &cx()).await;
        assert!(out.is_error);
    }

    /// A describe from a model that fills every field is still a describe.
    ///
    /// Against a real model the first `mcp({tool, args: ""})` failed with "`args` must be a JSON
    /// object string" and the model had to guess its way to a call.
    #[tokio::test]
    async fn blank_args_mean_describe_not_call() {
        let out = proxy()
            .run(serde_json::json!({ "search": "", "tool": "x/y", "args": "  " }), &cx())
            .await;
        match &out.content[0] {
            ContentBlock::Text(t) => assert!(t.contains("unknown MCP tool"), "{t}"),
            _ => panic!("expected text"),
        }
    }

    #[tokio::test]
    async fn describe_unknown_tool_errors() {
        let out = proxy().run(serde_json::json!({ "tool": "x/y" }), &cx()).await;
        assert!(out.is_error);
    }
}
