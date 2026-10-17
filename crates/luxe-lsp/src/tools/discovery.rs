//! Discovery funnel — `symbol_search` → `module_report` → `read_symbol`.
//!
//! A token-efficient path from "which symbols matter" (workspace search) to "what's in this file"
//! (outline) to "show me this one symbol's body" (verbatim read). Outlines prefer a **tree-sitter**
//! parse (fast, offline, no running server) and fall back to the warm LSP service's
//! `documentSymbol`; `symbol_search` uses `workspace/symbol`.

use super::*;
use crate::protocol::{symbol_kind_name, FlatSymbol};
use crate::syntax;
use crate::workspace::WorkspaceLspClient;
use tokio::sync::Mutex;

/// Shared warm service handle used by every discovery tool.
pub(super) type Service = Arc<WorkspaceLspClient>;

/// Records a successful funnel read against the shared tracker (no-op when unwired), so the
/// `edit`/`write` guards accept a subsequent mutation of the same file.
pub(super) fn record_read(
    tracker: Option<&Arc<luxe_core::read_tracker::ReadTracker>>,
    cx: &ToolContext,
    file: &str,
) {
    if let Some(tracker) = tracker {
        tracker.record(&cx.session_id, &cx.cwd.join(file));
    }
}

/// A file outline plus the state needed to enrich it (content for reads, a client for usages).
pub(super) struct Outline {
    symbols: Vec<FlatSymbol>,
    content: Option<String>,
    /// Warm LSP client + URI, when one was resolved (always for the LSP path; best-effort for the
    /// tree-sitter path when usages are requested).
    client: Option<Arc<Mutex<LspClient>>>,
    uri: String,
    /// `"tree-sitter"` or `"lsp"`.
    source: &'static str,
}

/// Build a file outline, preferring tree-sitter and falling back to the LSP `documentSymbol`.
///
/// `need_client` requests a warm LSP client for follow-up work (who-uses-this); the tree-sitter
/// path only resolves one when asked, since it doesn't otherwise need a server.
pub(super) async fn build_outline(
    service: &Service,
    cx: &ToolContext,
    file: &str,
    need_client: bool,
) -> Result<Outline, String> {
    let full = cx.cwd.join(file);
    // A typo'd path must not fall through to "no symbols", which reads like an empty file rather
    // than a wrong path. Say which it is.
    if !full.exists() {
        return Err(format!(
            "file not found: {file} (resolved to {})",
            luxe_core::paths::display(&full)
        ));
    }
    if full.is_dir() {
        return Err(format!("{file} is a directory — pass a file path"));
    }
    let uri = crate::uri::from_path(&full);
    let content = std::fs::read_to_string(&full).ok();

    // Tree-sitter path: offline, no server needed.
    if syntax::supported(&full) {
        if let Some(source) = &content {
            let symbols = syntax::outline(&full, source).unwrap_or_default();
            let client =
                if need_client { open_client(service, &full, &uri, source).await } else { None };
            return Ok(Outline { symbols, content, client, uri, source: "tree-sitter" });
        }
    }

    // LSP path: needs a running server.
    let client =
        { service.client_for_path(&full).await }.ok_or("no language server for this file")?;
    if let Some(source) = &content {
        let _ = client.lock().await.open_document(&uri, source).await;
    }
    let symbols = client
        .lock()
        .await
        .document_symbols(&uri)
        .await
        .map_err(|e| format!("outline failed: {e}"))?;
    Ok(Outline { symbols, content, client: Some(client), uri, source: "lsp" })
}

/// Best-effort: resolve + warm an LSP client for a file (opening the document).
pub(super) async fn open_client(
    service: &Service,
    full: &Path,
    uri: &str,
    content: &str,
) -> Option<Arc<Mutex<LspClient>>> {
    let client = { service.client_for_path(full).await }?;
    let _ = client.lock().await.open_document(uri, content).await;
    Some(client)
}

mod module_report;
mod read_symbol;
mod symbol_search;

pub use module_report::ModuleReportTool;
pub use read_symbol::ReadSymbolTool;
pub use symbol_search::SymbolSearchTool;

/// Max symbols listed by `symbol_search` / `module_report`.
pub(super) const MAX_SYMBOLS: usize = 60;

#[cfg(test)]
mod tests;
