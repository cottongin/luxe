//! Workspace tooling — what language servers, debug adapters and MCP servers are doing right now.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Merged LSP-server + debug-adapter status for the `/lsp` and `/debug` views.
///
/// Built by the runtime collector each snapshot, so it reflects config edits, binaries installed
/// mid-session, and which servers/sessions are live right now — a single always-fresh source of
/// truth rather than a value captured at startup.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolingSummary {
    /// Configured language servers (built-in + user config), sorted by language.
    pub servers: Vec<LanguageServerStatus>,
    /// Configured debug adapters (built-in + user config).
    pub adapters: Vec<DebugAdapterStatus>,
    /// Currently live debug sessions.
    pub debug_sessions: Vec<DebugSessionStatus>,
    /// Configured MCP servers (merged across config sources).
    pub mcp_servers: Vec<McpServerStatus>,
    /// Every tool the model can reach, and why — active, deferred, or disabled. Settled when the
    /// registry is assembled, so unlike the rest of this summary it does not change per snapshot.
    #[serde(default)]
    pub tools: Vec<ToolStatus>,
    /// Path to `lsp-servers.json` (where to add/override a server).
    pub lsp_config_path: Option<PathBuf>,
    /// Path to `debug-adapters.json` (where to add/override an adapter).
    pub adapters_config_path: Option<PathBuf>,
}

impl ToolingSummary {
    /// Whether any tooling is configured (always true in practice — built-ins ship by default).
    pub fn is_empty(&self) -> bool {
        self.servers.is_empty() && self.adapters.is_empty()
    }
}

/// One language server's configuration + live status.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanguageServerStatus {
    /// Language id (e.g. `rust`)
    pub language: String,
    /// Launch command (the server binary)
    pub command: String,
    /// File extensions this server handles (no leading dot)
    pub extensions: Vec<String>,
    /// Whether this came from user config (vs a shipped default)
    pub from_config: bool,
    /// Install command shown when the binary is missing
    pub install_hint: Option<String>,
    /// Whether the binary resolves on PATH right now
    pub installed: bool,
    /// Whether a warm client is currently running for this language in the project
    pub running: bool,
}

/// One debug adapter's configuration + live status.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DebugAdapterStatus {
    /// Adapter id (e.g. `debugpy`)
    pub id: String,
    /// Adapter command (program + args, joined for display)
    pub command: String,
    /// Source extensions that route to this adapter (empty = compiled binaries)
    pub extensions: Vec<String>,
    /// Whether this came from user config (vs a shipped default)
    pub from_config: bool,
    /// Whether the adapter binary resolves on PATH right now
    pub installed: bool,
}

/// Why a tool is or is not callable right now, for the `/tools` view.
///
/// A model that will not search the web might have had the tool taken away, or might merely need to
/// activate it first. Telling a user to check their config when the real answer is
/// "it is one round-trip away" is worse than saying nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ToolAvailability {
    /// Callable, and its schema ships in every request.
    #[default]
    Active,
    /// Registered and callable, but deferred behind `activate_tools` to keep the request small.
    Deferred,
    /// Unregistered on purpose. `source` says what did it, so the user knows which file to edit.
    Disabled {
        /// Where the decision came from, e.g. `config` or `--no-tool`.
        source: String,
    },
}

/// One tool's name and why it is in the state it is, for the `/tools` view.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolStatus {
    /// Tool name, as the model sees it
    pub name: String,
    /// Whether it is active, deferred, or disabled — and by what
    pub availability: ToolAvailability,
}

/// One configured MCP server, for the `/mcp` view.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerStatus {
    /// Server id
    pub id: String,
    /// Transport kind (`stdio` or `http`)
    pub transport: String,
    /// Auth kind (`none`, `bearer`, or `oauth`)
    pub auth: String,
    /// Number of tools known from the metadata cache (`None` = not yet discovered)
    pub tool_count: Option<usize>,
    /// Whether the server is enabled
    pub enabled: bool,
    /// The cached tools' names as the server spells them, less `exclude_tools`; empty until
    /// discovered, which `tool_count` tells apart from a server with none
    #[serde(default)]
    pub tools: Vec<String>,
    /// For an OAuth server, whether a token is stored; `None` for any other. Stored, not valid: an
    /// expired sign-in still reads `true`
    #[serde(default)]
    pub signed_in: Option<bool>,
}

/// One live debug session.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DebugSessionStatus {
    /// Session id
    pub id: String,
    /// Backing adapter id
    pub adapter: String,
    /// Debuggee program / label
    pub program: String,
    /// Whether the debuggee has already terminated
    pub terminated: bool,
    /// Whether the debuggee is currently paused at a stop (breakpoint/step/exception)
    #[serde(default)]
    pub stopped: bool,
    /// Why it stopped, when paused (e.g. `breakpoint`)
    #[serde(default)]
    pub stop_reason: Option<String>,
}
