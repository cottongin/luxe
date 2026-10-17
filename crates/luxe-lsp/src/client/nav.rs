//! LSP navigation requests — definition, references, symbols, hover, call hierarchy, and more.
//!
//! Responses are parsed **leniently**: real servers return `LocationLink[]` for definition and
//! hierarchical `DocumentSymbol[]` for document symbols, not just the simplest shapes, so the
//! helpers here normalize both. Complex results (signature help, call hierarchy, code actions,
//! rename edits) are returned as raw `Value` for the tool layer to format.

use super::*;
use crate::protocol::FlatSymbol;

impl LspClient {
    /// Go to definition; returns the first resolved location.
    pub async fn goto_definition(
        &mut self,
        uri: &str,
        position: Position,
    ) -> Result<Option<Location>, LspError> {
        let result = self.position_request("textDocument/definition", uri, position).await?;
        Ok(parse_locations(&result).into_iter().next())
    }

    /// Go to the type definition of the symbol at a position.
    pub async fn goto_type_definition(
        &mut self,
        uri: &str,
        position: Position,
    ) -> Result<Vec<Location>, LspError> {
        let result = self.position_request("textDocument/typeDefinition", uri, position).await?;
        Ok(parse_locations(&result))
    }

    /// Go to the declaration of the symbol at a position.
    pub async fn goto_declaration(
        &mut self,
        uri: &str,
        position: Position,
    ) -> Result<Vec<Location>, LspError> {
        let result = self.position_request("textDocument/declaration", uri, position).await?;
        Ok(parse_locations(&result))
    }

    /// Find implementations of the symbol at a position.
    pub async fn goto_implementation(
        &mut self,
        uri: &str,
        position: Position,
    ) -> Result<Vec<Location>, LspError> {
        let result = self.position_request("textDocument/implementation", uri, position).await?;
        Ok(parse_locations(&result))
    }

    /// Find all references to a symbol.
    pub async fn find_references(
        &mut self,
        uri: &str,
        position: Position,
    ) -> Result<Vec<Location>, LspError> {
        let result = self
            .send_request(
                "textDocument/references",
                serde_json::json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": position.line, "character": position.character },
                    "context": { "includeDeclaration": false }
                }),
            )
            .await?;
        Ok(parse_locations(&result))
    }

    /// List the symbols in a document (flattened; handles both server shapes).
    pub async fn document_symbols(&mut self, uri: &str) -> Result<Vec<FlatSymbol>, LspError> {
        let result = self
            .send_request(
                "textDocument/documentSymbol",
                serde_json::json!({ "textDocument": { "uri": uri } }),
            )
            .await?;
        let mut symbols = Vec::new();
        flatten_symbols(&result, uri, None, &mut symbols);
        Ok(symbols)
    }

    /// Search for symbols across the workspace by name.
    pub async fn workspace_symbol(&mut self, query: &str) -> Result<Vec<FlatSymbol>, LspError> {
        let result =
            self.send_request("workspace/symbol", serde_json::json!({ "query": query })).await?;
        let mut symbols = Vec::new();
        flatten_symbols(&result, "", None, &mut symbols);
        Ok(symbols)
    }

    /// Get hover information for a symbol at a position.
    pub async fn hover(
        &mut self,
        uri: &str,
        position: Position,
    ) -> Result<Option<MarkupContent>, LspError> {
        let result = self.position_request("textDocument/hover", uri, position).await?;
        if let Value::Object(obj) = &result {
            if let Some(contents) = obj.get("contents") {
                // `contents` may be MarkupContent, a MarkedString, or an array of them.
                if let Ok(markup) = serde_json::from_value::<MarkupContent>(contents.clone()) {
                    return Ok(Some(markup));
                }
                if let Some(text) = hover_plain_text(contents) {
                    return Ok(Some(MarkupContent { kind: "plaintext".to_string(), value: text }));
                }
            }
        }
        Ok(None)
    }

    /// Get signature help at a call-site position (raw result).
    pub async fn signature_help(
        &mut self,
        uri: &str,
        position: Position,
    ) -> Result<Value, LspError> {
        self.position_request("textDocument/signatureHelp", uri, position).await
    }

    /// Prepare a call hierarchy at a position; returns raw `CallHierarchyItem`s.
    pub async fn prepare_call_hierarchy(
        &mut self,
        uri: &str,
        position: Position,
    ) -> Result<Vec<Value>, LspError> {
        let result =
            self.position_request("textDocument/prepareCallHierarchy", uri, position).await?;
        Ok(as_array(result))
    }

    /// Fetch incoming calls for a prepared call-hierarchy item.
    pub async fn incoming_calls(&mut self, item: Value) -> Result<Vec<Value>, LspError> {
        let result = self
            .send_request("callHierarchy/incomingCalls", serde_json::json!({ "item": item }))
            .await?;
        Ok(as_array(result))
    }

    /// Fetch outgoing calls for a prepared call-hierarchy item.
    pub async fn outgoing_calls(&mut self, item: Value) -> Result<Vec<Value>, LspError> {
        let result = self
            .send_request("callHierarchy/outgoingCalls", serde_json::json!({ "item": item }))
            .await?;
        Ok(as_array(result))
    }

    /// List code actions (quick fixes/refactors) for a range (raw results).
    pub async fn code_action(
        &mut self,
        uri: &str,
        range: crate::protocol::Range,
    ) -> Result<Vec<Value>, LspError> {
        // The diagnostics overlapping `range`, not an empty list. The spec is explicit that this is
        // how a server learns "which errors are currently presented to the user for the given
        // range", and a quickfix is offered *for a diagnostic* — so a client that sends none is
        // asking for refactors and opting out of every fix. We already hold these; sending them
        // costs a filter.
        let overlapping = self.diagnostics_overlapping(uri, &range);
        let result = self
            .send_request(
                "textDocument/codeAction",
                serde_json::json!({
                    "textDocument": { "uri": uri },
                    "range": range,
                    "context": { "diagnostics": overlapping }
                }),
            )
            .await?;
        Ok(as_array(result))
    }

    /// This document's cached diagnostics whose range overlaps `range`.
    ///
    /// Line granularity, deliberately: a caller points at a line, and a diagnostic that starts
    /// mid-line would be missed by a strict character comparison against a zero-width position.
    fn diagnostics_overlapping(
        &self,
        uri: &str,
        range: &crate::protocol::Range,
    ) -> Vec<serde_json::Value> {
        let Ok(state) = self.shared.inner.lock() else { return Vec::new() };
        state
            .diagnostics
            .get(uri)
            .map(|diagnostics| {
                diagnostics
                    .iter()
                    .filter(|diagnostic| {
                        diagnostic.range.start.line <= range.end.line
                            && diagnostic.range.end.line >= range.start.line
                    })
                    .filter_map(|diagnostic| serde_json::to_value(diagnostic).ok())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Compute a rename edit (raw `WorkspaceEdit`); the caller previews or applies it.
    pub async fn rename(
        &mut self,
        uri: &str,
        position: Position,
        new_name: &str,
    ) -> Result<Value, LspError> {
        self.send_request(
            "textDocument/rename",
            serde_json::json!({
                "textDocument": { "uri": uri },
                "position": { "line": position.line, "character": position.character },
                "newName": new_name
            }),
        )
        .await
    }

    /// Shared `{ textDocument, position }` request.
    async fn position_request(
        &mut self,
        method: &str,
        uri: &str,
        position: Position,
    ) -> Result<Value, LspError> {
        self.send_request(
            method,
            serde_json::json!({
                "textDocument": { "uri": uri },
                "position": { "line": position.line, "character": position.character }
            }),
        )
        .await
    }
}

/// Coerce a result into an array of values (single object → one-element vec).
fn as_array(result: Value) -> Vec<Value> {
    match result {
        Value::Array(items) => items,
        Value::Null => Vec::new(),
        other => vec![other],
    }
}

/// Parse a definition-style result into `Location`s, accepting `Location`, `Location[]`, or
/// `LocationLink[]` (target-based, as rust-analyzer/tsserver return).
fn parse_locations(result: &Value) -> Vec<Location> {
    match result {
        Value::Array(items) => items.iter().filter_map(parse_location).collect(),
        Value::Object(_) => parse_location(result).into_iter().collect(),
        _ => Vec::new(),
    }
}

/// Parse one `Location` or `LocationLink` leniently.
fn parse_location(value: &Value) -> Option<Location> {
    if let Ok(location) = serde_json::from_value::<Location>(value.clone()) {
        return Some(location);
    }
    // LocationLink: `targetUri` + `targetSelectionRange`/`targetRange`.
    let uri = value.get("targetUri")?.as_str()?.to_string();
    let range = value
        .get("targetSelectionRange")
        .or_else(|| value.get("targetRange"))
        .and_then(|r| serde_json::from_value::<crate::protocol::Range>(r.clone()).ok())?;
    Some(Location { uri, range })
}

/// Flatten a `documentSymbol`/`workspace/symbol` result into [`FlatSymbol`]s, handling both
/// `SymbolInformation` (flat, has `location`) and `DocumentSymbol` (nested, has `selectionRange` +
/// `children`).
fn flatten_symbols(
    value: &Value,
    default_uri: &str,
    container: Option<&str>,
    out: &mut Vec<FlatSymbol>,
) {
    let Value::Array(items) = value else { return };
    for item in items {
        let name = item.get("name").and_then(Value::as_str).unwrap_or("").to_string();
        let kind = item.get("kind").and_then(Value::as_u64).unwrap_or(0) as u32;

        if let Some(location) = item.get("location") {
            // SymbolInformation / WorkspaceSymbol: `location.range` is the full extent, and there
            // is no separate selection range.
            let uri =
                location.get("uri").and_then(Value::as_str).unwrap_or(default_uri).to_string();
            let (line, character) = range_start(location.get("range"));
            let (start_line, end_line) = range_span(location.get("range"));
            let container = item
                .get("containerName")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| container.map(str::to_string));
            out.push(FlatSymbol {
                name,
                kind,
                uri,
                line,
                character,
                start_line,
                end_line,
                container,
            });
        } else if item.get("selectionRange").is_some() || item.get("range").is_some() {
            // DocumentSymbol (hierarchical): `selectionRange` names it, `range` is its body.
            let (line, character) =
                range_start(item.get("selectionRange").or_else(|| item.get("range")));
            let (start_line, end_line) =
                range_span(item.get("range").or_else(|| item.get("selectionRange")));
            out.push(FlatSymbol {
                name: name.clone(),
                kind,
                uri: default_uri.to_string(),
                line,
                character,
                start_line,
                end_line,
                container: container.map(str::to_string),
            });
            if let Some(children) = item.get("children") {
                flatten_symbols(children, default_uri, Some(&name), out);
            }
        }
    }
}

/// Extract a range's 0-based `(line, character)` start, defaulting to `(0, 0)`.
fn range_start(range: Option<&Value>) -> (u32, u32) {
    let start = range.and_then(|r| r.get("start"));
    let line = start.and_then(|s| s.get("line")).and_then(Value::as_u64).unwrap_or(0) as u32;
    let character =
        start.and_then(|s| s.get("character")).and_then(Value::as_u64).unwrap_or(0) as u32;
    (line, character)
}

/// Extract a range's 0-based `(start_line, end_line)`, defaulting the end to the start.
fn range_span(range: Option<&Value>) -> (u32, u32) {
    let line_of = |key: &str| {
        range
            .and_then(|r| r.get(key))
            .and_then(|p| p.get("line"))
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32
    };
    let start = line_of("start");
    (start, line_of("end").max(start))
}

/// Best-effort plain text from a hover `contents` that is a `MarkedString` or an array of them.
fn hover_plain_text(contents: &Value) -> Option<String> {
    match contents {
        Value::String(s) => Some(s.clone()),
        Value::Object(obj) => obj.get("value").and_then(Value::as_str).map(str::to_string),
        Value::Array(items) => {
            let joined = items.iter().filter_map(hover_plain_text).collect::<Vec<_>>().join("\n");
            (!joined.is_empty()).then_some(joined)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_locations_accepts_location_and_location_link() {
        let plain = serde_json::json!([{
            "uri": "file:///a.rs",
            "range": { "start": { "line": 3, "character": 2 }, "end": { "line": 3, "character": 8 } }
        }]);
        let link = serde_json::json!([{
            "targetUri": "file:///b.rs",
            "targetSelectionRange": {
                "start": { "line": 10, "character": 4 }, "end": { "line": 10, "character": 9 }
            },
            "targetRange": {
                "start": { "line": 9, "character": 0 }, "end": { "line": 12, "character": 1 }
            }
        }]);
        assert_eq!(parse_locations(&plain)[0].uri, "file:///a.rs");
        let linked = parse_locations(&link);
        assert_eq!(linked[0].uri, "file:///b.rs");
        assert_eq!(linked[0].range.start.line, 10);
    }

    #[test]
    fn flatten_handles_both_symbol_shapes() {
        // SymbolInformation
        let flat = serde_json::json!([{
            "name": "main", "kind": 12,
            "location": { "uri": "file:///m.rs",
                "range": { "start": { "line": 1, "character": 0 }, "end": { "line": 1, "character": 4 } } },
            "containerName": "app"
        }]);
        let mut out = Vec::new();
        flatten_symbols(&flat, "file:///m.rs", None, &mut out);
        assert_eq!(out[0].name, "main");
        assert_eq!(out[0].line, 1);
        assert_eq!(out[0].container.as_deref(), Some("app"));

        // DocumentSymbol (nested), children inherit the parent as container.
        let nested = serde_json::json!([{
            "name": "Foo", "kind": 23,
            "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 9, "character": 0 } },
            "selectionRange": { "start": { "line": 0, "character": 7 }, "end": { "line": 0, "character": 10 } },
            "children": [{
                "name": "bar", "kind": 6,
                "range": { "start": { "line": 2, "character": 4 }, "end": { "line": 4, "character": 5 } },
                "selectionRange": { "start": { "line": 2, "character": 7 }, "end": { "line": 2, "character": 10 } }
            }]
        }]);
        let mut out = Vec::new();
        flatten_symbols(&nested, "file:///t.rs", None, &mut out);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].name, "Foo");
        assert_eq!(out[0].character, 7);
        assert_eq!(out[1].name, "bar");
        assert_eq!(out[1].container.as_deref(), Some("Foo"));
    }
}
