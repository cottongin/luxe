//! Enrichment — verify heuristic graph edges against the LSP (design-code-graph.md §6, §8.2).
//!
//! Tree-sitter `Calls` edges are name-based heuristics; for the review-critical symbols in a diff
//! we want precision. When a language server is warm, [`verify_callers`] reconciles the graph's
//! heuristic callers of a symbol with the server's exact `textDocument/references`, labeling each
//! caller file as graph-known, LSP-confirmed, or both. It never fabricates: if no server answers,
//! it returns `None` and callers fall back to the heuristic graph.

use super::model::{EdgeKind, QualifiedName};
use super::store::{GraphStore, InMemoryGraph};
use crate::{LspClient, Position, WorkspaceLspClient};
use async_trait::async_trait;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

/// A source of precise "who references this symbol" answers — the LSP in production, mockable in
/// tests. Returns `None` when no server can answer (so enrichment is skipped, not read as "no
/// callers").
#[async_trait]
pub trait ReferenceSource: Send + Sync {
    /// Project-relative files that reference `symbol` (defined in project-relative `file`).
    async fn referencing_files(&self, file: &str, symbol: &str) -> Option<Vec<String>>;
}

/// One caller file's provenance after reconciling the graph with the LSP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallerVerification {
    /// Project-relative caller file.
    pub file: String,
    /// The heuristic graph had this caller.
    pub in_graph: bool,
    /// The LSP confirmed this caller.
    pub in_lsp: bool,
}

impl CallerVerification {
    /// Confirmed by the LSP — a precise, trustworthy caller.
    pub fn verified(&self) -> bool {
        self.in_lsp
    }
    /// Found by the LSP but missing from the heuristic graph (a false negative it recovers).
    pub fn lsp_only(&self) -> bool {
        self.in_lsp && !self.in_graph
    }
    /// In the heuristic graph but not confirmed by the LSP (a possible false positive — kept, not
    /// dropped, since references can be incomplete for macros/dynamic dispatch).
    pub fn unconfirmed(&self) -> bool {
        self.in_graph && !self.in_lsp
    }
}

/// Reconcile the graph's heuristic callers of `seed` with the LSP's exact references.
///
/// Returns the union of caller files, each labeled with its provenance, or `None` when no server
/// answered (callers should then trust the heuristic graph unchanged).
pub async fn verify_callers(
    graph: &InMemoryGraph,
    seed: &QualifiedName,
    source: &dyn ReferenceSource,
) -> Option<Vec<CallerVerification>> {
    let lsp_files = source.referencing_files(seed.file_path(), seed.base_name()).await?;
    let lsp: HashSet<String> = lsp_files.into_iter().collect();

    let graph_files: HashSet<String> = graph
        .in_edges(seed)
        .iter()
        .filter(|edge| matches!(edge.kind, EdgeKind::Calls | EdgeKind::TestCovers))
        .map(|edge| edge.from.file_path().to_string())
        .collect();

    let mut files: Vec<String> = lsp.union(&graph_files).cloned().collect();
    files.sort();
    Some(
        files
            .into_iter()
            .map(|file| CallerVerification {
                in_graph: graph_files.contains(&file),
                in_lsp: lsp.contains(&file),
                file,
            })
            .collect(),
    )
}

/// An LSP-backed [`ReferenceSource`] over the warm workspace client.
///
/// Resolves the symbol's name position offline (tree-sitter), then asks the server for references
/// and maps them back to project-relative files. Degrades to `None` whenever any step is
/// unavailable (no server, file unreadable, symbol not found).
pub struct LspReferenceSource {
    service: Arc<WorkspaceLspClient>,
    root: PathBuf,
}

impl LspReferenceSource {
    /// Create a source over the shared warm service, resolving paths against `root`.
    pub fn new(service: Arc<WorkspaceLspClient>, root: impl Into<PathBuf>) -> Self {
        Self { service, root: root.into() }
    }

    /// Map a `file://` reference URI back to a project-relative path.
    fn relativize(&self, uri: &str) -> Option<String> {
        let path = crate::uri::to_path(uri);
        path.strip_prefix(&self.root).ok().map(luxe_core::paths::display)
    }
}

#[async_trait]
impl ReferenceSource for LspReferenceSource {
    async fn referencing_files(&self, file: &str, symbol: &str) -> Option<Vec<String>> {
        let full = self.root.join(file);
        let source = std::fs::read_to_string(&full).ok()?;
        // The symbol's name position comes from the offline outline (no server needed).
        let outline = crate::syntax::outline(&full, &source)?;
        let target = outline.iter().find(|s| s.name == symbol)?;
        let position = Position { line: target.line, character: target.character };

        let client: Arc<Mutex<LspClient>> = { self.service.client_for_path(&full).await }?;
        let uri = crate::uri::from_path(&full);
        let mut guard = client.lock().await;
        let _ = guard.open_document(&uri, &source).await;
        let locations = guard.find_references(&uri, position).await.ok()?;
        drop(guard);

        let files: HashSet<String> =
            locations.iter().filter_map(|loc| self.relativize(&loc.uri)).collect();
        Some(files.into_iter().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::build::build_graph;

    /// A canned reference source: `Some(files)` for a warm server, `None` for none.
    struct MockRefs(Option<Vec<String>>);

    #[async_trait]
    impl ReferenceSource for MockRefs {
        async fn referencing_files(&self, _file: &str, _symbol: &str) -> Option<Vec<String>> {
            self.0.clone()
        }
    }

    fn graph() -> InMemoryGraph {
        build_graph(vec![
            ("src/auth.rs", "pub fn login() {}\n"),
            ("src/session.rs", "use crate::auth::login;\npub fn start() { login() }\n"),
        ])
    }

    #[tokio::test]
    async fn verify_merges_graph_and_lsp_callers() {
        let graph = graph();
        let seed = QualifiedName::from_raw("src/auth.rs::login");
        // The LSP sees the graph's caller (session.rs) plus one the heuristic missed (api.rs).
        let source = MockRefs(Some(vec!["src/session.rs".into(), "src/api.rs".into()]));
        let result = verify_callers(&graph, &seed, &source).await.expect("server answered");

        let session = result.iter().find(|c| c.file == "src/session.rs").unwrap();
        assert!(session.in_graph && session.verified(), "confirmed caller");

        let api = result.iter().find(|c| c.file == "src/api.rs").unwrap();
        assert!(api.lsp_only(), "LSP recovered a false-negative caller");
    }

    #[tokio::test]
    async fn unconfirmed_heuristic_caller_is_kept_not_dropped() {
        let graph = graph();
        let seed = QualifiedName::from_raw("src/auth.rs::login");
        // LSP returns no references for login (e.g. reached only via a macro) → keep the heuristic.
        let source = MockRefs(Some(vec![]));
        let result = verify_callers(&graph, &seed, &source).await.unwrap();
        let session = result.iter().find(|c| c.file == "src/session.rs").unwrap();
        assert!(session.unconfirmed(), "graph caller retained though LSP didn't confirm");
    }

    #[tokio::test]
    async fn no_server_yields_none() {
        let graph = graph();
        let seed = QualifiedName::from_raw("src/auth.rs::login");
        assert!(verify_callers(&graph, &seed, &MockRefs(None)).await.is_none());
    }
}
