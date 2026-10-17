//! Server registry — lazy MCP connections + a persisted tool-metadata cache.
//!
//! Connects servers on demand (or eagerly / keep-alive per config), remembers each server's tools
//! in a disk cache so `search`/`describe` work with no live connection, and idle-disconnects lazy
//! servers. All the state the [`crate::tool::McpProxyTool`] needs lives here.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use luxe_core::mcp_client::{
    McpAuth, McpConnection, McpError, McpLifecycle, McpServerConfig, McpTool, McpToolResult,
    McpTransport,
};
use luxe_core::mcp_stdio::McpStdioClient;

use crate::oauth::OAuthClient;

/// Default idle window before a lazy server disconnects.
const DEFAULT_IDLE: Duration = Duration::from_secs(10 * 60);

/// How long a call may take when its server does not say (`timeout_secs`).
const DEFAULT_CALL_TIMEOUT: Duration = luxe_core::tool::NETWORK_TOOL_TIMEOUT;

/// Why a tool could not be found, reached or finished.
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    /// No server offers a tool by that name
    #[error("unknown MCP tool '{0}' — use mcp({{ search: \"...\" }}) to discover tools")]
    UnknownTool(String),
    /// A bare name that two or more servers offer
    #[error("'{name}' is ambiguous: say which, one of {}", .candidates.join(", "))]
    Ambiguous {
        /// What was asked for
        name: String,
        /// Each server's `server/name` for it
        candidates: Vec<String>,
    },
    /// No server by that id is configured
    #[error("no such MCP server '{0}'")]
    NoSuchServer(String),
    /// The call outlived its server's deadline, so its server was stopped
    #[error(
        "`{tool}` did not answer within {secs}s, so its server was stopped with its process \
         group; it restarts on the next call. Anything it launched in a group of its own \
         — a nested agent's shell command — may still finish, so check before retrying. \
         A server that is slow by design can declare `timeout_secs` in its MCP config."
    )]
    TimedOut {
        /// What was called
        tool: String,
        /// The deadline it missed
        secs: u64,
    },
    /// The server could not be reached, or said no
    #[error(transparent)]
    Server(#[from] McpError),
}

/// A resolved tool: which server owns it, its server-side name, and its schema. `presented` is the
/// discovery-facing `server_id/raw_name` the agent passes back to `describe`/`call`.
#[derive(Debug, Clone)]
pub struct ToolRef {
    /// The server that owns the tool
    pub server_id: String,
    /// The tool's name on that server
    pub raw_name: String,
    /// `server_id/raw_name`, as the agent passes it back
    pub presented: String,
    /// What the server says the tool does
    pub description: String,
    /// The tool's JSON input schema, as the server declared it
    pub input_schema: serde_json::Value,
}

/// One server's cached tool list, tagged with the transport hash it was captured under (so a
/// changed command/url invalidates it).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedMeta {
    config_hash: u64,
    tools: Vec<McpTool>,
}

/// A live connection plus the bookkeeping for idle-disconnect.
struct Live {
    conn: Arc<dyn McpConnection>,
    last_used: Instant,
    idle: Option<Duration>,
}

/// Lazy MCP server manager: owns configs, live connections, and the metadata cache.
pub struct ServerRegistry {
    servers: Vec<McpServerConfig>,
    live: Mutex<HashMap<String, Live>>,
    /// One lock per server, held while connecting it
    connecting: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    metadata: Mutex<HashMap<String, CachedMeta>>,
    cache_path: Option<PathBuf>,
}

impl ServerRegistry {
    /// Build a manager over the enabled servers, persisting tool metadata to `cache_path` (e.g.
    /// `.luxe/mcp-metadata.json`). MCP *calls* are permission-gated at the engine dispatch
    /// choke point (keyed by the `mcp` tool's `server/name` target), so the manager itself does
    /// not re-gate.
    pub fn new(servers: Vec<McpServerConfig>, cache_path: Option<PathBuf>) -> Self {
        let metadata = cache_path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self {
            servers: servers.into_iter().filter(|s| s.enabled).collect(),
            live: Mutex::new(HashMap::new()),
            connecting: Mutex::new(HashMap::new()),
            metadata: Mutex::new(metadata),
            cache_path,
        }
    }

    /// Whether any servers are configured (the proxy tool registers only when true).
    pub fn has_servers(&self) -> bool {
        !self.servers.is_empty()
    }

    /// Server ids, for status surfaces.
    pub fn server_ids(&self) -> Vec<String> {
        self.servers.iter().map(|s| s.id.clone()).collect()
    }

    /// Connect eager/keep-alive servers now and spawn the idle sweeper. Lazy servers stay cold.
    pub async fn start(self: &Arc<Self>) {
        for server in &self.servers {
            if matches!(server.lifecycle, McpLifecycle::Eager | McpLifecycle::KeepAlive) {
                if let Err(error) = self.ensure_connected(&server.id).await {
                    tracing::warn!(server = %server.id, %error, "mcp: eager connect failed");
                }
            }
        }
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(30));
            loop {
                tick.tick().await;
                match weak.upgrade() {
                    Some(manager) => manager.sweep_idle().await,
                    None => break, // manager dropped — stop sweeping
                }
            }
        });
    }

    /// Search cached tool metadata across all servers, returning matches (name/description
    /// substring, case-insensitive; empty query = everything). Populates the cache lazily.
    pub async fn search(&self, query: &str) -> Vec<ToolRef> {
        let needle = query.trim().to_lowercase();
        let mut hits = Vec::new();
        for server in &self.servers {
            let refs = match self.tool_refs(server).await {
                Ok(refs) => refs,
                Err(error) => {
                    tracing::warn!(server = %server.id, %error, "mcp: metadata unavailable");
                    continue;
                }
            };
            hits.extend(refs.into_iter().filter(|r| matches(r, &needle)));
        }
        hits
    }

    /// Resolve `server_id/raw_name`, or a bare `raw_name` exactly one server offers, to its
    /// [`ToolRef`].
    ///
    /// A named server is the only one consulted, so calling its tool starts no other server just
    /// to list theirs. A bare name has to ask them all, and two answers are an error naming both.
    pub async fn describe(&self, name: &str) -> Result<ToolRef, RegistryError> {
        let unknown = || RegistryError::UnknownTool(name.to_string());
        if let Some((id, raw)) = name.split_once('/') {
            if let Some(server) = self.servers.iter().find(|server| server.id == id) {
                let refs = self.tool_refs(server).await?;
                return refs.into_iter().find(|r| r.raw_name == raw).ok_or_else(unknown);
            }
        }
        let mut found = Vec::new();
        for server in &self.servers {
            if let Ok(refs) = self.tool_refs(server).await {
                found
                    .extend(refs.into_iter().filter(|r| r.presented == name || r.raw_name == name));
            }
        }
        match found.len() {
            0 => Err(unknown()),
            1 => Ok(found.remove(0)),
            _ => {
                let candidates = found.into_iter().map(|r| r.presented).collect();
                Err(RegistryError::Ambiguous { name: name.to_string(), candidates })
            }
        }
    }

    /// Call a tool (resolved from `name`) with `arguments`. Connects the owning server lazily.
    /// Returns the server's result text (as data — never interpreted). Permission gating happens
    /// upstream at engine dispatch.
    ///
    /// Bounded by the server's `timeout_secs`. A call given up on — by that deadline, or because
    /// the turn was cancelled and this future dropped — is [abandoned](McpConnection::abandon),
    /// which for stdio stops the server's whole tree; the next call starts it fresh.
    pub async fn call(
        &self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<McpToolResult, RegistryError> {
        let target = self.describe(name).await?;
        let conn = self.ensure_connected(&target.server_id).await?;
        let deadline = self.call_timeout(&target.server_id);
        let mut abandon = AbandonOnDrop(Some(conn.clone()));
        let result =
            tokio::time::timeout(deadline, conn.call_tool(&target.raw_name, arguments)).await;
        match result {
            Ok(result) => {
                abandon.0 = None;
                Ok(result?)
            }
            Err(_) => {
                Err(RegistryError::TimedOut { tool: name.to_string(), secs: deadline.as_secs() })
            }
        }
    }

    /// The deadline for one call to `server_id`.
    fn call_timeout(&self, server_id: &str) -> Duration {
        self.servers
            .iter()
            .find(|server| server.id == server_id)
            .and_then(|server| server.timeout_secs)
            .filter(|secs| *secs > 0)
            .map_or(DEFAULT_CALL_TIMEOUT, Duration::from_secs)
    }

    /// The longest any configured server's call may take — the ceiling the engine's own deadline
    /// for the `mcp` tool has to clear, so the server's deadline is the one that fires.
    pub fn longest_call_timeout(&self) -> Duration {
        self.servers
            .iter()
            .map(|server| self.call_timeout(&server.id))
            .max()
            .unwrap_or(DEFAULT_CALL_TIMEOUT)
    }

    /// Cached (or freshly listed) tool refs for one server, honoring its `exclude_tools`.
    async fn tool_refs(&self, server: &McpServerConfig) -> Result<Vec<ToolRef>, RegistryError> {
        let tools = self.ensure_metadata(server).await?;
        Ok(tools
            .into_iter()
            .filter(|t| !server.exclude_tools.iter().any(|x| x == &t.name))
            .map(|t| ToolRef {
                presented: format!("{}/{}", server.id, t.name),
                server_id: server.id.clone(),
                raw_name: t.name,
                description: t.description,
                input_schema: t.input_schema,
            })
            .collect())
    }

    /// Return a server's tools from cache, or connect once to list + cache them (memory + disk).
    async fn ensure_metadata(
        &self,
        server: &McpServerConfig,
    ) -> Result<Vec<McpTool>, RegistryError> {
        let hash = transport_hash(&server.transport);
        if let Some(meta) = self.metadata.lock().await.get(&server.id) {
            if meta.config_hash == hash {
                return Ok(meta.tools.clone());
            }
        }
        let conn = self.ensure_connected(&server.id).await?;
        let tools = conn.list_tools().await?;
        self.metadata
            .lock()
            .await
            .insert(server.id.clone(), CachedMeta { config_hash: hash, tools: tools.clone() });
        self.persist_metadata().await;
        Ok(tools)
    }

    /// Get (or open) the live connection for a server, refreshing its idle clock.
    ///
    /// Connecting holds that server's own lock, so calls arriving together at a cold server wait
    /// for one connection instead of each starting a server and the last one replacing the rest.
    async fn ensure_connected(
        &self,
        server_id: &str,
    ) -> Result<Arc<dyn McpConnection>, RegistryError> {
        let connecting =
            self.connecting.lock().await.entry(server_id.to_string()).or_default().clone();
        let _connecting = connecting.lock().await;
        {
            let mut live = self.live.lock().await;
            // An abandoned connection is dead weight; replacing it is what "restarts on the next
            // call" means.
            if live.get(server_id).is_some_and(|entry| entry.conn.is_closed()) {
                live.remove(server_id);
            }
            if let Some(entry) = live.get_mut(server_id) {
                entry.last_used = Instant::now();
                return Ok(entry.conn.clone());
            }
        }
        let server = self
            .servers
            .iter()
            .find(|s| s.id == server_id)
            .ok_or_else(|| RegistryError::NoSuchServer(server_id.to_string()))?;
        let conn = open_connection(server).await?;
        let idle = match server.lifecycle {
            McpLifecycle::KeepAlive => None,
            _ => Some(
                server
                    .idle_timeout_mins
                    .map(|m| Duration::from_secs(m * 60))
                    .unwrap_or(DEFAULT_IDLE),
            ),
        };
        self.live.lock().await.insert(
            server_id.to_string(),
            Live { conn: conn.clone(), last_used: Instant::now(), idle },
        );
        Ok(conn)
    }

    /// Drop live connections idle past their timeout (keep-alive servers have `idle == None`).
    async fn sweep_idle(&self) {
        let now = Instant::now();
        self.live.lock().await.retain(|server_id, entry| {
            let expired = entry.idle.is_some_and(|idle| now.duration_since(entry.last_used) > idle);
            if expired {
                tracing::debug!(server = %server_id, "mcp: idle-disconnecting");
            }
            !expired // dropping the entry drops the connection → transport reaps the process
        });
    }

    /// Gracefully drop every live connection (used when leaving a project).
    pub async fn shutdown_all(&self) {
        self.live.lock().await.clear();
    }

    /// Build an [`OAuthClient`] for an OAuth-configured HTTP server (for the `mcp login` CLI flow).
    pub fn oauth_client(&self, server_id: &str) -> Option<OAuthClient> {
        let server = self.servers.iter().find(|s| s.id == server_id)?;
        let url = match &server.transport {
            McpTransport::HttpSse { url } => url.clone(),
            McpTransport::Stdio { .. } => return None,
        };
        match &server.auth {
            Some(McpAuth::OAuth(config)) => Some(
                OAuthClient::new(server.id.clone(), url, config.clone())
                    .with_secret_commands(server.declared_by_user),
            ),
            _ => None,
        }
    }

    async fn persist_metadata(&self) {
        let Some(path) = &self.cache_path else { return };
        let snapshot = self.metadata.lock().await.clone();
        if let Ok(json) = serde_json::to_string_pretty(&snapshot) {
            let path = path.clone();
            let written = tokio::task::spawn_blocking(move || {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                luxe_core::fs_atomic::replace(&path, json.as_bytes())
            });
            match written.await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => tracing::warn!(%error, "mcp: failed to persist metadata cache"),
                Err(error) => tracing::warn!(%error, "mcp: metadata cache write did not finish"),
            }
        }
    }
}

/// Abandons its connection when dropped armed: the call it guards was given up on.
struct AbandonOnDrop(Option<Arc<dyn McpConnection>>);

impl Drop for AbandonOnDrop {
    fn drop(&mut self) {
        if let Some(conn) = self.0.take() {
            tracing::warn!(server = %conn.server_id(), "mcp: abandoning a call; stopping its server");
            conn.abandon();
        }
    }
}

/// Open a live connection for a server's transport: stdio through core, HTTP through
/// [`crate::http`].
///
/// Public because the registry is not the only caller that needs a connection: `luxe mcp tools`
/// lists a server without registering it, and has to reach either transport the same way.
pub async fn open_connection(server: &McpServerConfig) -> Result<Arc<dyn McpConnection>, McpError> {
    match &server.transport {
        McpTransport::Stdio { .. } => {
            let client = McpStdioClient::connect(server).await?;
            Ok(Arc::new(client))
        }
        McpTransport::HttpSse { .. } => {
            let client = crate::http::HttpConnection::connect(server).await?;
            Ok(Arc::new(client))
        }
    }
}

/// Hash a transport config so a changed command/url invalidates the cached metadata.
///
/// FNV-1a over a spelled-out key rather than `DefaultHasher`, whose output std does not promise to
/// keep between releases: the hash is written to disk, and a new toolchain would otherwise empty
/// every cache it reads.
fn transport_hash(transport: &McpTransport) -> u64 {
    let key = match transport {
        McpTransport::Stdio { command, args, cwd, .. } => {
            let cwd = cwd.as_deref().map(std::path::Path::to_string_lossy).unwrap_or_default();
            format!("stdio\0{command}\0{}\0{cwd}", args.join("\0"))
        }
        McpTransport::HttpSse { url } => format!("http\0{url}"),
    };
    key.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// Whether a tool answers a search for `needle` (already lowercased): its name, what it says it
/// does, or the server it belongs to.
///
/// The server's id is what a reference to one names — `@[mcp:asana]` — so `search: "asana"` must
/// list that server's tools even when none of them happens to say the word.
fn matches(tool: &ToolRef, needle: &str) -> bool {
    needle.is_empty()
        || tool.raw_name.to_lowercase().contains(needle)
        || tool.description.to_lowercase().contains(needle)
        || tool.server_id.to_lowercase().contains(needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The hash a cache on disk was written under is the one a later build computes.
    ///
    /// The break this catches: `DefaultHasher`, which std may change between releases, so an
    /// upgrade emptied every server's cached tool list.
    #[test]
    fn a_transport_hashes_the_same_in_every_build() {
        let stdio = McpTransport::Stdio {
            command: "npx".into(),
            args: vec!["-y".into(), "asana-mcp".into()],
            cwd: None,
            env: Default::default(),
        };
        assert_eq!(transport_hash(&stdio), 0xcb0d_a7d1_8332_65b3);
        let McpTransport::Stdio { command, args, env, .. } = stdio.clone() else { unreachable!() };
        let placed = McpTransport::Stdio { command, args, cwd: Some("/srv/mcp".into()), env };
        assert_eq!(transport_hash(&placed), 0xfe38_673c_a328_0a9e);
        let http = McpTransport::HttpSse { url: "https://mcp.example/sse".into() };
        assert_ne!(transport_hash(&http), transport_hash(&stdio));
    }

    /// A search finds a tool by its name, by what it does, and by its server.
    #[test]
    fn a_search_matches_the_name_the_description_or_the_server() {
        let tool = ToolRef {
            server_id: "Asana".into(),
            raw_name: "get_tasks".into(),
            presented: "Asana/get_tasks".into(),
            description: "List the tasks in a project.".into(),
            input_schema: serde_json::Value::Null,
        };
        for needle in ["", "get_tasks", "project", "asana", "sana"] {
            assert!(matches(&tool, needle), "{needle:?}");
        }
        for needle in ["jira", "create"] {
            assert!(!matches(&tool, needle), "{needle:?}");
        }
    }

    fn manager_with(servers: Vec<McpServerConfig>) -> Arc<ServerRegistry> {
        Arc::new(ServerRegistry::new(servers, None))
    }

    #[tokio::test]
    async fn empty_manager_has_no_servers() {
        let manager = manager_with(vec![]);
        assert!(!manager.has_servers());
        assert!(manager.search("anything").await.is_empty());
    }

    #[tokio::test]
    async fn disabled_servers_are_filtered_out() {
        let manager = manager_with(vec![McpServerConfig {
            id: "off".into(),
            name: "Off".into(),
            enabled: false,
            ..Default::default()
        }]);
        assert!(!manager.has_servers());
    }

    /// A stdio server that spawns a grandchild on `slow` and never answers, and answers `fast`.
    #[cfg(unix)]
    const SLOW_SERVER: &str = r#"
import json, subprocess, sys
pidfile = sys.argv[1]
for line in sys.stdin:
    msg = json.loads(line)
    method, id = msg.get("method"), msg.get("id")
    if method == "initialize":
        result = {"protocolVersion": "2025-06-18", "capabilities": {"tools": {}}, "serverInfo": {"name": "slow"}}
    elif method == "tools/list":
        result = {"tools": [{"name": n, "description": n, "inputSchema": {"type": "object"}} for n in ("slow", "fast")]}
    elif method == "tools/call" and msg["params"]["name"] == "slow":
        child = subprocess.Popen(["sleep", "60"])
        open(pidfile, "w").write(str(child.pid))
        continue
    elif method == "tools/call":
        result = {"content": [{"type": "text", "text": "fast"}]}
    else:
        continue
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": id, "result": result}) + "\n")
    sys.stdout.flush()
"#;

    /// A call past its server's deadline stops the server *and what it started*, and the next call
    /// gets a fresh server.
    ///
    /// Against a real model, a delegating server's subagent went on writing files after the model
    /// was told the call "was stopped" — and after the session had exited, because killing the
    /// server left its child behind.
    #[cfg(unix)]
    #[tokio::test]
    async fn an_abandoned_call_stops_the_whole_server_tree() {
        if std::process::Command::new("python3").arg("--version").output().is_err() {
            eprintln!("skipping: python3 not available");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("slow.py");
        let pidfile = dir.path().join("grandchild.pid");
        std::fs::write(&script, SLOW_SERVER).unwrap();
        let manager = manager_with(vec![McpServerConfig {
            id: "srv".into(),
            name: "srv".into(),
            enabled: true,
            timeout_secs: Some(1),
            transport: McpTransport::Stdio {
                command: "python3".into(),
                args: vec![script.display().to_string(), pidfile.display().to_string()],
                cwd: None,
                env: Default::default(),
            },
            ..Default::default()
        }]);

        let error = manager.call("srv/slow", serde_json::json!({})).await.unwrap_err();
        assert!(matches!(&error, RegistryError::TimedOut { secs: 1, .. }), "{error:?}");
        assert!(error.to_string().contains("did not answer within 1s"), "{error}");
        let grandchild: u32 = std::fs::read_to_string(&pidfile).unwrap().trim().parse().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while luxe_core::program::process_is_alive(grandchild) && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(!luxe_core::program::process_is_alive(grandchild), "the grandchild outlived it");

        let result = manager.call("srv/fast", serde_json::json!({})).await.unwrap();
        assert_eq!(result.output.as_deref(), Some("fast"), "a fresh server answered");
    }

    #[tokio::test]
    async fn transport_hash_changes_with_command() {
        let a = transport_hash(&McpTransport::Stdio {
            command: "a".into(),
            args: vec![],
            cwd: None,
            env: Default::default(),
        });
        let b = transport_hash(&McpTransport::Stdio {
            command: "b".into(),
            args: vec![],
            cwd: None,
            env: Default::default(),
        });
        assert_ne!(a, b);
    }
}
