//! Client state — what the background reader folds in, and the requests waiting on it.

use crate::message::JsonRpcError;
use crate::protocol::Diagnostic;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex as SyncMutex};
use std::time::{Duration, Instant};
use tokio::sync::{oneshot, Mutex, Notify};

/// Live server→client state folded by the background reader.
///
/// Guarded by a std mutex held only for brief, non-async updates: the diagnostics cache keyed by
/// document URI, a count of in-flight `$/progress` work-done tokens, a monotonic publish counter,
/// and the last-change instant used for quiescence.
pub(crate) struct DiagnosticsState {
    /// Latest *published* diagnostics per document URI.
    ///
    /// The key is itself an answer: present with an empty vec means the server said "clean", and
    /// absent means it has said nothing at all, which is what decides whether a pull-capable
    /// server still has to be asked directly. Only `publishDiagnostics` writes here — a pulled
    /// report lives on the client, in [`LspClient::pulled`], so the distinction survives.
    pub(crate) diagnostics: HashMap<String, Vec<Diagnostic>>,
    /// In-flight work-done progress tokens; `0` means the server is idle.
    pub(crate) progress_active: usize,
    /// Monotonic count of `publishDiagnostics` notifications seen.
    pub(crate) publish_count: u64,
    /// Whether the server reported itself quiescent (ready) via `experimental/serverStatus` — the
    /// reliable readiness signal for rust-analyzer. Other servers leave this `false` and readiness
    /// falls back to a progress-settle window.
    pub(crate) quiescent: bool,
    /// Recent server-reported Error/Warning notices (`window/showMessage` + `window/logMessage`),
    /// bounded. Surfaced when a diagnostics read comes back empty so a broken/misconfigured server
    /// (e.g. it can't find a usable toolchain) isn't a silent "no diagnostics".
    pub(crate) server_notices: Vec<ServerNotice>,
    /// Instant of the last diagnostics/progress change, for quiescence checks.
    last_change: Instant,
}

/// One Error/Warning message a server sent via `window/showMessage` or `window/logMessage`.
#[derive(Clone)]
pub(crate) struct ServerNotice {
    /// LSP `MessageType`: 1 = Error, 2 = Warning.
    pub(crate) severity: u8,
    pub(crate) text: String,
}

/// A server's advertised `diagnosticProvider`, as far as a pull request needs it.
#[derive(Debug)]
pub(super) struct PullProvider {
    /// The identifier the server registered under, echoed back on every request it asked for.
    pub(super) identifier: Option<String>,
}

/// The last `textDocument/diagnostic` answer for one document.
#[derive(Debug)]
pub(super) struct PulledReport {
    /// The server's token for this answer, sent back so it can reply `unchanged`.
    pub(super) result_id: Option<String>,
    pub(super) items: Vec<Diagnostic>,
}

impl DiagnosticsState {
    pub(crate) fn mark_change(&mut self) {
        self.last_change = Instant::now();
    }

    /// Record a server Error/Warning notice (deduping the immediately-repeated one, bounded).
    pub(crate) fn push_notice(&mut self, severity: u8, text: String) {
        if self.server_notices.last().is_some_and(|n| n.text == text) {
            return;
        }
        self.server_notices.push(ServerNotice { severity, text });
        const MAX_NOTICES: usize = 8;
        let len = self.server_notices.len();
        if len > MAX_NOTICES {
            self.server_notices.drain(0..len - MAX_NOTICES);
        }
    }

    pub(super) fn since_change(&self) -> Duration {
        self.last_change.elapsed()
    }
}

/// In-flight requests, each waiting on the server's answer or its refusal.
pub(crate) type PendingRequests =
    Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, JsonRpcError>>>>>;

/// Removes a request's pending entry however the request ends.
pub(super) struct PendingEntry {
    pub(super) pending: PendingRequests,
    pub(super) id: u64,
}

impl Drop for PendingEntry {
    fn drop(&mut self) {
        let id = self.id;
        if let Ok(mut map) = self.pending.try_lock() {
            map.remove(&id);
            return;
        }
        // the reader holds it for a moment; finish the removal behind it
        let pending = Arc::clone(&self.pending);
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move { pending.lock().await.remove(&id) });
        }
    }
}

/// Shared state between the client and its background reader.
pub(crate) struct SharedState {
    pub(crate) inner: SyncMutex<DiagnosticsState>,
    pub(crate) notify: Notify,
}

impl SharedState {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: SyncMutex::new(DiagnosticsState {
                diagnostics: HashMap::new(),
                progress_active: 0,
                publish_count: 0,
                quiescent: false,
                server_notices: Vec::new(),
                last_change: Instant::now(),
            }),
            notify: Notify::new(),
        })
    }

    pub(super) fn mark_change(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.mark_change();
        }
        self.notify.notify_waiters();
    }
}
