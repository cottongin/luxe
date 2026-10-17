//! Formatting — locations, symbols and previews as the lines a model reads.

use super::*;

/// Most locations listed before the tail is summarized.
pub(super) const MAX_LOCATIONS: usize = 60;

/// Most symbols listed before the tail is summarized.
pub(super) const MAX_SYMBOLS: usize = 60;

pub(super) fn locations_reply(
    result: Result<Vec<Location>, crate::client::LspError>,
    label: &str,
    client: &LspClient,
) -> ToolResultData {
    match result {
        Ok(locs) if locs.is_empty() => {
            ToolResultData::text(nothing_found(&format!("No {label}s found"), client))
        }
        Ok(locs) => {
            let mut out = format!("{} {label}(s):\n", locs.len());
            // `find_references` on a hot symbol answers with hundreds of locations, and the count
            // in the header is the part that decides what to do next — a caller reading every entry
            // is already better served by `impact`.
            for loc in locs.iter().take(MAX_LOCATIONS) {
                out.push_str(&format!("  {}\n", format_location(loc)));
            }
            if locs.len() > MAX_LOCATIONS {
                out.push_str(&format!(
                    "… and {} more. Use `impact` for a ranked view of what a change here reaches.\n",
                    locs.len() - MAX_LOCATIONS
                ));
            }
            ToolResultData::text(out)
        }
        Err(e) => ToolResultData::error(format!("Error: {label} failed: {e}")),
    }
}

pub(super) fn format_location(loc: &Location) -> String {
    format!(
        "{}:{}:{}",
        crate::uri::short_display(&loc.uri),
        loc.range.start.line + 1,
        loc.range.start.character + 1
    )
}

pub(super) fn format_symbols(symbols: &[FlatSymbol], with_uri: bool) -> String {
    if symbols.is_empty() {
        return String::from("No symbols found");
    }
    let mut out = format!("{} symbol(s):\n", symbols.len());
    // A broad `workspace_symbol` query matches most of a codebase; the header carries the count and
    // a narrower query beats scrolling the tail.
    for sym in symbols.iter().take(MAX_SYMBOLS) {
        let kind = symbol_kind_name(sym.kind);
        let loc = if with_uri {
            format!("{}:{}", crate::uri::short_display(&sym.uri), sym.line + 1)
        } else {
            format!("{}", sym.line + 1)
        };
        out.push_str(&format!("  {} ({kind}) {loc}", sym.name));
        if let Some(container) = &sym.container {
            out.push_str(&format!(" in {container}"));
        }
        out.push('\n');
    }
    if symbols.len() > MAX_SYMBOLS {
        out.push_str(&format!(
            "… and {} more. Narrow the query for the rest.\n",
            symbols.len() - MAX_SYMBOLS
        ));
    }
    out
}

pub(super) fn format_signature_help(value: &Value) -> String {
    let signatures = value.get("signatures").and_then(Value::as_array);
    let Some(signatures) = signatures.filter(|s| !s.is_empty()) else {
        return "No signature help".to_string();
    };
    let active = value.get("activeSignature").and_then(Value::as_u64).unwrap_or(0) as usize;
    let chosen = signatures.get(active).or_else(|| signatures.first());
    chosen
        .and_then(|s| s.get("label"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| "No signature help".to_string())
}

pub(super) fn format_code_actions(actions: &[Value]) -> String {
    if actions.is_empty() {
        return "No code actions available".to_string();
    }
    let mut out = format!("{} code action(s):\n", actions.len());
    for action in actions {
        let title = action.get("title").and_then(Value::as_str).unwrap_or("(command)");
        let kind = action
            .get("kind")
            .and_then(Value::as_str)
            .map(|k| format!(" [{k}]"))
            .unwrap_or_default();
        out.push_str(&format!("  {title}{kind}\n"));
    }
    out
}

pub(super) fn format_rename_preview(edit: &Value) -> String {
    // WorkspaceEdit: `changes` (uri → TextEdit[]) or `documentChanges` (array of {textDocument, edits}).
    let mut files: Vec<(String, usize)> = Vec::new();
    if let Some(changes) = edit.get("changes").and_then(Value::as_object) {
        for (uri, edits) in changes {
            files.push((uri.clone(), edits.as_array().map_or(0, Vec::len)));
        }
    }
    if let Some(document_changes) = edit.get("documentChanges").and_then(Value::as_array) {
        for change in document_changes {
            if let Some(uri) =
                change.get("textDocument").and_then(|d| d.get("uri")).and_then(Value::as_str)
            {
                let count = change.get("edits").and_then(Value::as_array).map_or(0, Vec::len);
                files.push((uri.to_string(), count));
            }
        }
    }
    if files.is_empty() {
        return "Rename produced no edits".to_string();
    }
    let total: usize = files.iter().map(|(_, n)| n).sum();
    files.sort();
    let mut out = format!("Rename preview — {total} edit(s) across {} file(s):\n", files.len());
    for (uri, count) in files {
        out.push_str(&format!("  {} ({count} edit(s))\n", crate::uri::short_display(&uri)));
    }
    out.push_str("(preview only — apply the change with the edit tool)");
    out
}

pub(super) fn format_hierarchy_node(node: &Value) -> String {
    let name = node.get("name").and_then(Value::as_str).unwrap_or("?");
    let uri = node.get("uri").and_then(Value::as_str).unwrap_or("");
    let line = node
        .get("selectionRange")
        .or_else(|| node.get("range"))
        .and_then(|r| r.get("start"))
        .and_then(|s| s.get("line"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    format!("{name} — {}:{}", crate::uri::short_display(uri), line + 1)
}

/// An empty answer, plus whatever the server said about why it cannot answer.
///
/// "No definition found" and "this server cannot resolve anything" are the same sentence, and the
/// difference matters entirely: the first is information about the code, the second is a broken
/// setup, and a server that cannot do its job usually says so. A `typescript-language-server` with
/// no `typescript` package to drive answers every request with nothing; without its notices the
/// reader is told their symbol has no definition.
pub(super) fn nothing_found(answer: &str, client: &LspClient) -> String {
    let notices = client.server_notices();
    if notices.is_empty() {
        return answer.to_string();
    }
    let mut out = answer.to_string();
    for notice in notices {
        out.push_str(&format!("\n  note: language server reported {notice}"));
    }
    out
}
