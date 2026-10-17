//! Fetched — tests for the file-changed hook.

use super::*;
use std::sync::Mutex;

/// Records every file it was asked about.
struct Recording(Arc<Mutex<Vec<PathBuf>>>);

#[async_trait]
impl crate::lsp::DiagnosticsProvider for Recording {
    async fn run_diagnostics(
        &self,
        path: &std::path::Path,
    ) -> Result<Vec<crate::lsp::Diagnostic>, crate::lsp::DiagnosticsError> {
        self.0.lock().expect("lock").push(path.to_path_buf());
        Ok(Vec::new())
    }
}

#[tokio::test]
async fn an_edit_inside_a_fetched_repository_is_neither_formatted_nor_checked() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let lsp: Arc<dyn crate::lsp::DiagnosticsProvider> = Arc::new(Recording(seen.clone()));
    let hook = FileChangedHook::new(None, Some(lsp));
    let cx = HookCtx {
        session_id: "test".to_string(),
        event_tx: tokio::sync::mpsc::channel(1).0,
        hook_registry: crate::hook::HookRegistry::new(),
    };
    for path in ["/p/.luxe/gh-cache/o/r/build.rs", "/p/src/lib.rs"] {
        let event =
            HookEvent::FileChanged { path: path.to_string(), operation: "edit".to_string() };
        hook.on(HookPoint::FileChanged, &event, &cx).await;
    }
    assert_eq!(*seen.lock().expect("lock"), vec![PathBuf::from("/p/src/lib.rs")]);
}
