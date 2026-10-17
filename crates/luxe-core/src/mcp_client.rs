//! MCP (Model Context Protocol) client types.
//!
//! Provides types for connecting to MCP servers via stdio or HTTP/SSE,
//! listing tools, and calling them.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// When an MCP server's process/connection is started and stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum McpLifecycle {
    /// Don't connect at startup; connect on the first tool call, disconnect after idle. Cached
    /// metadata keeps discovery working without a live connection.
    #[default]
    Lazy,
    /// Connect at startup; no auto-reconnect and no idle disconnect unless `idle_timeout_mins` is set.
    Eager,
    /// Connect at startup and keep alive (health-checked, auto-reconnect); never idle-disconnect.
    KeepAlive,
}

/// Transport type for MCP connections.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum McpTransport {
    /// Stdio transport — communicates with a local server process via stdin/stdout.
    Stdio {
        /// Command to run.
        command: String,
        /// Arguments to pass to the command.
        args: Vec<String>,
        /// Working directory for the process.
        cwd: Option<PathBuf>,
        /// Extra environment variables for the child process. Values support `${VAR}` and
        /// `$env:VAR` interpolation against Luxe's own environment.
        #[serde(default)]
        env: std::collections::HashMap<String, String>,
    },
    /// HTTP/SSE transport — communicates with a remote server via HTTP.
    HttpSse {
        /// Server URL.
        url: String,
    },
}

impl Default for McpTransport {
    fn default() -> Self {
        Self::Stdio {
            command: String::new(),
            args: Vec::new(),
            cwd: None,
            env: std::collections::HashMap::new(),
        }
    }
}

/// Configuration for an MCP server.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Unique identifier for this server.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Transport configuration.
    pub transport: McpTransport,
    /// Whether this server is enabled.
    pub enabled: bool,
    /// When to start/stop the connection (default: lazy).
    #[serde(default)]
    pub lifecycle: McpLifecycle,
    /// Minutes of inactivity before a lazy server disconnects (`None` = the manager default).
    #[serde(default)]
    pub idle_timeout_mins: Option<u64>,
    /// Seconds one tool call may take before it is abandoned (`None` = the network default, 60).
    ///
    /// Abandoning a stdio call stops the server's whole process tree and reconnects on next use,
    /// so the work really stops — a server that delegates to a subagent would otherwise go on
    /// writing after the model was told the call had ended. A server whose calls are slow by
    /// nature, like that delegate, declares how slow here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    /// Tool names to hide from this server, as the server spells them (`get_project_internal`,
    /// not `asana/get_project_internal`)
    #[serde(default)]
    pub exclude_tools: Vec<String>,
    /// Extra HTTP headers (HTTP transport only); values support `${VAR}` interpolation.
    #[serde(default)]
    pub headers: std::collections::HashMap<String, String>,
    /// Authentication for HTTP transports (bearer or, later, OAuth).
    #[serde(default)]
    pub auth: Option<McpAuth>,
    /// Declared in the reader's own config rather than a project's, which is what lets a
    /// `!command` secret run. Set by the store from where the server was found; never read from,
    /// or written to, a file — a config cannot vouch for itself.
    #[serde(skip)]
    pub declared_by_user: bool,
}

/// Authentication for an HTTP MCP server. Stdio servers ignore it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum McpAuth {
    /// Static bearer token — the literal `token` (supports `${VAR}`) or the value of `token_env`.
    Bearer {
        /// Literal token value (supports `${VAR}` interpolation).
        #[serde(default)]
        token: Option<String>,
        /// Environment variable to read the token from.
        #[serde(default)]
        token_env: Option<String>,
    },
    /// OAuth 2.1. Endpoints and client id are optional — missing pieces are filled by
    /// `.well-known` discovery and dynamic client registration (RFC 7591).
    ///
    /// Spelled `oauth`, as every example in the docs writes it. `snake_case` alone made it
    /// `o_auth`, so a hand-written `"kind": "oauth"` failed to parse and took the whole file with it;
    /// `o_auth` is still read, since that is what Luxe itself would have written.
    #[serde(rename = "oauth", alias = "o_auth")]
    OAuth(OAuthConfig),
}

/// OAuth grant type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OAuthGrant {
    /// Interactive browser flow with PKCE (default).
    #[default]
    AuthorizationCode,
    /// Non-interactive machine-to-machine flow.
    ClientCredentials,
}

/// OAuth client configuration for an MCP server. All endpoint/client fields are optional so a bare
/// `{ "kind": "oauth" }` works when the server supports discovery + dynamic registration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OAuthConfig {
    /// Grant type (default: authorization_code).
    #[serde(default)]
    pub grant_type: OAuthGrant,
    /// Pre-registered client id; dynamic registration is used when omitted.
    #[serde(default)]
    pub client_id: Option<String>,
    /// Client secret for confidential clients, as written: `${VAR}` and `$env:VAR` are expanded when
    /// it is sent, so the file can name an environment variable rather than hold the secret.
    #[serde(default)]
    pub client_secret: Option<ConfigSecret>,
    /// Requested scopes (space-joined on the wire).
    #[serde(default)]
    pub scope: Option<String>,
    /// Authorization endpoint; discovered from `.well-known` when omitted.
    #[serde(default)]
    pub authorization_endpoint: Option<String>,
    /// Token endpoint; discovered from `.well-known` when omitted.
    #[serde(default)]
    pub token_endpoint: Option<String>,
    /// Dynamic client registration endpoint; discovered when omitted.
    #[serde(default)]
    pub registration_endpoint: Option<String>,
    /// Exact loopback redirect URI to advertise (for providers that pre-register it). A lazy
    /// OS-assigned localhost port is used when omitted.
    #[serde(default)]
    pub redirect_uri: Option<String>,
}

/// A secret as written in an MCP config file: hidden from `Debug`, written back exactly as read.
///
/// Redacting it on `Serialize` instead would be a data-loss bug, not a hardening: `luxe mcp add`
/// parses the whole file and writes it back, so a redacting serializer would replace every other
/// server's secret with the placeholder.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ConfigSecret(String);

impl ConfigSecret {
    /// The secret exactly as the file spells it, `${VAR}` references unexpanded.
    pub fn as_written(&self) -> &str {
        &self.0
    }
}

impl From<&str> for ConfigSecret {
    fn from(written: &str) -> Self {
        Self(written.to_owned())
    }
}

impl From<String> for ConfigSecret {
    fn from(written: String) -> Self {
        Self(written)
    }
}

impl std::fmt::Debug for ConfigSecret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ConfigSecret([redacted])")
    }
}

/// Why a call to an MCP server did not come back with an answer.
///
/// What a [`McpConnection`] reports, whichever transport it speaks. The variant says what kind of
/// failure it was; each carries the text for the reader, which a caller shows as it is.
#[derive(Debug, thiserror::Error)]
pub enum McpError {
    /// The server could not be started or reached, or the exchange with it broke
    #[error("{0}")]
    Transport(String),
    /// The server answered with a JSON-RPC error
    #[error("{0}")]
    Rpc(String),
    /// The server's answer was not MCP
    #[error("{0}")]
    Protocol(String),
    /// The connection is gone: the server exited, or the call was abandoned
    #[error("{0}")]
    Closed(String),
}

/// A transport-agnostic live connection to an MCP server, so the orchestration layer can drive
/// stdio and HTTP servers uniformly. Implemented by each transport (stdio lives in core; HTTP in
/// `luxe-mcp`, which carries the HTTP dependency core forbids).
#[async_trait::async_trait]
pub trait McpConnection: Send + Sync {
    /// The server id this connection speaks to.
    fn server_id(&self) -> &str;
    /// List the server's tools (raw, un-namespaced names).
    async fn list_tools(&self) -> Result<Vec<McpTool>, McpError>;
    /// Call a tool by its raw (server-side) name.
    async fn call_tool(
        &self,
        raw_name: &str,
        arguments: serde_json::Value,
    ) -> Result<McpToolResult, McpError>;
    /// Stop whatever the server is doing for a call nobody is waiting on any more.
    ///
    /// Stdio kills the server's process tree, since a call cannot be withdrawn from one pipe
    /// without the others; HTTP has nothing local to stop. Called from `Drop`, so it must not
    /// block on the runtime.
    fn abandon(&self) {}
    /// Whether [`McpConnection::abandon`] (or anything else) has left this connection unusable.
    fn is_closed(&self) -> bool {
        false
    }
}

/// A tool provided by an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    /// Server ID that provides this tool.
    pub server_id: String,
    /// Tool name, as the server spells it; `mcp` presents it as `server_id/name`
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// JSON Schema for the tool's input parameters.
    pub input_schema: serde_json::Value,
}

/// A call to an MCP tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolCall {
    /// The tool being called.
    pub tool: McpTool,
    /// Input arguments for the tool call.
    pub arguments: serde_json::Value,
}

/// Result of an MCP tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolResult {
    /// Whether the call was successful.
    pub success: bool,
    /// Output text (if successful).
    pub output: Option<String>,
    /// Error message (if failed).
    pub error: Option<String>,
}

/// The MCP protocol version Luxe speaks, on both transports and the OAuth probe.
pub const MCP_PROTOCOL_VERSION: &str = "2025-06-18";

/// Expands `${VAR}` and `$env:VAR` from the environment, left to right, whichever marker comes
/// first; what a variable held is never read again. An unset variable becomes empty, with a
/// warning naming it, so a half-configured header degrades rather than sending the literal.
pub fn expand_env(input: &str) -> String {
    expand(input, false)
}

/// [`expand_env`], and a bare `$NAME` as well — left as written when unset, since a key may hold
/// a `$` of its own. What `models.json` values take.
pub fn expand_env_with_bare(input: &str) -> String {
    expand(input, true)
}

fn expand(input: &str, bare: bool) -> String {
    let name_end = |text: &str| {
        text.find(|c: char| !c.is_ascii_alphanumeric() && c != '_').unwrap_or(text.len())
    };
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(at) = rest.find('$') {
        out.push_str(&rest[..at]);
        let marker = &rest[at..];
        let (name, skip, keep_unset) = if let Some(body) = marker.strip_prefix("${") {
            match body.find('}') {
                Some(end) => (&body[..end], end + 3, false),
                None => ("", 0, true),
            }
        } else if let Some(body) = marker.strip_prefix("$env:") {
            let end = name_end(body);
            (&body[..end], end + 5, false)
        } else if bare && marker[1..].starts_with(|c: char| c.is_alphabetic() || c == '_') {
            let end = name_end(&marker[1..]);
            (&marker[1..=end], end + 1, true)
        } else {
            ("", 0, true)
        };
        if skip == 0 {
            out.push('$');
            rest = &marker[1..];
            continue;
        }
        match std::env::var(name) {
            Ok(value) => out.push_str(&value),
            Err(_) if keep_unset => out.push_str(&marker[..skip]),
            Err(_) => tracing::warn!(
                variable = name,
                "a configured variable is not set; it expands to nothing"
            ),
        }
        rest = &marker[skip..];
    }
    out.push_str(rest);
    out
}

/// Whether `name` — a server id, or a tool's name — is safe to put in front of the model and the
/// reader as it is: `[A-Za-z0-9._-]`, 1 to 64 characters.
///
/// A project chooses its own server ids and a server chooses its tools' names, and both end up in
/// a picker row and in the text a reference sends. A name that could hold a space could hold a
/// sentence, and a sentence in that position reads as an instruction; one that could hold a control
/// character could redraw the terminal. Anything else is not listed, and not repeated.
pub fn is_plain_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// Reads a `tools/list` result into [`McpTool`]s attributed to `server_id`.
///
/// The MCP shape is `{ "tools": [ { "name": …, "description": …, "inputSchema": … } ] }`. Entries
/// without a name are skipped rather than failing the batch — one malformed tool should not cost a
/// server all of its others. The names stay raw; namespacing is the caller's job.
///
/// Shared by both transports: the wire shape belongs to the protocol, not to stdio or HTTP.
pub fn parse_tool_list(result: &serde_json::Value, server_id: &str) -> Option<Vec<McpTool>> {
    let tools = result.get("tools")?.as_array()?;
    Some(
        tools
            .iter()
            .filter_map(|tool| {
                Some(McpTool {
                    server_id: server_id.to_string(),
                    name: tool.get("name")?.as_str()?.to_string(),
                    description: tool
                        .get("description")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    input_schema: tool
                        .get("inputSchema")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                })
            })
            .collect(),
    )
}

/// Reads a `tools/call` result into an [`McpToolResult`], shaped for the model.
///
/// The MCP shape is `{ "content": [ { "type": "text", "text": … } ], "isError": bool }`; a missing
/// `isError` means success, per the specification's default. What the model is sent is shaped once,
/// here, where both transports meet:
///
/// - each text block that is a JSON document loses its insignificant whitespace
///   ([`crate::output_limit::compact_json`]) — the indentation servers send is a third or more of
///   it, and nothing else changes;
/// - a block this surface cannot show is named at the end rather than dropped without a word, so
///   the model knows an image or a resource was there;
/// - the whole is capped at [`crate::output_limit::RESULT_MAX_CHARS`], the ceiling every tool
///   promises, with a note saying how to see more. An unbounded MCP result would stay in the
///   conversation for the rest of the session.
pub fn parse_tool_call_result(result: &serde_json::Value) -> McpToolResult {
    let blocks = result.get("content").and_then(serde_json::Value::as_array);
    let mut texts = Vec::new();
    let mut unseen: Vec<(&str, usize)> = Vec::new();
    for block in blocks.into_iter().flatten() {
        match block.get("text").and_then(serde_json::Value::as_str) {
            Some(text) => texts
                .push(crate::output_limit::compact_json(text).unwrap_or_else(|| text.to_owned())),
            None => {
                let kind = block.get("type").and_then(serde_json::Value::as_str).unwrap_or("");
                match unseen.iter_mut().find(|(seen, _)| *seen == kind) {
                    Some((_, count)) => *count += 1,
                    None => unseen.push((kind, 1)),
                }
            }
        }
    }
    let mut text = texts.join("\n");
    if !unseen.is_empty() {
        let note = format!("({} not shown)", named_blocks(&unseen));
        text = if text.is_empty() { note } else { format!("{text}\n{note}") };
    }
    let text = crate::output_limit::cap_chars(
        &text,
        crate::output_limit::RESULT_MAX_CHARS,
        "Narrow the call — a filter, fewer fields, a smaller page, or one item — to see the rest.",
    );

    if result.get("isError").and_then(serde_json::Value::as_bool).unwrap_or(false) {
        McpToolResult { success: false, output: None, error: Some(text) }
    } else {
        McpToolResult { success: true, output: Some(text), error: None }
    }
}

/// `1 image and 2 embedded resources`: what a result held that it could not show, in the words a
/// reader would use.
fn named_blocks(unseen: &[(&str, usize)]) -> String {
    let named: Vec<String> = unseen
        .iter()
        .map(|(kind, count)| {
            let noun = match *kind {
                "image" => "image",
                "audio" => "audio clip",
                "resource" => "embedded resource",
                "resource_link" => "resource link",
                _ => "block of another kind",
            };
            let noun = match (*count, noun) {
                (1, noun) => noun.to_string(),
                (_, "block of another kind") => "blocks of another kind".to_string(),
                (_, noun) => format!("{noun}s"),
            };
            format!("{count} {noun}")
        })
        .collect();
    match named.as_slice() {
        [only] => only.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
        [] => String::new(),
    }
}

/// State of an MCP server connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum McpServerState {
    /// Not connected.
    Disconnected,
    /// Connecting...
    Connecting,
    /// Connected and ready.
    Connected {
        /// Server name.
        name: String,
        /// Version reported by the server.
        version: Option<String>,
    },
    /// Connection error.
    Error(String),
}

/// Summary of an MCP server.
#[derive(Debug, Clone)]
pub struct McpServerSummary {
    /// Server configuration.
    pub config: McpServerConfig,
    /// Current connection state.
    pub state: McpServerState,
    /// Tools available from this server.
    pub tools: Vec<McpTool>,
}

#[cfg(test)]
mod tests;
