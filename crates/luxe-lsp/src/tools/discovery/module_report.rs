//! `module_report` — a file's outline, with who uses each symbol.

use super::*;

/// Returns a navigable outline of a file's symbols.
pub struct ModuleReportTool {
    service: Service,
    tracker: Option<Arc<luxe_core::read_tracker::ReadTracker>>,
    graph: Option<Arc<crate::graph::GraphService>>,
}

impl ModuleReportTool {
    /// Create the tool over the shared warm service.
    pub fn new(service: Service) -> Self {
        Self { service, tracker: None, graph: None }
    }

    /// Attach the shared read tracker so a `module_report` counts as having seen the file for the
    /// `edit`/`write` guards.
    pub fn with_tracker(mut self, tracker: Arc<luxe_core::read_tracker::ReadTracker>) -> Self {
        self.tracker = Some(tracker);
        self
    }

    /// Attach the warm code graph so `module_report` can report a real blast radius on request.
    pub fn with_graph(mut self, graph: Arc<crate::graph::GraphService>) -> Self {
        self.graph = Some(graph);
        self
    }
}

#[async_trait]
impl Tool for ModuleReportTool {
    fn name(&self) -> &str {
        "module_report"
    }

    fn argument_aliases(&self) -> &'static [&'static str] {
        super::FILE_ALIASES
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "module_report".to_string(),
            description:
                "Outline a file's symbols (name, kind, line range, nesting) via tree-sitter \
                          or the language server — a token-efficient substitute for reading the \
                          whole file. Follow up with read_symbol to read one symbol's body."
                    .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path to the file (relative to cwd)" },
                    "usages": { "type": "boolean", "description": "Also count references (who-uses-this) for top-level symbols (needs a language server; slower)" },
                    "blastRadius": { "type": "boolean", "description": "Also report the blast radius (transitive callers/dependents + covering tests) from the offline code graph" }
                },
                "required": ["path"]
            }),
        }
    }

    async fn run(&self, args: serde_json::Value, cx: &ToolContext) -> ToolResultData {
        let Some(file) = super::file_arg(&args) else {
            return ToolResultData::error("Error: 'path' is required");
        };
        let want_usages = args.get("usages").and_then(serde_json::Value::as_bool).unwrap_or(false);
        let want_blast = luxe_core::tool::arg(&args, &["blastRadius", "blast_radius"])
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let outline = match build_outline(&self.service, cx, file, want_usages).await {
            Ok(outline) => outline,
            Err(e) => return ToolResultData::error(format!("Error: {e}")),
        };
        // Outlining the file reads it, which counts as having seen it for the mutation guards.
        record_read(self.tracker.as_ref(), cx, file);
        if outline.symbols.is_empty() {
            return ToolResultData::text(format!("{file}: no symbols"));
        }

        // who-uses-this: bounded references from each top-level symbol's name position.
        let usages = match (want_usages, &outline.client) {
            (true, Some(client)) => reference_counts(client, &outline.uri, &outline.symbols).await,
            _ => Vec::new(),
        };

        let mut out = format!(
            "{file} — {} symbol(s) [{}]:\n",
            outline.symbols.len().min(MAX_SYMBOLS),
            outline.source
        );
        for (index, sym) in outline.symbols.iter().take(MAX_SYMBOLS).enumerate() {
            let indent = if sym.container.is_some() { "  " } else { "" };
            let uses = usages
                .get(index)
                .copied()
                .flatten()
                .map(|count| format!("  · used by {count}"))
                .unwrap_or_default();
            out.push_str(&format!(
                "{indent}[{}] {}  L{}-{}{uses}\n",
                symbol_kind_name(sym.kind),
                sym.name,
                sym.start_line + 1,
                sym.end_line + 1,
            ));
        }
        if outline.symbols.len() > MAX_SYMBOLS {
            out.push_str(&format!("… {} more\n", outline.symbols.len() - MAX_SYMBOLS));
        }
        if want_usages && outline.client.is_none() {
            out.push_str("(who-uses-this needs a language server; none available)\n");
        }
        if want_blast {
            append_blast_radius(&mut out, self.graph.as_ref(), file).await;
        }
        ToolResultData::text(out)
    }
}

/// Reference counts per shown symbol (aligned by index): `Some(n)` for a queried top-level symbol,
/// `None` for members or beyond the query budget. One `references` request per top-level symbol,
/// capped so a big file can't fan out unboundedly.
async fn reference_counts(
    client: &Arc<Mutex<LspClient>>,
    uri: &str,
    symbols: &[FlatSymbol],
) -> Vec<Option<usize>> {
    const MAX_QUERIES: usize = 25;
    let mut counts = vec![None; symbols.len().min(MAX_SYMBOLS)];
    let mut queried = 0;
    let mut guard = client.lock().await;
    for (index, sym) in symbols.iter().take(MAX_SYMBOLS).enumerate() {
        if sym.container.is_some() || queried >= MAX_QUERIES {
            continue;
        }
        queried += 1;
        let position = Position { line: sym.line, character: sym.character };
        if let Ok(locations) = guard.find_references(uri, position).await {
            counts[index] = Some(locations.len());
        }
    }
    counts
}

/// Append a compact blast-radius section to a `module_report`, using the warm code graph.
async fn append_blast_radius(
    out: &mut String,
    graph: Option<&Arc<crate::graph::GraphService>>,
    file: &str,
) {
    let Some(service) = graph else {
        out.push_str("(blastRadius needs the code graph; not available)\n");
        return;
    };
    let graph = match service.graph(false).await {
        Ok(graph) => graph,
        Err(error) => return out.push_str(&format!("\n(blastRadius unavailable: {error})\n")),
    };
    let br = crate::graph::blast_radius(&graph, file, crate::graph::DEFAULT_MAX_DEPTH);
    out.push_str(&format!("\nblast radius: {}\n", br.summary_line()));
    for impacted in br.dependents.iter().take(5) {
        out.push_str(&format!(
            "  affected: {}  · {}  [depth {}]\n",
            impacted.file, impacted.reason, impacted.min_depth
        ));
    }
    if !br.untested_seeds.is_empty() {
        let names: Vec<&str> = br.untested_seeds.iter().map(|s| s.base_name()).collect();
        out.push_str(&format!("  untested: {}\n", names.join(", ")));
    }
}
