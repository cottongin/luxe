//! Service — the warm code graph and its cached build (design-code-graph.md §4).
//!
//! Mirrors the BM25 `SymbolSearchTool` lifecycle: the graph builds lazily on `spawn_blocking`, is
//! cached as an `Arc<InMemoryGraph>` behind a `Mutex<Option<…>>`, and is shared across tools. The
//! build reads every source file but re-parses only files whose content hash changed since the last
//! run (the rest load from `.luxe/graph/` shards). On a file change the service invalidates the
//! cached graph; the next query rebuilds from the cache, re-parsing just the changed file — so an
//! edit costs one parse, and dependents' resolutions are recomputed for free during the reload.

use super::build::assemble;
use super::cache::{content_hash, GraphCache};
use super::extract::{extract_file, FileGraph};
use super::store::InMemoryGraph;
use crate::symbol_index::source_files;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

/// What a cached build did — for perf assertions and diagnostics.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BuildStats {
    /// Indexed source files.
    pub files: usize,
    /// Files loaded from a valid cache shard (not re-parsed).
    pub cache_hits: usize,
    /// Files re-parsed (new or changed).
    pub cache_misses: usize,
}

/// Build a resolved graph for `root`, reusing `cache` for unchanged files. Blocking (filesystem +
/// parsing); call from `spawn_blocking`.
pub fn build_graph_cached(root: &Path, cache: &GraphCache) -> (InMemoryGraph, BuildStats) {
    let mut fragments: Vec<(String, FileGraph)> = Vec::new();
    let mut manifest = std::collections::HashMap::new();
    let mut stats = BuildStats::default();

    for abs in source_files(root).files {
        let Ok(content) = std::fs::read_to_string(&abs) else { continue };
        // Forward slashes, because the key is also what the model reads back and hands to `read`.
        let rel = luxe_core::paths::display(abs.strip_prefix(root).unwrap_or(&abs));
        let hash = content_hash(&content);

        let fragment = match cache.load(&rel, &hash) {
            Some(fg) => {
                stats.cache_hits += 1;
                fg
            }
            None => {
                let Some(fg) = extract_file(&rel, &content) else { continue };
                cache.store(&rel, &hash, &fg);
                stats.cache_misses += 1;
                fg
            }
        };
        stats.files += 1;
        manifest.insert(rel.clone(), hash);
        fragments.push((rel, fragment));
    }

    cache.store_manifest(&manifest);
    cache.prune(&manifest);
    (assemble(fragments), stats)
}

/// Why there is no graph to answer from.
#[derive(Debug, thiserror::Error)]
pub enum GraphError {
    /// The build panicked or was cancelled.
    #[error("the code graph could not be built: {0}")]
    Build(String),
}

/// The build every service runs: from the shard cache under `root`.
fn build_from_cache(root: &Path) -> InMemoryGraph {
    build_graph_cached(root, &GraphCache::new(root)).0
}

/// A warm, shared code graph for one project root.
pub struct GraphService {
    root: PathBuf,
    graph: Mutex<Option<Arc<InMemoryGraph>>>,
    build: fn(&Path) -> InMemoryGraph,
}

impl GraphService {
    /// A service over `root` (nothing is built until the first query).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into(), graph: Mutex::new(None), build: build_from_cache }
    }

    /// Get (or build) the shared graph. `refresh` forces a full rebuild from the cache.
    ///
    /// A build that fails is an error and is not kept: an empty graph would read as "nothing
    /// depends on this", the one answer a failure must never give.
    pub async fn graph(&self, refresh: bool) -> Result<Arc<InMemoryGraph>, GraphError> {
        let mut guard = self.graph.lock().await;
        if let Some(graph) = guard.as_ref().filter(|_| !refresh) {
            return Ok(Arc::clone(graph));
        }
        let (root, build) = (self.root.clone(), self.build);
        let built = tokio::task::spawn_blocking(move || build(&root))
            .await
            .map_err(|error| GraphError::Build(error.to_string()))?;
        let built = Arc::new(built);
        *guard = Some(Arc::clone(&built));
        Ok(built)
    }

    /// Invalidate the cached graph after a file change; the next `graph()` rebuilds from the cache,
    /// re-parsing only the changed file.
    pub async fn invalidate(&self) {
        *self.graph.lock().await = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::store::GraphStore;

    /// A throwaway project dir with a couple of source files.
    fn scratch(name: &str) -> tempfile::TempDir {
        let scratch = tempfile::Builder::new()
            .prefix(&format!("luxe-graph-svc-{name}-"))
            .tempdir()
            .expect("temp dir");
        let dir = scratch.path();
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/lib.rs"), "pub fn compute() -> i32 { 1 }\n").unwrap();
        std::fs::write(
            dir.join("src/app.rs"),
            "fn run() { compute(); }\n#[cfg(test)]\nmod tests {\n    fn test_run() { compute(); }\n}\n",
        )
        .unwrap();
        scratch
    }

    #[test]
    fn warm_start_hits_cache_and_edit_reparses_only_changed_file() {
        let scratch = scratch("warm");
        let dir = scratch.path().to_path_buf();
        let cache = GraphCache::new(&dir);

        // Cold build: everything is a miss.
        let (_, cold) = build_graph_cached(&dir, &cache);
        assert_eq!(cold.files, 2);
        assert_eq!(cold.cache_misses, 2);
        assert_eq!(cold.cache_hits, 0);

        // Warm build with no changes: every file hits the cache (no re-parse).
        let (_, warm) = build_graph_cached(&dir, &cache);
        assert_eq!(warm.cache_hits, 2);
        assert_eq!(warm.cache_misses, 0);

        // Edit one file: exactly one miss, the rest still hit.
        std::fs::write(dir.join("src/lib.rs"), "pub fn compute() -> i32 { 2 }\n").unwrap();
        let (_, after) = build_graph_cached(&dir, &cache);
        assert_eq!(after.cache_misses, 1);
        assert_eq!(after.cache_hits, 1);
    }

    #[test]
    fn cached_build_matches_a_fresh_build() {
        let scratch = scratch("determinism");
        let dir = scratch.path().to_path_buf();
        let cache = GraphCache::new(&dir);

        let (fresh, _) = build_graph_cached(&dir, &cache);
        // Second build entirely from cache must produce the same graph shape.
        let (cached, stats) = build_graph_cached(&dir, &cache);
        assert_eq!(stats.cache_hits, 2);
        assert_eq!(fresh.node_count(), cached.node_count());
        assert_eq!(fresh.edge_count(), cached.edge_count());
    }

    #[tokio::test]
    async fn service_builds_invalidates_and_rebuilds() {
        let scratch = scratch("service");
        let dir = scratch.path().to_path_buf();
        let service = GraphService::new(&dir);

        let g1 = service.graph(false).await.unwrap();
        assert!(g1.node_count() > 0);
        // Cached: same Arc handed back.
        let g2 = service.graph(false).await.unwrap();
        assert!(Arc::ptr_eq(&g1, &g2));

        // After a change + invalidate, a fresh graph is built.
        std::fs::write(
            dir.join("src/lib.rs"),
            "pub fn compute() -> i32 { 3 }\npub fn extra() {}\n",
        )
        .unwrap();
        service.invalidate().await;
        let g3 = service.graph(false).await.unwrap();
        assert!(!Arc::ptr_eq(&g1, &g3));
        assert!(g3.node_count() > g1.node_count());
    }

    /// A build that panics is an error the caller sees, and the next query tries again.
    ///
    /// The break this catches: a panicking build became an empty graph and was cached, so `impact`
    /// answered "nothing depends on this" with no error, for the rest of the session.
    #[tokio::test]
    async fn a_panicking_build_reports_an_error_and_is_not_kept() {
        fn panics(_: &Path) -> InMemoryGraph {
            panic!("an extractor bug")
        }
        let service = GraphService {
            root: PathBuf::from("/nowhere"),
            graph: Mutex::new(None),
            build: panics,
        };
        let Err(error) = service.graph(false).await else { panic!("a graph from a panic") };
        assert!(error.to_string().contains("could not be built"), "{error}");
        assert!(service.graph.lock().await.is_none(), "nothing cached");
    }
}
