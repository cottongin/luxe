//! `symbol_search` — the workspace's symbols that match a name, best first.

use super::*;

/// Ranked project symbol search. Prefers an **offline BM25 index** (all tree-sitter languages);
/// falls back to the language server's `workspace/symbol` when the index is empty.
pub struct SymbolSearchTool {
    service: Service,
    index: Arc<crate::symbol_index::SymbolIndexCache>,
    graph: Option<Arc<crate::graph::GraphService>>,
}

impl SymbolSearchTool {
    /// Create the tool over the shared warm service.
    pub fn new(service: Service) -> Self {
        Self { service, index: Arc::default(), graph: None }
    }

    /// Share `index` with whatever invalidates it when a file changes.
    pub fn with_index(mut self, index: Arc<crate::symbol_index::SymbolIndexCache>) -> Self {
        self.index = index;
        self
    }

    /// Attach the warm code graph so results are ranked with a degree-centrality prior.
    pub fn with_graph(mut self, graph: Arc<crate::graph::GraphService>) -> Self {
        self.graph = Some(graph);
        self
    }
}

#[async_trait]
impl Tool for SymbolSearchTool {
    fn name(&self) -> &str {
        "symbol_search"
    }

    fn argument_aliases(&self) -> &'static [&'static str] {
        super::FILE_ALIASES
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "symbol_search".to_string(),
            description: "Find symbols across the project by name, ranked exact → prefix → BM25. \
                          Uses a fast offline tree-sitter index (all supported languages); falls \
                          back to the language server when the index is empty. First step of the \
                          discovery funnel; follow up with module_report / read_symbol."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Symbol name to search for" },
                    "path": { "type": "string", "description": "A file in the target workspace (only needed for the language-server fallback)" },
                    "refresh": { "type": "boolean", "description": "Rebuild the offline index before searching" }
                },
                "required": ["query"]
            }),
        }
    }

    async fn run(&self, args: serde_json::Value, cx: &ToolContext) -> ToolResultData {
        let Some(query) = args.get("query").and_then(|v| v.as_str()) else {
            return ToolResultData::error("Error: 'query' is required");
        };
        let refresh = args.get("refresh").and_then(serde_json::Value::as_bool).unwrap_or(false);

        // Preferred path: the offline BM25 index over tree-sitter symbols.
        match self.index.get(&cx.cwd, refresh).await {
            Ok(index) if !index.is_empty() => {
                // Fold in a graph-centrality prior when the code graph is available, so central
                // symbols (many callers/dependents) outrank equally-matched peripheral ones.
                // the prior only reorders, so a graph that cannot be built costs the ranking alone
                let graph = match &self.graph {
                    Some(service) => service.graph(refresh).await.ok(),
                    None => None,
                };
                let hits = match &graph {
                    Some(graph) => index.search_with_prior(query, MAX_SYMBOLS, |sym| {
                        centrality_prior(graph.as_ref(), sym)
                    }),
                    None => index.search(query, MAX_SYMBOLS),
                };
                let capped = index.capped_note();
                if hits.is_empty() {
                    return ToolResultData::text(format!(
                        "No symbols matching '{query}'\n{capped}"
                    ));
                }
                let (kept, looser_dropped) = tighten(hits, query);
                let mut out = format!("{} match(es) for '{query}':\n", kept.len());
                for hit in kept {
                    out.push_str(&format!("  {}\n", crate::symbol_index::format_hit(hit)));
                }
                if looser_dropped {
                    out.push_str(
                        "  Looser matches left out. Search part of a name to see more, or \
                         `grep` for text.\n",
                    );
                }
                out.push_str(&capped);
                return ToolResultData::text(out);
            }
            Ok(_) => {} // empty index → fall back to the language server
            Err(e) => return ToolResultData::error(format!("Error: {e}")),
        }

        // Fallback: language-server `workspace/symbol` (needs a `file` to anchor).
        let Some(file) = super::file_arg(&args) else {
            return ToolResultData::error(
                "Error: no indexable files found; pass `path` for a language-server search",
            );
        };
        let full = cx.cwd.join(file);
        let client = { self.service.client_for_path(&full).await };
        let Some(client) = client else {
            return ToolResultData::error("Error: no language server for this file");
        };
        let mut symbols = match client.lock().await.workspace_symbol(query).await {
            Ok(symbols) => symbols,
            Err(e) => return ToolResultData::error(format!("Error: symbol_search failed: {e}")),
        };
        if symbols.is_empty() {
            return ToolResultData::text(format!("No symbols matching '{query}'"));
        }
        symbols.sort_by_key(|s| (rank(&s.name, query), s.name.clone()));
        let mut out = format!("{} match(es) for '{query}':\n", symbols.len().min(MAX_SYMBOLS));
        for sym in symbols.iter().take(MAX_SYMBOLS) {
            out.push_str(&format!(
                "  {} ({}) {}:{}\n",
                sym.name,
                symbol_kind_name(sym.kind),
                crate::uri::short_display(&sym.uri),
                sym.line + 1,
            ));
        }
        ToolResultData::text(out)
    }
}

/// Most looser matches a search returns when nothing matched the name itself.
pub(super) const MAX_LOOSE_SYMBOLS: usize = 20;

/// The hits worth sending, and whether looser ones were left out.
///
/// The index ranks exact names, then prefixes, then token matches, and when a name matched there is
/// nothing to gain from the rest: `set_cancelled` came back with 59 lookalikes, `set` methods and
/// every `Cancelled` variant, 1,200 tokens where `rg` answers in one line. So an exact or prefix
/// match is the whole answer, and token matches stand alone only when nothing better exists, and
/// then at most [`MAX_LOOSE_SYMBOLS`] of them.
pub(super) fn tighten<'a>(
    hits: Vec<&'a crate::symbol_index::IndexedSymbol>,
    query: &str,
) -> (Vec<&'a crate::symbol_index::IndexedSymbol>, bool) {
    let query = query.to_lowercase();
    let named =
        |hit: &&crate::symbol_index::IndexedSymbol| hit.name.to_lowercase().starts_with(&query);
    let total = hits.len();
    let kept: Vec<_> = if hits.iter().any(named) {
        hits.into_iter().filter(named).collect()
    } else {
        hits.into_iter().take(MAX_LOOSE_SYMBOLS).collect()
    };
    let dropped = kept.len() < total;
    (kept, dropped)
}

/// Degree-centrality prior for a symbol: 1.0 for an isolated/unknown symbol, rising toward 1.5 for
/// a hub (many callers/dependents). Boosts central symbols within their match tier without
/// overriding exact/prefix ranking.
fn centrality_prior(
    graph: &crate::graph::InMemoryGraph,
    sym: &crate::symbol_index::IndexedSymbol,
) -> f64 {
    use crate::graph::{GraphStore, QualifiedName};
    let qn = QualifiedName::symbol(&sym.path, sym.container.as_deref(), &sym.name);
    let degree = graph.degree(&qn) as f64;
    1.0 + 0.5 * (degree / (degree + 5.0))
}

/// Rank a symbol name against a query: exact (0) → prefix (1) → substring (2) → other (3).
pub(super) fn rank(name: &str, query: &str) -> u8 {
    if name == query {
        0
    } else if name.starts_with(query) {
        1
    } else if name.contains(query) {
        2
    } else {
        3
    }
}
