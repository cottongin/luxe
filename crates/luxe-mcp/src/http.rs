//! Streamable HTTP transport for remote MCP servers.
//!
//! Speaks the MCP "Streamable HTTP" transport: JSON-RPC is POSTed to the server URL, and the server
//! answers with either `application/json` (one response) or `text/event-stream` (an SSE stream we
//! read until our request id comes back). A server-issued `Mcp-Session-Id` is echoed on later
//! requests. Lives here rather than in `luxe-core` because core carries no HTTP dependency.
//!
//! Auth: a static bearer token (literal or from an env var) plus arbitrary headers, both with
//! `${VAR}` interpolation, or an OAuth bearer from [`crate::oauth`].

use std::sync::atomic::{AtomicI64, Ordering};

use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use reqwest::Client;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use luxe_core::mcp_client::{
    McpAuth, McpConnection, McpError, McpServerConfig, McpTool, McpToolResult, McpTransport,
};

use crate::oauth::{OAuthClient, OAuthError, TokenState};
use crate::secret::SecretError;
use std::sync::Arc;

/// How long reaching the server may take
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
/// How long its handshake may take once reached
const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Why the HTTP transport could not connect or get an answer.
///
/// Each carries the sentence the reader sees. [`McpError`] is what the engine gets: the variants
/// here are sorted into its four at the boundary.
#[derive(Debug, thiserror::Error)]
pub enum McpHttpError {
    /// The configured transport is not HTTP
    #[error("server '{0}' is not an HTTP transport")]
    NotHttp(String),
    /// A header, a bearer token or the client could not be built from the config
    #[error("{0}")]
    Config(String),
    /// An OAuth step refused: usually a URL that would send a credential somewhere it could be read
    #[error(transparent)]
    OAuth(#[from] OAuthError),
    /// A configured secret did not resolve
    #[error(transparent)]
    Secret(#[from] SecretError),
    /// The server needs a sign-in the stored one does not provide
    #[error("{0}")]
    SignIn(String),
    /// The handshake did not finish in time
    #[error("'{0}' did not finish its handshake in time")]
    HandshakeTimeout(String),
    /// The request did not reach the server, or its answer broke off
    #[error("{0}")]
    Transport(String),
    /// The server answered with an HTTP error
    #[error("{method} returned HTTP {status}")]
    Status {
        /// The JSON-RPC method that was sent
        method: String,
        /// What the server said
        status: reqwest::StatusCode,
    },
    /// The server answered with a JSON-RPC error
    #[error("{0}")]
    Rpc(String),
    /// The answer was not MCP
    #[error("{0}")]
    Protocol(String),
    /// The server answered 404 to the session the request carried
    #[error("the server's session expired")]
    SessionExpired(String),
}

impl From<McpHttpError> for McpError {
    /// Sorted by what went wrong; the text stays this transport's own.
    fn from(error: McpHttpError) -> Self {
        let text = error.to_string();
        match error {
            McpHttpError::Rpc(_) => Self::Rpc(text),
            McpHttpError::Protocol(_) => Self::Protocol(text),
            McpHttpError::SessionExpired(_) => Self::Closed(text),
            McpHttpError::NotHttp(_)
            | McpHttpError::Config(_)
            | McpHttpError::OAuth(_)
            | McpHttpError::Secret(_)
            | McpHttpError::SignIn(_)
            | McpHttpError::HandshakeTimeout(_)
            | McpHttpError::Transport(_)
            | McpHttpError::Status { .. } => Self::Transport(text),
        }
    }
}

/// A live Streamable-HTTP connection to a remote MCP server.
pub struct HttpConnection {
    server_id: String,
    url: String,
    client: Client,
    session_id: Mutex<Option<String>>,
    /// The version the server agreed to in `initialize`, sent on every request after it
    protocol_version: Mutex<Option<String>>,
    /// Held while an expired session is started again, so concurrent calls restart it once
    restarting: Mutex<()>,
    next_id: AtomicI64,
    /// When set, a fresh OAuth bearer is attached per request (handles expiry/refresh).
    oauth: Option<Arc<OAuthClient>>,
}

impl HttpConnection {
    /// Build the HTTP client (headers + auth) and run the `initialize` handshake.
    pub async fn connect(config: &McpServerConfig) -> Result<Self, McpHttpError> {
        Self::connect_within(config, HANDSHAKE_TIMEOUT).await
    }

    /// [`Self::connect`], with the handshake held to `handshake`.
    pub(crate) async fn connect_within(
        config: &McpServerConfig,
        handshake: std::time::Duration,
    ) -> Result<Self, McpHttpError> {
        let url = match &config.transport {
            McpTransport::HttpSse { url } => url.clone(),
            McpTransport::Stdio { .. } => return Err(McpHttpError::NotHttp(config.id.clone())),
        };

        // a token goes nowhere it could be read on the way
        if config.auth.is_some() {
            crate::oauth::require_secure(&url, "the MCP server")?;
        }
        let mut headers = HeaderMap::new();
        for (name, value) in &config.headers {
            let header = HeaderName::from_bytes(name.as_bytes())
                .map_err(|e| McpHttpError::Config(format!("invalid header name '{name}': {e}")))?;
            let expanded = luxe_core::mcp_client::expand_env(value);
            // a header filled from the environment is usually the server's key
            if expanded != *value {
                crate::secret::remembered(&expanded);
            }
            let value = HeaderValue::from_str(&expanded).map_err(|e| {
                McpHttpError::Config(format!("invalid header value for '{name}': {e}"))
            })?;
            headers.insert(header, value);
        }
        let mut oauth = None;
        match &config.auth {
            Some(McpAuth::Bearer { token, token_env }) => {
                let resolved = resolve_bearer(
                    token.as_deref(),
                    token_env.as_deref(),
                    config.declared_by_user,
                    &config.id,
                )
                .await?;
                let mut value = HeaderValue::from_str(&format!("Bearer {resolved}"))
                    .map_err(|e| McpHttpError::Config(format!("invalid bearer token: {e}")))?;
                value.set_sensitive(true);
                headers.insert(AUTHORIZATION, value);
            }
            Some(McpAuth::OAuth(cfg)) => {
                oauth = Some(Arc::new(
                    OAuthClient::new(config.id.clone(), url.clone(), cfg.clone())
                        .with_secret_commands(config.declared_by_user),
                ));
            }
            None => {}
        }

        let client = Client::builder()
            .default_headers(headers)
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .map_err(|e| McpHttpError::Config(format!("http client build failed: {e}")))?;

        let conn = Self {
            server_id: config.id.clone(),
            url,
            client,
            session_id: Mutex::new(None),
            protocol_version: Mutex::new(None),
            restarting: Mutex::new(()),
            next_id: AtomicI64::new(1),
            oauth,
        };
        tokio::time::timeout(handshake, conn.handshake())
            .await
            .map_err(|_| McpHttpError::HandshakeTimeout(conn.server_id.clone()))??;
        Ok(conn)
    }

    /// Attach a fresh OAuth bearer (refreshing if needed) to a request builder. A server that still
    /// needs interactive login fails the request with an actionable message.
    async fn authorize(
        &self,
        builder: reqwest::RequestBuilder,
    ) -> Result<reqwest::RequestBuilder, McpHttpError> {
        let Some(oauth) = &self.oauth else { return Ok(builder) };
        match oauth.access_token().await {
            TokenState::Have(token) => Ok(builder.bearer_auth(token)),
            TokenState::NeedsLogin(reason) | TokenState::Unavailable(reason) => {
                Err(McpHttpError::SignIn(reason))
            }
        }
    }

    async fn handshake(&self) -> Result<(), McpHttpError> {
        *self.session_id.lock().await = None;
        *self.protocol_version.lock().await = None;
        let params = json!({
            "protocolVersion": luxe_core::mcp_client::MCP_PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": { "name": "luxe", "version": luxe_core::version::VERSION },
        });
        let result = self.request_once("initialize", &params).await?;
        let agreed = result.get("protocolVersion").and_then(Value::as_str);
        *self.protocol_version.lock().await = agreed.map(str::to_string);
        self.notify("notifications/initialized").await?;
        Ok(())
    }

    /// Send a JSON-RPC request and return its `result`, starting the session again once if the
    /// server says it has expired.
    async fn request(&self, method: &str, params: Value) -> Result<Value, McpHttpError> {
        match self.request_once(method, &params).await {
            Err(McpHttpError::SessionExpired(expired)) => {
                self.restart_session(&expired).await?;
                self.request_once(method, &params).await
            }
            other => other,
        }
    }

    /// Starts the session again if it is still the one that expired: of calls that all saw it
    /// expire, the first restarts it and the rest go out on the new one.
    async fn restart_session(&self, expired: &str) -> Result<(), McpHttpError> {
        let _restarting = self.restarting.lock().await;
        if self.session_id.lock().await.as_deref() == Some(expired) {
            tracing::info!(server = %self.server_id, "mcp: session expired; starting a new one");
            self.handshake().await?;
        }
        Ok(())
    }

    /// Send a JSON-RPC request once and return its `result`, handling json + SSE responses.
    async fn request_once(&self, method: &str, params: &Value) -> Result<Value, McpHttpError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let body = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });

        let builder = self
            .client
            .post(&self.url)
            .header(ACCEPT, "application/json, text/event-stream")
            .json(&body);
        let (builder, sent_session) = self.with_session(builder).await?;
        let response = builder
            .send()
            .await
            .map_err(|e| McpHttpError::Transport(format!("{method} request failed: {e}")))?;
        if let Some(session) = response.headers().get("mcp-session-id") {
            if let Ok(value) = session.to_str() {
                *self.session_id.lock().await = Some(value.to_string());
            }
        }
        // the spec's word for an expired session; one without a session is just not found
        if let Some(sent) = sent_session.filter(|_| response.status() == 404) {
            return Err(McpHttpError::SessionExpired(sent));
        }
        if !response.status().is_success() {
            return Err(McpHttpError::Status {
                method: method.to_string(),
                status: response.status(),
            });
        }

        let is_sse = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|ct| ct.contains("text/event-stream"));

        if is_sse {
            read_sse_response(response, id).await
        } else {
            let value: Value = response
                .json()
                .await
                .map_err(|e| McpHttpError::Protocol(format!("bad json: {e}")))?;
            extract_result(&value)
        }
    }

    /// `builder` with the session, the agreed protocol version and the bearer it carries, and the
    /// session it went out on.
    async fn with_session(
        &self,
        mut builder: reqwest::RequestBuilder,
    ) -> Result<(reqwest::RequestBuilder, Option<String>), McpHttpError> {
        let session = self.session_id.lock().await.clone();
        if let Some(session) = &session {
            builder = builder.header("Mcp-Session-Id", session.as_str());
        }
        if let Some(version) = self.protocol_version.lock().await.clone() {
            builder = builder.header("MCP-Protocol-Version", version);
        }
        Ok((self.authorize(builder).await?, session))
    }

    /// Send a JSON-RPC notification (no id, no response expected).
    async fn notify(&self, method: &str) -> Result<(), McpHttpError> {
        let body = json!({ "jsonrpc": "2.0", "method": method });
        let (builder, _) = self.with_session(self.client.post(&self.url).json(&body)).await?;
        // Notifications are fire-and-forget; a transport hiccup here shouldn't fail the session.
        if let Err(error) = builder.send().await {
            tracing::debug!(server = %self.server_id, %error, "mcp: notification send failed");
        }
        Ok(())
    }
}

#[async_trait]
impl McpConnection for HttpConnection {
    fn server_id(&self) -> &str {
        &self.server_id
    }

    async fn list_tools(&self) -> Result<Vec<McpTool>, McpError> {
        let result = self.request("tools/list", json!({})).await?;
        luxe_core::mcp_client::parse_tool_list(&result, &self.server_id)
            .ok_or_else(|| McpError::Protocol("tools/list missing `tools` array".to_string()))
    }

    async fn call_tool(&self, raw_name: &str, arguments: Value) -> Result<McpToolResult, McpError> {
        let result =
            self.request("tools/call", json!({ "name": raw_name, "arguments": arguments })).await?;
        Ok(luxe_core::mcp_client::parse_tool_call_result(&result))
    }
}

/// Read an SSE stream until a JSON-RPC message with `id` arrives, returning its `result`.
async fn read_sse_response(response: reqwest::Response, id: i64) -> Result<Value, McpHttpError> {
    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|e| McpHttpError::Transport(format!("sse read error: {e}")))?;
        buffer.push_str(&String::from_utf8_lossy(&bytes));
        while let Some(newline) = buffer.find('\n') {
            let line: String = buffer.drain(..=newline).collect();
            let line = line.trim_end();
            let Some(data) = line.strip_prefix("data:") else { continue };
            let data = data.trim();
            if data.is_empty() {
                continue;
            }
            if let Ok(value) = serde_json::from_str::<Value>(data) {
                if value.get("id").and_then(Value::as_i64) == Some(id) {
                    return extract_result(&value);
                }
            }
        }
    }
    Err(McpHttpError::Protocol("SSE stream ended before a matching response".to_string()))
}

/// Pull `result` out of a JSON-RPC envelope, surfacing an `error` as `Err`.
fn extract_result(message: &Value) -> Result<Value, McpHttpError> {
    if let Some(error) = message.get("error") {
        let msg = error.get("message").and_then(Value::as_str).unwrap_or("unknown error");
        return Err(McpHttpError::Rpc(msg.to_string()));
    }
    Ok(message.get("result").cloned().unwrap_or(Value::Null))
}

/// Resolve a bearer token from `token` — a value, `${VAR}` or `!command`
/// ([`crate::secret::resolve_secret`]) — or else from the variable `token_env` names.
///
/// Args: `(token, token_env, commands_allowed, server_id)`.
async fn resolve_bearer(
    token: Option<&str>,
    token_env: Option<&str>,
    commands_allowed: bool,
    server_id: &str,
) -> Result<String, McpHttpError> {
    if let Some(written) = token.filter(|t| !t.is_empty()) {
        let what = format!("the bearer token for '{server_id}'");
        match crate::secret::resolve_secret(written, commands_allowed, &what).await {
            Ok(value) => return Ok(value),
            // An unset `${VAR}` falls through to `token_env`, as it always has; a `!command` that
            // was refused or failed does not, which would hide that the config asked for one
            Err(SecretError::Unset(_)) if token_env.is_some() => {}
            Err(error) => return Err(error.into()),
        }
    }
    if let Some(var) = token_env {
        let config = |message: String| McpHttpError::Config(message);
        let token = std::env::var(var)
            .map_err(|_| config(format!("bearer token env var '{var}' is not set")))?;
        if token.is_empty() {
            return Err(config(format!("env var '{var}' is empty")));
        }
        return Ok(crate::secret::remembered(&token));
    }
    Err(McpHttpError::Config("bearer auth needs `token` or `token_env`".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A token is never sent in the clear: a server reached over plain http off this machine is
    /// refused before anything is sent to it when it has auth configured.
    #[tokio::test]
    async fn a_token_is_not_sent_over_plain_http() {
        let config = McpServerConfig {
            id: "plain".into(),
            transport: McpTransport::HttpSse { url: "http://example.com/mcp".into() },
            auth: Some(McpAuth::Bearer { token: Some("plain-text-token".into()), token_env: None }),
            ..Default::default()
        };
        let error = HttpConnection::connect(&config).await.err().expect("refused");
        assert!(matches!(error, McpHttpError::OAuth(OAuthError::Insecure(_))), "{error:?}");
        assert!(error.to_string().contains("not https"), "{error}");
    }

    /// A header filled from the environment is usually the server's key, so it is remembered for
    /// redaction like a bearer token is.
    #[tokio::test]
    async fn a_header_from_the_environment_is_remembered_for_redaction() {
        std::env::set_var("LUXE_MCP_HEADER_KEY_TEST", "mcp-header-secret-91");
        let config = McpServerConfig {
            id: "h".into(),
            transport: McpTransport::HttpSse { url: "http://127.0.0.1:1/mcp".into() },
            headers: [("X-API-Key".into(), "${LUXE_MCP_HEADER_KEY_TEST}".into())]
                .into_iter()
                .collect(),
            ..Default::default()
        };
        let _ = HttpConnection::connect(&config).await;
        let seen = luxe_core::redact::process_secrets().redact("key mcp-header-secret-91");
        assert_eq!(seen, "key [REDACTED]");
    }

    /// A server that takes the connection and never answers is given up on, not waited for.
    #[tokio::test]
    async fn a_silent_server_fails_its_handshake_in_time() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let held = tokio::spawn(async move {
            let (_socket, _) = listener.accept().await.unwrap();
            std::future::pending::<()>().await;
        });
        let config = McpServerConfig {
            id: "silent".into(),
            transport: McpTransport::HttpSse { url },
            ..Default::default()
        };
        let quick = std::time::Duration::from_millis(200);
        let connect = HttpConnection::connect_within(&config, quick);
        let error = tokio::time::timeout(std::time::Duration::from_secs(5), connect)
            .await
            .expect("given up on in time")
            .err()
            .expect("refused");
        assert!(matches!(error, McpHttpError::HandshakeTimeout(_)), "{error:?}");
        assert!(error.to_string().contains("did not finish its handshake"), "{error}");
        held.abort();
    }

    /// The engine sorts failures by kind and shows the text this transport wrote.
    #[test]
    fn an_http_error_is_sorted_by_what_went_wrong_and_keeps_its_text() {
        let rpc = McpError::from(McpHttpError::Rpc("boom".into()));
        assert!(matches!(&rpc, McpError::Rpc(text) if text == "boom"));
        let protocol = McpError::from(McpHttpError::Protocol("bad json: eof".into()));
        assert!(matches!(protocol, McpError::Protocol(_)));
        let expired = McpError::from(McpHttpError::SessionExpired("s1".into()));
        assert!(
            matches!(&expired, McpError::Closed(text) if text == "the server's session expired")
        );
        let status = McpHttpError::Status {
            method: "tools/call".into(),
            status: reqwest::StatusCode::BAD_GATEWAY,
        };
        let status = McpError::from(status);
        assert!(
            matches!(&status, McpError::Transport(text) if text == "tools/call returned HTTP 502 Bad Gateway")
        );
    }

    #[test]
    fn extract_result_surfaces_errors() {
        let ok = json!({ "jsonrpc": "2.0", "id": 1, "result": { "x": 1 } });
        assert_eq!(extract_result(&ok).unwrap(), json!({ "x": 1 }));
        let err = json!({ "jsonrpc": "2.0", "id": 1, "error": { "message": "boom" } });
        let error = extract_result(&err).unwrap_err();
        assert!(matches!(&error, McpHttpError::Rpc(message) if message == "boom"), "{error:?}");
    }

    #[tokio::test]
    async fn resolve_bearer_prefers_the_token_then_the_variable() {
        assert_eq!(resolve_bearer(Some("abc"), None, false, "s").await.unwrap(), "abc");
        std::env::set_var("LUXE_MCP_BEARER_ENV", "fromenv");
        let env = Some("LUXE_MCP_BEARER_ENV");
        assert_eq!(resolve_bearer(None, env, false, "s").await.unwrap(), "fromenv");
        assert_eq!(
            resolve_bearer(Some("${LUXE_MCP_BEARER_UNSET}"), env, false, "s").await.unwrap(),
            "fromenv",
            "an unset variable in `token` falls through to `token_env`"
        );
        std::env::remove_var("LUXE_MCP_BEARER_ENV");
        assert!(resolve_bearer(None, None, false, "s").await.is_err());
    }

    /// A `!command` token runs for your own config and is refused, not skipped, for a project's:
    /// falling through to `token_env` would hide that the config asked for a command.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_command_token_runs_only_for_your_own_config() {
        std::env::set_var("LUXE_MCP_BEARER_CMD_ENV", "fromenv");
        let env = Some("LUXE_MCP_BEARER_CMD_ENV");
        assert_eq!(resolve_bearer(Some("!echo t0k"), env, true, "s").await.unwrap(), "t0k");
        let refused =
            resolve_bearer(Some("!echo t0k"), env, false, "s").await.unwrap_err().to_string();
        assert!(refused.starts_with("the bearer token for 's' is a command"), "{refused}");
        std::env::remove_var("LUXE_MCP_BEARER_CMD_ENV");
    }
}
