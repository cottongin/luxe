//! `read_symbol` — one symbol's verbatim source, with what a method needs around it.

use super::*;

/// Max lines returned by `read_symbol`.
const MAX_BODY_LINES: usize = 400;

/// Most field-assignment lines carried alongside a method body.
const MAX_CONTEXT_LINES: usize = 8;

/// Returns the verbatim source of one named symbol in a file.
pub struct ReadSymbolTool {
    service: Service,
    tracker: Option<Arc<luxe_core::read_tracker::ReadTracker>>,
}

impl ReadSymbolTool {
    /// Create the tool over the shared warm service.
    pub fn new(service: Service) -> Self {
        Self { service, tracker: None }
    }

    /// Attach the shared read tracker so a `read_symbol` counts as having seen the file for the
    /// `edit`/`write` guards (it returns verbatim source, so a subsequent edit is grounded).
    pub fn with_tracker(mut self, tracker: Arc<luxe_core::read_tracker::ReadTracker>) -> Self {
        self.tracker = Some(tracker);
        self
    }
}

#[async_trait]
impl Tool for ReadSymbolTool {
    fn name(&self) -> &str {
        "read_symbol"
    }

    fn argument_aliases(&self) -> &'static [&'static str] {
        super::FILE_ALIASES
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "read_symbol".to_string(),
            description: "Read the verbatim source of one named symbol (function/type/etc.) in a \
                          file — a cheap alternative to reading the whole file. Resolves the symbol \
                          via a tree-sitter outline (offline) or the language server."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path to the file (relative to cwd)" },
                    "symbol": { "type": "string", "description": "Exact (or substring) symbol name to read" }
                },
                "required": ["path", "symbol"]
            }),
        }
    }

    async fn run(&self, args: serde_json::Value, cx: &ToolContext) -> ToolResultData {
        let Some(file) = super::file_arg(&args) else {
            return ToolResultData::error("Error: 'path' is required");
        };
        let Some(symbol) = args.get("symbol").and_then(|v| v.as_str()) else {
            return ToolResultData::error("Error: 'symbol' is required");
        };
        let outline = match build_outline(&self.service, cx, file, false).await {
            Ok(outline) => outline,
            Err(e) => return ToolResultData::error(format!("Error: {e}")),
        };
        let Some(found) = pick_symbol(&outline.symbols, symbol) else {
            return ToolResultData::error(format!("Error: symbol '{symbol}' not found in {file}"));
        };
        let Some(content) = outline.content else {
            return ToolResultData::error(format!("Error: could not read {file}"));
        };

        let lines: Vec<&str> = content.lines().collect();
        let start = found.start_line as usize;
        if start >= lines.len() {
            return ToolResultData::error(format!(
                "Error: symbol '{symbol}' range is out of bounds"
            ));
        }
        let end = (found.end_line as usize).min(lines.len().saturating_sub(1));
        let shown = end.saturating_sub(start) + 1;
        let body =
            lines[start..=end].iter().take(MAX_BODY_LINES).copied().collect::<Vec<_>>().join("\n");
        let truncated = if shown > MAX_BODY_LINES {
            format!("\n… ({} more lines)", shown - MAX_BODY_LINES)
        } else {
            String::new()
        };
        // Reading a symbol's verbatim body counts as having seen the file for the mutation guards.
        record_read(self.tracker.as_ref(), cx, file);
        let context = enclosing_context(found, &outline.symbols, &lines, &body);
        ToolResultData::text(format!(
            "{} ({}) — {}:{}-{}\n\n{body}{truncated}{context}",
            found.name,
            symbol_kind_name(found.kind),
            file,
            start + 1,
            end + 1,
        ))
    }
}

/// What a method's body needs to be *interpretable*: the class it lives on, and where the fields
/// it touches were assigned.
///
/// A symbol-scoped read is cheaper than the whole file right up until the answer lives just
/// outside the window. Measured failure: `get_connection` returns
/// `self.connection_repo.get_by_user_id(...)`, and a model shown only those five lines answered
/// with the enclosing *service* rather than the repository class assigned in `__init__` — twice,
/// on two different model families. Carrying the field assignments closes that gap without
/// giving up the cheapness.
pub(super) fn enclosing_context(
    found: &FlatSymbol,
    symbols: &[FlatSymbol],
    lines: &[&str],
    body: &str,
) -> String {
    let Some(container) = found.container.as_deref().filter(|name| !name.is_empty()) else {
        return String::new();
    };

    // Fields the body actually reaches for: `self.x` / `this.x`.
    let referenced: std::collections::BTreeSet<&str> = body
        .match_indices("self.")
        .chain(body.match_indices("this."))
        .filter_map(|(at, prefix)| {
            let rest = &body[at + prefix.len()..];
            let end = rest.find(|c: char| !c.is_alphanumeric() && c != '_').unwrap_or(rest.len());
            (end > 0).then(|| &rest[..end])
        })
        .collect();
    if referenced.is_empty() {
        return format!("\n\n(defined on `{container}`)");
    }

    // Where those fields are assigned, anywhere inside the enclosing container.
    let scope = symbols.iter().find(|s| s.name == container);
    let (from, to) = match scope {
        Some(scope) => (scope.start_line as usize, (scope.end_line as usize).min(lines.len())),
        None => (0, lines.len()),
    };
    let assignments: Vec<String> = lines
        .get(from..to)
        .unwrap_or_default()
        .iter()
        .enumerate()
        .filter(|(_, line)| {
            referenced.iter().any(|field| {
                line.contains(&format!("self.{field} ="))
                    || line.contains(&format!("this.{field} ="))
            })
        })
        .take(MAX_CONTEXT_LINES)
        .map(|(offset, line)| format!("  {}: {}", from + offset + 1, line.trim()))
        .collect();

    if assignments.is_empty() {
        format!("\n\n(defined on `{container}`)")
    } else {
        format!(
            "\n\nDefined on `{container}`; the fields this uses are assigned at:\n{}",
            assignments.join("\n")
        )
    }
}

/// The symbol `name` asks for: an exact name first, else the first whose name contains it.
pub(super) fn pick_symbol<'a>(symbols: &'a [FlatSymbol], name: &str) -> Option<&'a FlatSymbol> {
    symbols
        .iter()
        .find(|s| s.name == name)
        .or_else(|| symbols.iter().find(|s| s.name.contains(name)))
}
