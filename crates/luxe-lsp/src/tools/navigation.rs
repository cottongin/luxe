//! The `lsp_navigation` tool — IDE navigation over a warm, multi-language server.
//!
//! Supports definition/type-definition/declaration/implementation, references, hover,
//! document/workspace symbols, signature help, call hierarchy, code actions, and a rename
//! *preview*. Positions may be given as `line`/`character` (1-based) or resolved from a `symbol`
//! name via `documentSymbol`. Uses the shared warm [`WorkspaceLspClient`] service.

use super::*;
use crate::client::LspClient;
use crate::edit_apply::Writes;
use crate::protocol::{symbol_kind_name, FlatSymbol, Location, Range};
use crate::workspace::WorkspaceLspClient;
use luxe_core::read_tracker::ReadTracker;
use serde_json::Value;
use std::time::Duration;

const POSITION_ACTIONS: &str = "go_to_definition, type_definition, declaration, implementation, \
                                find_references, hover, signature_help, call_hierarchy, \
                                code_action, rename";

fn lsp_navigation_schema() -> ToolSchema {
    ToolSchema {
        name: "lsp_navigation".to_string(),
        description: "Navigate code via a warm language server. Give a position as line+character \
                      (1-based) OR a `symbol` name — every position-based action accepts either. \
                      Params by action: \
                      go_to_definition|type_definition|declaration|implementation|find_references|\
                      hover|signature_help{path, position}; document_symbols{path}; \
                      workspace_symbol{path, query}; call_hierarchy{path, position, direction?}; \
                      rename{path, position, newName, apply?}; \
                      code_action{path, position, apply?, title? | index?}. rename and code_action \
                      preview by default; pass apply:true to write the edits."
            .to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path to the file (relative to cwd)" },
                "action": {
                    "type": "string",
                    "enum": [
                        "go_to_definition", "type_definition", "declaration", "implementation",
                        "find_references", "hover", "document_symbols", "workspace_symbol",
                        "signature_help", "call_hierarchy", "code_action", "rename"
                    ],
                    "description": "Navigation action to perform"
                },
                "line": { "type": "integer", "description": "1-based line for position-based actions" },
                "character": { "type": "integer", "description": "1-based column for position-based actions" },
                "symbol": { "type": "string", "description": "Resolve the position from this symbol name (instead of line/character)" },
                "query": { "type": "string", "description": "workspace_symbol: symbol name to search for" },
                "direction": { "type": "string", "enum": ["incoming", "outgoing"], "description": "call_hierarchy direction (default incoming)" },
                "newName": { "type": "string", "description": "rename: the new symbol name" },
                "apply": { "type": "boolean", "description": "rename/code_action: apply the edits to disk (default false = preview)" },
                "title": { "type": "string", "description": "code_action apply: title of the action to apply" },
                "index": { "type": "integer", "description": "code_action apply: 0-based index of the action to apply (default 0)" }
            },
            "required": ["path", "action"]
        }),
    }
}

/// The `lsp_navigation` tool.
pub struct LspNavigationTool {
    service: Arc<WorkspaceLspClient>,
    tracker: Option<Arc<ReadTracker>>,
}

impl LspNavigationTool {
    /// Creates a navigation tool backed by the shared warm service.
    pub fn new(service: Arc<WorkspaceLspClient>) -> Self {
        Self { service, tracker: None }
    }

    /// Attach the shared read tracker, so a file an applied edit rewrote stays editable.
    pub fn with_tracker(mut self, tracker: Arc<ReadTracker>) -> Self {
        self.tracker = Some(tracker);
        self
    }
}

#[async_trait]
impl Tool for LspNavigationTool {
    fn name(&self) -> &str {
        "lsp_navigation"
    }

    fn argument_aliases(&self) -> &'static [&'static str] {
        super::FILE_ALIASES
    }

    fn schema(&self) -> ToolSchema {
        lsp_navigation_schema()
    }

    async fn run(&self, args: serde_json::Value, cx: &ToolContext) -> ToolResultData {
        let Some(file_str) = super::file_arg(&args) else {
            return ToolResultData::error("Error: 'path' argument is required");
        };
        let Some(action) = args.get("action").and_then(|v| v.as_str()).map(str::to_string) else {
            return ToolResultData::error("Error: 'action' argument is required");
        };
        let full_path = cx.cwd.join(file_str);
        let uri = crate::uri::from_path(&full_path);
        // Read the file once: opening the document lets a cold server answer, and the content backs
        // tree-sitter symbol-name resolution.
        let content = std::fs::read_to_string(&full_path).ok();

        let client = { self.service.client_for_path(&full_path).await };
        let Some(client) = client else {
            return ToolResultData::error(format!(
                "Error: {}",
                self.service.missing_server_hint(&full_path)
            ));
        };
        let mut guard = client.lock().await;
        if let Some(source) = &content {
            let _ = guard.open_document(&uri, source).await;
        }
        // Navigation/symbol queries need the crate indexed; a warm server returns instantly.
        guard.wait_until_idle(NAV_READY_WAIT).await;
        let writes = Writes { cx, tracker: self.tracker.as_deref() };
        dispatch(&mut guard, &uri, &action, &args, writes, &full_path, content.as_deref()).await
    }
}

/// How long a navigation call waits for a cold server to finish indexing before querying.
const NAV_READY_WAIT: Duration = Duration::from_secs(45);

/// Runs one navigation action and formats the result.
#[allow(clippy::too_many_arguments)]
async fn dispatch(
    client: &mut LspClient,
    uri: &str,
    action: &str,
    args: &serde_json::Value,
    writes: Writes<'_>,
    full_path: &Path,
    content: Option<&str>,
) -> ToolResultData {
    // Actions that don't take a position first.
    match action {
        "workspace_symbol" => {
            let Some(query) = args.get("query").and_then(Value::as_str) else {
                return ToolResultData::error("Error: 'query' is required for workspace_symbol");
            };
            return match client.workspace_symbol(query).await {
                Ok(symbols) => ToolResultData::text(format_symbols(&symbols, true)),
                Err(e) => ToolResultData::error(format!("Error: workspace_symbol failed: {e}")),
            };
        }
        "document_symbols" => {
            return match client.document_symbols(uri).await {
                Ok(symbols) => ToolResultData::text(format_symbols(&symbols, false)),
                Err(e) => ToolResultData::error(format!("Error: document_symbols failed: {e}")),
            };
        }
        _ => {}
    }

    // Everything else needs a position (from line/character or a symbol name).
    let position = match resolve_position(client, uri, args, full_path, content).await {
        Ok(position) => position,
        Err(message) => return ToolResultData::error(&message),
    };

    match action {
        "go_to_definition" => match client.goto_definition(uri, position).await {
            Ok(Some(loc)) => ToolResultData::text(format!("Definition: {}", format_location(&loc))),
            Ok(None) => ToolResultData::text(nothing_found("No definition found", client)),
            Err(e) => ToolResultData::error(format!("Error: definition failed: {e}")),
        },
        "type_definition" => locations_reply(
            client.goto_type_definition(uri, position).await,
            "type definition",
            client,
        ),
        "declaration" => {
            locations_reply(client.goto_declaration(uri, position).await, "declaration", client)
        }
        "implementation" => locations_reply(
            client.goto_implementation(uri, position).await,
            "implementation",
            client,
        ),
        "find_references" => {
            locations_reply(client.find_references(uri, position).await, "reference", client)
        }
        "hover" => match client.hover(uri, position).await {
            Ok(Some(markup)) => ToolResultData::text(markup.value),
            Ok(None) => ToolResultData::text(nothing_found("No hover information", client)),
            Err(e) => ToolResultData::error(format!("Error: hover failed: {e}")),
        },
        "signature_help" => match client.signature_help(uri, position).await {
            Ok(value) => ToolResultData::text(format_signature_help(&value)),
            Err(e) => ToolResultData::error(format!("Error: signature_help failed: {e}")),
        },
        "call_hierarchy" => call_hierarchy(client, uri, position, args).await,
        "code_action" => {
            let range = Range { start: position.clone(), end: position };
            let actions = match client.code_action(uri, range).await {
                Ok(actions) => actions,
                Err(e) => return ToolResultData::error(format!("Error: code_action failed: {e}")),
            };
            if !args.get("apply").and_then(Value::as_bool).unwrap_or(false) {
                return ToolResultData::text(format_code_actions(&actions));
            }
            apply_code_action(client, &actions, args, writes).await
        }
        "rename" => {
            let Some(new_name) = args.get("newName").and_then(Value::as_str) else {
                return ToolResultData::error("Error: 'newName' is required for rename");
            };
            let edit = match client.rename(uri, position, new_name).await {
                Ok(edit) => edit,
                Err(e) => return ToolResultData::error(format!("Error: rename failed: {e}")),
            };
            if args.get("apply").and_then(Value::as_bool).unwrap_or(false) {
                crate::edit_apply::apply_workspace_edit(&edit, writes).await
            } else {
                ToolResultData::text(format_rename_preview(&edit))
            }
        }
        other => ToolResultData::error(format!(
            "Error: unknown action '{other}'. Position actions: {POSITION_ACTIONS}."
        )),
    }
}

/// Resolve a position from `line`/`character` (1-based) or a `symbol` name.
///
/// Symbol names resolve via **tree-sitter** first (offline, reliable even on a cold server), then
/// fall back to the server's `documentSymbol`.
async fn resolve_position(
    client: &mut LspClient,
    uri: &str,
    args: &serde_json::Value,
    full_path: &Path,
    content: Option<&str>,
) -> Result<Position, String> {
    if let Some(line) = args.get("line").and_then(Value::as_u64) {
        let character = args.get("character").and_then(Value::as_u64).unwrap_or(1);
        return Ok(Position {
            line: (line.saturating_sub(1)) as u32,
            character: (character.saturating_sub(1)) as u32,
        });
    }
    let Some(name) = args.get("symbol").and_then(Value::as_str) else {
        return Err("Error: provide a `line` (1-based) or a `symbol` name".to_string());
    };

    // Tree-sitter first.
    if let Some(source) = content {
        if let Some(symbols) = crate::syntax::outline(full_path, source) {
            if let Some(found) = symbols
                .iter()
                .find(|s| s.name == name)
                .or_else(|| symbols.iter().find(|s| s.name.contains(name)))
            {
                return Ok(Position { line: found.line, character: found.character });
            }
        }
    }

    // Fall back to the language server's document outline.
    let symbols =
        client.document_symbols(uri).await.map_err(|e| format!("symbol lookup failed: {e}"))?;
    symbols
        .iter()
        .find(|s| s.name == name)
        .or_else(|| symbols.iter().find(|s| s.name.contains(name)))
        .map(|found| Position { line: found.line, character: found.character })
        .ok_or_else(|| format!("symbol '{name}' not found in this file"))
}

async fn call_hierarchy(
    client: &mut LspClient,
    uri: &str,
    position: Position,
    args: &serde_json::Value,
) -> ToolResultData {
    let items = match client.prepare_call_hierarchy(uri, position).await {
        Ok(items) => items,
        Err(e) => return ToolResultData::error(format!("Error: call_hierarchy failed: {e}")),
    };
    let Some(item) = items.into_iter().next() else {
        return ToolResultData::text("No call hierarchy at this position");
    };
    let outgoing = args.get("direction").and_then(Value::as_str) == Some("outgoing");
    let (calls, verb, side) = if outgoing {
        (client.outgoing_calls(item).await, "calls out to", "to")
    } else {
        (client.incoming_calls(item).await, "called by", "from")
    };
    match calls {
        Ok(calls) if calls.is_empty() => ToolResultData::text(format!("No {verb} entries")),
        Ok(calls) => {
            let mut out = format!("{} {} entry(ies):\n", calls.len(), verb);
            for call in &calls {
                if let Some(node) = call.get(side) {
                    out.push_str(&format!("  {}\n", format_hierarchy_node(node)));
                }
            }
            ToolResultData::text(out)
        }
        Err(e) => ToolResultData::error(format!("Error: call_hierarchy failed: {e}")),
    }
}

/// Select + apply one code action (inline `edit`, else `codeAction/resolve`; commands unsupported).
async fn apply_code_action(
    client: &mut LspClient,
    actions: &[Value],
    args: &serde_json::Value,
    writes: Writes<'_>,
) -> ToolResultData {
    if actions.is_empty() {
        return ToolResultData::text("No code actions available");
    }
    let chosen = if let Some(title) = args.get("title").and_then(Value::as_str) {
        actions.iter().find(|a| a.get("title").and_then(Value::as_str) == Some(title))
    } else {
        let index = args.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
        actions.get(index)
    };
    let Some(action) = chosen else {
        return ToolResultData::error(
            "Error: no matching code action; list them first (omit apply)",
        );
    };

    // Prefer the inline edit; otherwise resolve it; commands aren't executed here.
    let edit = if let Some(edit) = action.get("edit") {
        Some(edit.clone())
    } else {
        client
            .send_request("codeAction/resolve", action.clone())
            .await
            .ok()
            .and_then(|resolved| resolved.get("edit").cloned())
    };
    match edit {
        Some(edit) => crate::edit_apply::apply_workspace_edit(&edit, writes).await,
        None => ToolResultData::error(
            "Error: this code action runs a command, not an inline edit; command execution is not \
             supported yet",
        ),
    }
}

mod format;

use format::*;

#[cfg(test)]
mod tests {
    use super::*;

    /// Adding host-file excerpts to server locations must not expose the target's contents.
    #[test]
    fn server_locations_do_not_expand_target_contents() {
        let dir = tempfile::tempdir().unwrap();
        let secret = dir.path().join(".env");
        std::fs::write(&secret, "LOCATION_CONTENT_MUST_NOT_BE_RETURNED").unwrap();
        let location: Location = serde_json::from_value(serde_json::json!({
            "uri": crate::uri::from_path(&secret),
            "range": {"start": {"line": 2, "character": 4}, "end": {"line": 2, "character": 7}}
        }))
        .unwrap();
        let formatted = format_location(&location);
        assert!(formatted.ends_with(".env:3:5"), "{formatted}");
        assert!(!formatted.contains("LOCATION_CONTENT_MUST_NOT_BE_RETURNED"), "{formatted}");
    }
}
