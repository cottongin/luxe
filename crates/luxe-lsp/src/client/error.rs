//! Errors — what a language-server client can fail with.

use std::time::Duration;

use crate::message::JsonRpcError;

/// LSP client errors.
#[derive(Debug, thiserror::Error)]
pub enum LspError {
    #[error("client not initialized")]
    NotInitialized,

    #[error("failed to spawn language server '{server}': {error}")]
    SpawnFailed { server: String, error: String },

    #[error("initialize timed out after {timeout:?} for server '{server}'")]
    InitTimeout { server: String, timeout: Duration },

    /// The server was found and started, and then stopped before answering the handshake.
    ///
    /// Distinct from [`Self::SpawnFailed`] (nothing to run) and [`Self::InitTimeout`] (running but
    /// silent), because it is the only one whose cause the *server* already explained — and the
    /// commonest way to reach it is a version-manager shim that resolves and then refuses.
    #[error("language server '{server}' exited during startup ({status}){}", detail(stderr))]
    ServerExited { server: String, status: String, stderr: String },

    /// The server answered the request with a JSON-RPC error.
    ///
    /// Its own words, propagated rather than flattened to an empty result — "no references found"
    /// and "this server cannot answer you" are not the same sentence.
    #[error("the language server refused the request: {} (code {})", .0.message, .0.code)]
    ServerRefused(JsonRpcError),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("request cancelled")]
    RequestCancelled,
}

/// The server's own complaint, appended to an error when it made one.
pub(super) fn detail(stderr: &str) -> String {
    let first: Vec<&str> = stderr.lines().filter(|line| !line.trim().is_empty()).take(3).collect();
    match first.is_empty() {
        true => String::new(),
        false => format!(": {}", first.join(" / ")),
    }
}
