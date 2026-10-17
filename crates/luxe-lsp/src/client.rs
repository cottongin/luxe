//! Generic, warm LSP client.
//!
//! Spawns a language server, speaks JSON-RPC over stdio with Content-Length framing, and stays
//! **warm**: one client is reused across many tool calls. A background reader folds
//! `publishDiagnostics` and `$/progress` into shared state so `get_diagnostics` can wait for the
//! server to *settle* (indexing/flycheck idle + a quiescence window) and return the real result,
//! not the first publish, which servers routinely send empty before indexing. Documents are opened
//! once and updated with `didChange`; an unchanged warm read returns the cached diagnostics
//! immediately.
//!
//! # Push and pull
//!
//! Two protocols answer the same question. Most servers *push* `publishDiagnostics` unasked;
//! LSP 3.17 added a *pull* model where the client requests `textDocument/diagnostic` and the
//! server answers when it has an answer. A server that only pulls — TypeScript 7's native LSP is
//! one — says nothing at all about an open file, which a push-only client cannot tell apart from
//! a clean one. See `LspClient::pulls_diagnostics_for` for which of the two a read uses.

use crate::message::JsonRpcMessage;
use crate::protocol::{Diagnostic, Location, MarkupContent, Position};
use luxe_core::jsonrpc_frame::write_frame;
use serde_json::Value;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufRead, AsyncReadExt};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::time::timeout;
use tracing::{debug, error};

mod diagnostics;
mod error;
mod language;
mod lifecycle;
mod nav;
mod state;
mod transport;

pub use error::LspError;
use language::language_id_for_extension;
use state::{PendingEntry, PullProvider, PulledReport};
pub(crate) use state::{PendingRequests, SharedState};

use transport::read_loop;

/// Configuration for an LSP client.
#[derive(Debug, Clone)]
pub struct LspConfig {
    /// Command to launch the language server.
    pub command: Vec<String>,
    /// Language server ID (e.g., "rust-analyzer").
    pub server_id: String,
    /// Working directory for the language server.
    pub root_path: PathBuf,
    /// Timeout for the initialize handshake.
    pub init_timeout: Duration,
}

/// State of the LSP client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientState {
    /// Client has not been initialized.
    Uninitialized,
    /// Client is initialized and ready.
    Initialized,
    /// Client has been shut down.
    Shutdown,
}

/// A generic, warm LSP client.
pub struct LspClient {
    config: LspConfig,
    /// Child process (stdio); `kill_on_drop` reaps it when the client is dropped.
    child: Option<Child>,
    /// Request ID counter.
    next_id: u64,
    /// Current client state.
    state: ClientState,
    /// Pending requests: id → response channel (shared with the reader task).
    pending_requests: PendingRequests,
    /// Best-effort notification tap (kept for `wait_for_notification`).
    notification_rx: mpsc::Receiver<JsonRpcMessage>,
    notification_tx: mpsc::Sender<JsonRpcMessage>,
    /// Stdin handle, shared so the reader can auto-reply to server→client requests.
    stdin: Option<Arc<Mutex<ChildStdin>>>,
    /// Live diagnostics/progress state folded by the reader.
    shared: Arc<SharedState>,
    /// Open documents: URI → (version, content hash) for `didChange` sync + warm reuse.
    open_docs: HashMap<String, (i32, u64)>,
    /// The server's `diagnosticProvider` capability, when it answers `textDocument/diagnostic`.
    pull_diagnostics: Option<PullProvider>,
    /// Latest pull-diagnostics report per URI.
    ///
    /// Kept beside the pushed cache rather than merged into it, so "this server has never said
    /// anything about this file" stays a question the client can answer.
    pulled: HashMap<String, PulledReport>,
    /// Keeps the server's stderr drained for as long as it runs; see [`drain_stderr`].
    stderr_drain: Option<tokio::task::JoinHandle<()>>,
}

impl LspClient {
    /// Create a new LSP client with the given configuration.
    pub fn new(config: LspConfig) -> Self {
        let (notification_tx, notification_rx) = mpsc::channel(100);
        Self {
            config,
            child: None,
            next_id: 1,
            state: ClientState::Uninitialized,
            pending_requests: Arc::new(Mutex::new(HashMap::new())),
            notification_rx,
            notification_tx,
            stdin: None,
            shared: SharedState::new(),
            open_docs: HashMap::new(),
            pull_diagnostics: None,
            pulled: HashMap::new(),
            stderr_drain: None,
        }
    }

    /// The spawned server's pid, while it is running.
    ///
    /// For asserting that a failed handshake actually reaped its child rather than abandoning it.
    pub fn child_id(&self) -> Option<u32> {
        self.child.as_ref().and_then(tokio::process::Child::id)
    }

    /// The `languageId` to announce for `uri`.
    ///
    /// The extension decides, and this server's own language is the fallback — not `rust`. A `.c`
    /// file opened against clangd announced as Rust, or a `.tsx` as plain `typescript`, is an
    /// assertion about the file that is false, and a server is entitled to act on it.
    fn language_id_for(&self, uri: &str) -> String {
        uri.rsplit('.')
            .next()
            .filter(|extension| !extension.contains('/'))
            .and_then(language_id_for_extension)
            .map_or_else(|| self.config.server_id.clone(), str::to_string)
    }

    /// Send a request and wait for the matching response.
    pub async fn send_request(&mut self, method: &str, params: Value) -> Result<Value, LspError> {
        if self.state != ClientState::Initialized {
            return Err(LspError::NotInitialized);
        }
        self.request(method, params).await
    }

    /// Sends a request whatever the client's state: the handshake's own `initialize` goes out
    /// before the client is ready.
    async fn request(&mut self, method: &str, params: Value) -> Result<Value, LspError> {
        let stdin = self
            .stdin
            .clone()
            .ok_or_else(|| LspError::IoError(std::io::Error::other("Stdin not available")))?;

        let id = self.next_id;
        self.next_id += 1;
        let message = JsonRpcMessage::request(id, method.to_owned(), params);
        let json = serde_json::to_string(&message).map_err(LspError::Serialization)?;

        let (tx, rx) = oneshot::channel();
        self.pending_requests.lock().await.insert(id, tx);
        // a failed write or a dropped future would otherwise leave the entry behind
        let _forget = PendingEntry { pending: Arc::clone(&self.pending_requests), id };
        {
            let mut guard = stdin.lock().await;
            write_frame(&mut *guard, json.as_bytes()).await.map_err(LspError::IoError)?;
        }
        let response = rx.await.map_err(|_| LspError::RequestCancelled)?;
        response.map_err(LspError::ServerRefused)
    }

    /// Send a notification (no response expected).
    pub async fn send_notification(&mut self, method: &str, params: Value) -> Result<(), LspError> {
        if self.state != ClientState::Initialized {
            return Err(LspError::NotInitialized);
        }
        self.notify(method, params).await
    }

    /// Sends a notification whatever the client's state, as the handshake's `initialized` is.
    async fn notify(&mut self, method: &str, params: Value) -> Result<(), LspError> {
        let stdin = self
            .stdin
            .clone()
            .ok_or_else(|| LspError::IoError(std::io::Error::other("Stdin not available")))?;
        let message = JsonRpcMessage::notification(method.to_owned(), params);
        let json = serde_json::to_string(&message).map_err(LspError::Serialization)?;
        let mut guard = stdin.lock().await;
        write_frame(&mut *guard, json.as_bytes()).await.map_err(LspError::IoError)
    }

    /// Receive a notification without blocking (best-effort tap).
    pub async fn receive_notification(&mut self) -> Option<JsonRpcMessage> {
        self.notification_rx.try_recv().ok()
    }

    /// Wait for the next notification (best-effort tap).
    pub async fn wait_for_notification(&mut self) -> Option<JsonRpcMessage> {
        self.notification_rx.recv().await
    }
}

#[cfg(test)]
mod tests;
