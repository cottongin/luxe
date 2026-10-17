//! Graph invalidation hook — keeps the code graph and the symbol index fresh on file changes.
//!
//! The blast-radius tools (`impact`, `module_report.blastRadius`) read a warm [`GraphService`].
//! When the edit/write tools raise a `FileChanged` event, this hook invalidates the cached graph so
//! the next query rebuilds from the content-addressed cache — re-parsing only the changed file. It
//! lives in `luxe-cli` because `luxe-core` (where hooks are defined) can't depend on `luxe-lsp`.

use async_trait::async_trait;
use luxe_core::hook::{Hook, HookCtx, HookEvent, HookOutcome, HookPoint};
use luxe_lsp::graph::GraphService;
use std::sync::Arc;

/// Invalidates the shared code graph, and the symbol index when given one, whenever a file changes.
pub(crate) struct GraphInvalidateHook {
    graph: Arc<GraphService>,
    index: Option<Arc<luxe_lsp::SymbolIndexCache>>,
}

impl GraphInvalidateHook {
    pub(crate) fn new(graph: Arc<GraphService>) -> Self {
        Self { graph, index: None }
    }

    /// Forget `index` on a file change too.
    pub(crate) fn with_symbol_index(mut self, index: Arc<luxe_lsp::SymbolIndexCache>) -> Self {
        self.index = Some(index);
        self
    }
}

#[async_trait]
impl Hook for GraphInvalidateHook {
    fn name(&self) -> &'static str {
        "graph-invalidate"
    }

    fn interest(&self) -> &[HookPoint] {
        &[HookPoint::FileChanged]
    }

    async fn on(&self, _point: HookPoint, ev: &HookEvent, _cx: &HookCtx) -> HookOutcome {
        if matches!(ev, HookEvent::FileChanged { .. }) {
            self.graph.invalidate().await;
            if let Some(index) = &self.index {
                index.invalidate().await;
            }
        }
        HookOutcome::Continue
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file change reaches the symbol index, so the next search reads the file the agent wrote.
    ///
    /// The break this catches: the index was built once and only `refresh` rebuilt it, so a symbol
    /// written in the session was "No symbols matching" until somebody asked for a rebuild.
    #[tokio::test]
    async fn a_file_change_forgets_the_symbol_index() {
        let project = tempfile::tempdir().unwrap();
        std::fs::write(project.path().join("lib.rs"), "pub fn first() {}\n").unwrap();
        let index = Arc::new(luxe_lsp::SymbolIndexCache::default());
        let hook = GraphInvalidateHook::new(Arc::new(GraphService::new(project.path())))
            .with_symbol_index(Arc::clone(&index));
        assert!(index.get(project.path(), false).await.unwrap().search("second", 5).is_empty());

        std::fs::write(project.path().join("more.rs"), "pub fn second() {}\n").unwrap();
        let (event_tx, _events) = tokio::sync::mpsc::channel(4);
        let cx = HookCtx {
            session_id: "s".into(),
            event_tx,
            hook_registry: luxe_core::HookRegistry::new(),
        };
        let changed = HookEvent::FileChanged { path: "more.rs".into(), operation: "write".into() };
        hook.on(HookPoint::FileChanged, &changed, &cx).await;
        assert_eq!(index.get(project.path(), false).await.unwrap().search("second", 5).len(), 1);
    }
}
