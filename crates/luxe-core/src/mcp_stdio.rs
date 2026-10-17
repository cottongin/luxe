//! Real MCP stdio transport — JSON-RPC 2.0 over a child process's stdin/stdout.
//!
//! Speaks the Model Context Protocol to a locally-spawned server: `initialize` handshake,
//! `notifications/initialized`, then `tools/list` and `tools/call`. Messages are
//! newline-delimited JSON (the MCP stdio framing), one request in flight at a time
//! (calls are serialized behind a mutex). HTTP/SSE transport is deliberately **not** here
//! — `core` carries no HTTP deps; a future non-core crate can add it.
//!
//! MCP servers are untrusted: tool descriptions and results are treated strictly as data,
//! never as instructions.

use std::sync::atomic::{AtomicI64, Ordering};

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;
use tracing::{debug, warn};

use crate::mcp_client::{McpError, McpServerConfig, McpTool, McpToolResult, McpTransport};

/// The longest line a server may send; past it the connection is refused rather than buffered.
///
/// The same bound a language server's frames get ([`crate::jsonrpc_frame::MAX_FRAME_BYTES`]): a
/// tool result that carries an image is one base64 line, and a megabyte is a screenshot.
const MAX_LINE_BYTES: u64 = crate::jsonrpc_frame::MAX_FRAME_BYTES as u64;

/// How much of a stderr line is read at once. A server's stderr is only kept for its last words, so
/// it is not given the room its answers are.
const STDERR_READ_BYTES: u64 = 1024 * 1024;

/// How many of a server's last stderr lines an error carries, and how much of each.
const STDERR_LINES: usize = 20;
const STDERR_LINE_CHARS: usize = 1024;

/// Errors from the stdio MCP transport.
#[derive(Debug, thiserror::Error)]
pub enum McpStdioError {
    #[error("MCP transport is not stdio for server '{0}'")]
    NotStdio(String),
    #[error("spawn failed: {0}")]
    Spawn(String),
    #[error("I/O error: {0}")]
    Io(std::io::Error),
    #[error("JSON error: {0}")]
    Json(serde_json::Error),
    #[error("server closed the connection{}", said(.0))]
    Closed(String),
    #[error("server returned an error: {0}")]
    Rpc(String),
    #[error("unexpected response: {0}")]
    Protocol(String),
}

crate::error_cause::from_cause!(McpStdioError {
    std::io::Error => Io,
    serde_json::Error => Json,
});

impl From<McpStdioError> for McpError {
    /// Sorted by what went wrong; the text stays this transport's own.
    fn from(error: McpStdioError) -> Self {
        let text = error.to_string();
        match error {
            McpStdioError::Rpc(_) => Self::Rpc(text),
            McpStdioError::Json(_) | McpStdioError::Protocol(_) => Self::Protocol(text),
            McpStdioError::Closed(_) => Self::Closed(text),
            McpStdioError::NotStdio(_) | McpStdioError::Spawn(_) | McpStdioError::Io(_) => {
                Self::Transport(text)
            }
        }
    }
}

/// What a server last wrote to stderr, as an error's tail: a server that dies in its handshake
/// usually says why there.
fn said(stderr: &str) -> String {
    match stderr.trim() {
        "" => String::new(),
        lines => format!("; it last said:\n{lines}"),
    }
}

/// The pipes + child for one live stdio connection.
struct Conn {
    /// Held to keep the server process alive; `kill_on_drop` reaps it when `Conn` drops.
    child: Child,
    stdin: ChildStdin,
    reader: BufReader<ChildStdout>,
    /// The server's last stderr lines, kept by a task that drains the pipe
    stderr: std::sync::Arc<std::sync::Mutex<std::collections::VecDeque<String>>>,
    /// That task, waited on once the server has closed, so its last lines are in
    stderr_drain: Option<tokio::task::JoinHandle<()>>,
}

impl Conn {
    /// The error for a server that closed its end, with what it last wrote to stderr.
    async fn closed(&mut self) -> McpStdioError {
        // the drain task may still be reading the last lines of a server that just exited
        let settle = std::time::Duration::from_millis(200);
        let _ = tokio::time::timeout(settle, self.child.wait()).await;
        if let Some(drain) = self.stderr_drain.take() {
            let _ = tokio::time::timeout(settle, drain).await;
        }
        let lines = self.stderr.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        McpStdioError::Closed(lines.iter().cloned().collect::<Vec<_>>().join("\n"))
    }
}

/// The last lines a server wrote to stderr, and the task draining the pipe into them.
type StderrTail = (
    std::sync::Arc<std::sync::Mutex<std::collections::VecDeque<String>>>,
    tokio::task::JoinHandle<()>,
);

/// Keeps the last [`STDERR_LINES`] lines of `stderr`, so the pipe never fills and stalls the server.
///
/// Read to the end whatever the server writes: a line past the bound arrives in pieces, bytes that
/// are not UTF-8 are replaced, and only end of file or a read error stops it, since a closed read
/// end would make the server's next write fail.
fn keep_stderr_tail(stderr: tokio::process::ChildStderr) -> StderrTail {
    let tail = std::sync::Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new()));
    let kept = std::sync::Arc::clone(&tail);
    let drain = tokio::spawn(async move {
        let mut reader = BufReader::new(stderr);
        loop {
            let mut line = Vec::new();
            match (&mut reader).take(STDERR_READ_BYTES).read_until(b'\n', &mut line).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let text = String::from_utf8_lossy(&line);
            let text =
                text.trim_end_matches(['\n', '\r']).chars().take(STDERR_LINE_CHARS).collect();
            let mut kept = kept.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if kept.len() == STDERR_LINES {
                kept.pop_front();
            }
            kept.push_back(text);
        }
    });
    (tail, drain)
}

/// A connected MCP server reached over stdio. Cheap to `Arc`-share; calls serialize.
pub struct McpStdioClient {
    server_id: String,
    conn: Mutex<Conn>,
    next_id: AtomicI64,
    /// The server's process group, for stopping what it spawned as well as the server itself.
    pid: u32,
    closed: std::sync::atomic::AtomicBool,
}

impl McpStdioClient {
    /// Stop the server and everything in its process group — if the server is still there.
    ///
    /// A server that already exited is left alone: its group id is free to be reused once the
    /// group empties, and signalling a reused one would kill somebody else's processes. A call in
    /// flight holds the lock, and a server with a call in flight is running.
    fn kill_group_if_running(&self) {
        let running = match self.conn.try_lock() {
            Ok(mut conn) => matches!(conn.child.try_wait(), Ok(None)),
            Err(_) => true,
        };
        if running {
            crate::program::kill_tree(self.pid);
        }
    }
}

impl Drop for McpStdioClient {
    /// `kill_on_drop` reaches only the server. A server that delegates spawns work of its own —
    /// a headless Luxe, a browser — and that went on running, writing files, after the session
    /// that asked for it had exited.
    fn drop(&mut self) {
        self.kill_group_if_running();
    }
}

impl McpStdioClient {
    /// Spawn the server and complete the MCP `initialize` handshake.
    pub async fn connect(config: &McpServerConfig) -> Result<Self, McpStdioError> {
        let (command, args, cwd, env) = match &config.transport {
            McpTransport::Stdio { command, args, cwd, env } => (command, args, cwd, env),
            McpTransport::HttpSse { .. } => return Err(McpStdioError::NotStdio(config.id.clone())),
        };

        let mut cmd = Command::new(command);
        cmd.args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        crate::program::detach_from_terminal(&mut cmd);
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }
        if !env.is_empty() {
            cmd.envs(env.iter().map(|(k, v)| (k.as_str(), crate::mcp_client::expand_env(v))));
        }

        let mut child = cmd.spawn().map_err(|e| McpStdioError::Spawn(e.to_string()))?;
        let pid = child.id().unwrap_or(0);
        let unpiped = || McpStdioError::Closed(String::new());
        let stdin = child.stdin.take().ok_or_else(unpiped)?;
        let stdout = child.stdout.take().ok_or_else(unpiped)?;
        let (stderr, drain) = keep_stderr_tail(child.stderr.take().ok_or_else(unpiped)?);
        let reader = BufReader::new(stdout);
        let stderr_drain = Some(drain);

        let client = Self {
            server_id: config.id.clone(),
            conn: Mutex::new(Conn { child, stdin, reader, stderr, stderr_drain }),
            next_id: AtomicI64::new(1),
            pid,
            closed: std::sync::atomic::AtomicBool::new(false),
        };

        client.handshake().await?;
        Ok(client)
    }

    /// The server id this client speaks to.
    pub fn server_id(&self) -> &str {
        &self.server_id
    }

    /// `initialize` request + `notifications/initialized`.
    async fn handshake(&self) -> Result<(), McpStdioError> {
        let params = json!({
            "protocolVersion": crate::mcp_client::MCP_PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": { "name": "luxe", "version": crate::version::VERSION },
        });
        let mut conn = self.conn.lock().await;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let _ = self.rpc(&mut conn, id, "initialize", params).await?;
        // MCP requires the client to confirm before issuing further requests.
        write_message(
            &mut conn.stdin,
            &json!({
                "jsonrpc": "2.0",
                "method": "notifications/initialized",
            }),
        )
        .await?;
        Ok(())
    }

    /// List the server's tools (raw, un-namespaced names).
    pub async fn list_tools(&self) -> Result<Vec<McpTool>, McpStdioError> {
        let mut conn = self.conn.lock().await;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let result = self.rpc(&mut conn, id, "tools/list", json!({})).await?;

        crate::mcp_client::parse_tool_list(&result, &self.server_id)
            .ok_or_else(|| McpStdioError::Protocol("tools/list missing `tools` array".into()))
    }

    /// Call a tool by its raw (server-side) name.
    pub async fn call_tool(
        &self,
        raw_name: &str,
        arguments: Value,
    ) -> Result<McpToolResult, McpStdioError> {
        let mut conn = self.conn.lock().await;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let params = json!({ "name": raw_name, "arguments": arguments });
        let result = self.rpc(&mut conn, id, "tools/call", params).await?;
        Ok(crate::mcp_client::parse_tool_call_result(&result))
    }

    /// Send one request and read until the matching response id, skipping notifications
    /// and any stray non-matching messages.
    async fn rpc(
        &self,
        conn: &mut Conn,
        id: i64,
        method: &str,
        params: Value,
    ) -> Result<Value, McpStdioError> {
        let req = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        if let Err(error) = write_message(&mut conn.stdin, &req).await {
            return Err(match error {
                McpStdioError::Io(_) => conn.closed().await,
                other => other,
            });
        }

        loop {
            let mut line = String::new();
            // Bounded: an untrusted server that never writes a newline must not grow this forever
            let n = (&mut conn.reader).take(MAX_LINE_BYTES).read_line(&mut line).await?;
            if n == 0 {
                return Err(conn.closed().await);
            }
            if !line.ends_with('\n') && n as u64 == MAX_LINE_BYTES {
                // the rest of the line is still in the pipe, so nothing after it can be trusted
                self.closed.store(true, Ordering::Relaxed);
                crate::program::kill_tree(self.pid);
                return Err(McpStdioError::Protocol(format!(
                    "a message longer than {MAX_LINE_BYTES} bytes"
                )));
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let msg: Value = match serde_json::from_str(trimmed) {
                Ok(v) => v,
                Err(e) => {
                    let raw: String = trimmed.chars().take(200).collect();
                    warn!(server = %self.server_id, error = %e, %raw, "mcp: bad json line");
                    continue;
                }
            };
            // Skip notifications and responses to other ids.
            match msg.get("id").and_then(Value::as_i64) {
                Some(mid) if mid == id => {
                    if let Some(err) = msg.get("error") {
                        let message = err
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown error")
                            .to_string();
                        return Err(McpStdioError::Rpc(message));
                    }
                    return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
                }
                _ => {
                    debug!(server = %self.server_id, "mcp: skipping unrelated message");
                }
            }
        }
    }
}

#[async_trait::async_trait]
impl crate::mcp_client::McpConnection for McpStdioClient {
    fn server_id(&self) -> &str {
        &self.server_id
    }

    async fn list_tools(&self) -> Result<Vec<McpTool>, McpError> {
        Ok(McpStdioClient::list_tools(self).await?)
    }

    async fn call_tool(&self, raw_name: &str, arguments: Value) -> Result<McpToolResult, McpError> {
        Ok(McpStdioClient::call_tool(self, raw_name, arguments).await?)
    }

    fn abandon(&self) {
        self.closed.store(true, Ordering::Relaxed);
        self.kill_group_if_running();
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }
}

/// Write a single newline-delimited JSON-RPC message and flush.
async fn write_message(stdin: &mut ChildStdin, msg: &Value) -> Result<(), McpStdioError> {
    let mut buf = serde_json::to_vec(msg)?;
    buf.push(b'\n');
    stdin.write_all(&buf).await?;
    stdin.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests;
