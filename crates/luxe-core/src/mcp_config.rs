//! MCP server config store — durable `.luxe/mcp.json`.
//!
//! Persists the set of opt-in MCP servers a project uses so `luxe mcp add/remove/list`
//! and engine startup share one source of truth. The file is a small JSON document:
//!
//! ```json
//! { "servers": [ { "id": "...", "name": "...", "transport": { ... }, "enabled": true } ] }
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::mcp_client::{McpAuth, McpLifecycle, McpServerConfig, McpTransport};

/// On-disk shape of the MCP config file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpConfigFile {
    /// Configured servers.
    #[serde(default)]
    pub servers: Vec<McpServerConfig>,
}

/// [`McpConfigStore::for_run`] over explicit files, so the split is testable without a home.
fn split_for_run(
    user: Vec<PathBuf>,
    project: [PathBuf; 2],
    include_project: bool,
) -> (Vec<McpServerConfig>, Vec<String>) {
    let everything = merge(user.iter().cloned().chain(project.iter().cloned()), &project);
    if include_project {
        return (everything.into_iter().map(|(server, _)| server).collect(), Vec::new());
    }
    let mine: Vec<McpServerConfig> =
        merge(user, &project).into_iter().map(|(server, _)| server).collect();
    let refused = everything
        .into_iter()
        .filter(|(server, path)| {
            project.contains(path) && !mine.iter().any(|kept| kept.id == server.id)
        })
        .map(|(server, _)| server.id)
        .collect();
    (mine, refused)
}

/// Every server declared across `sources`, each with the file it came from.
///
/// Later (higher-precedence) sources override an earlier server with the same id. A server from a
/// file outside `project` is marked [`McpServerConfig::declared_by_user`]; the mark follows the
/// winning file, so a project entry shadowing one of yours loses the right to run your commands.
fn merge(
    sources: impl IntoIterator<Item = PathBuf>,
    project: &[PathBuf],
) -> Vec<(McpServerConfig, PathBuf)> {
    let mut merged: Vec<(McpServerConfig, PathBuf)> = Vec::new();
    for path in sources {
        for mut server in load_any(&path) {
            server.declared_by_user = !project.contains(&path);
            merged.retain(|(existing, _)| existing.id != server.id);
            merged.push((server, path.clone()));
        }
    }
    merged
}

/// The standard `mcpServers` config shape used by other hosts (Cursor/Claude/Codex/etc.).
#[derive(Debug, Deserialize)]
pub struct StandardConfigFile {
    #[serde(rename = "mcpServers", default)]
    mcp_servers: HashMap<String, StandardServer>,
}

/// One entry of the standard `mcpServers` map.
#[derive(Debug, Deserialize)]
struct StandardServer {
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    headers: HashMap<String, String>,
    #[serde(default)]
    env: HashMap<String, String>,
    #[serde(default)]
    lifecycle: McpLifecycle,
    #[serde(default)]
    exclude_tools: Vec<String>,
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default)]
    timeout_secs: Option<u64>,
    /// Luxe's own addition to the shape: other hosts ignore it, and without it an OAuth server
    /// written the way `docs/guide/mcp.md` shows lost its auth without a word
    #[serde(default)]
    auth: Option<McpAuth>,
}

fn default_true() -> bool {
    true
}

/// Read a config file in either the Luxe (`servers`) or standard (`mcpServers`) shape. A missing,
/// empty, or unparseable file yields no servers (so a stray file never blocks launch), and says so.
fn load_any(path: &Path) -> Vec<McpServerConfig> {
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    if text.trim().is_empty() {
        return Vec::new();
    }
    parse_any(&text).unwrap_or_else(|error| {
        tracing::warn!(path = %path.display(), %error, "mcp: ignoring unparseable config");
        Vec::new()
    })
}

/// The servers `text` names in either shape, or why it names none it meant to.
///
/// A Luxe-shape file that fails to parse also reads as an empty standard one, so when the
/// standard shape finds nothing the Luxe error is the one that explains the file.
fn parse_any(text: &str) -> Result<Vec<McpServerConfig>, String> {
    let luxe = serde_json::from_str::<McpConfigFile>(text);
    if let Ok(file) = &luxe {
        if !file.servers.is_empty() {
            return luxe.map(|file| file.servers).map_err(|error| error.to_string());
        }
    }
    let standard = serde_json::from_str::<StandardConfigFile>(text).map_err(|e| e.to_string())?;
    let servers: Vec<McpServerConfig> = standard
        .mcp_servers
        .into_iter()
        .filter_map(|(id, server)| server.into_config(id))
        .collect();
    match luxe {
        Err(error) if servers.is_empty() => Err(format!("in its `servers`: {error}")),
        _ => Ok(servers),
    }
}

impl StandardServer {
    /// Map a standard entry to a [`McpServerConfig`] (`None` when it has neither command nor url).
    fn into_config(self, id: String) -> Option<McpServerConfig> {
        let transport = match (self.command, self.url) {
            (_, Some(url)) => McpTransport::HttpSse { url },
            (Some(command), None) => {
                McpTransport::Stdio { command, args: self.args, cwd: None, env: self.env }
            }
            (None, None) => return None,
        };
        Some(McpServerConfig {
            name: id.clone(),
            id,
            transport,
            enabled: self.enabled,
            lifecycle: self.lifecycle,
            idle_timeout_mins: None,
            timeout_secs: self.timeout_secs,
            exclude_tools: self.exclude_tools,
            headers: self.headers,
            auth: self.auth,
            // the store marks it, from the file it was found in
            declared_by_user: false,
        })
    }
}

/// Errors from reading/writing the MCP config file.
#[derive(Debug, thiserror::Error)]
pub enum McpConfigError {
    #[error("I/O error: {0}")]
    Io(std::io::Error),
    #[error("JSON error: {0}")]
    Json(serde_json::Error),
    #[error("server '{0}' already exists")]
    Duplicate(String),
}

crate::error_cause::from_cause!(McpConfigError {
    std::io::Error => Io,
    serde_json::Error => Json,
});

/// A file-backed store for MCP server configs.
///
/// All mutations rewrite the whole file — the config is tiny and only touched by
/// interactive `mcp` commands, so atomicity beats cleverness.
pub struct McpConfigStore {
    path: PathBuf,
}

impl McpConfigStore {
    /// Create a store rooted at an explicit config-file path.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The conventional store at `<repo>/.luxe/mcp.json`.
    pub fn for_repo(repo_root: &Path) -> Self {
        Self::new(repo_root.join(".luxe").join("mcp.json"))
    }

    /// Path to the backing file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Load the config, returning an empty set when the file is absent.
    pub fn load(&self) -> Result<McpConfigFile, McpConfigError> {
        match std::fs::read_to_string(&self.path) {
            Ok(s) if s.trim().is_empty() => Ok(McpConfigFile::default()),
            Ok(s) => Ok(serde_json::from_str(&s)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(McpConfigFile::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// The configured servers (convenience over [`Self::load`]).
    pub fn list(&self) -> Result<Vec<McpServerConfig>, McpConfigError> {
        Ok(self.load()?.servers)
    }

    /// The effective server set for a project, merged across every known config location with
    /// precedence (project overrides user-global; luxe-owned overrides the shared standard file):
    ///
    /// 1. `~/.config/mcp/mcp.json` (standard, user-global) — lowest
    /// 2. `~/.config/luxe/mcp.json` (Luxe, user-global)
    /// 3. `.mcp.json` (standard, project)
    /// 4. `.luxe/mcp.json` (Luxe, project) — highest
    ///
    /// Both the Luxe shape (`{ "servers": [...] }`) and the standard shape
    /// (`{ "mcpServers": { id: {...} } }`, as written by Cursor/Claude/Codex) are understood, so an
    /// existing MCP setup works with no conversion.
    pub fn resolved(repo_root: &Path) -> Vec<McpServerConfig> {
        Self::attributed(repo_root).into_iter().map(|(server, _)| server).collect()
    }

    /// [`resolved`](Self::resolved), keeping the file each surviving server was declared in.
    ///
    /// A merged server does not otherwise remember where it came from, which is fine for starting
    /// it and useless for editing it — four files can declare one and only one of them wins.
    pub fn attributed(repo_root: &Path) -> Vec<(McpServerConfig, PathBuf)> {
        merge(Self::sources(repo_root), &Self::project_sources(repo_root))
    }

    /// The servers a run may start, and the ids of the project's that it may not.
    ///
    /// Args: `(repo_root, include_project)`. A stdio server is a command line, so one the
    /// *repository* declares is the repository choosing what runs — the question its
    /// `.luxe/hooks.json` raises, answered by whoever decides whether this run may execute what the
    /// project supplies. Refused, a project entry that shadowed a user server of the same id gives
    /// the user's definition back rather than taking it away, and is not reported as refused.
    pub fn for_run(repo_root: &Path, include_project: bool) -> (Vec<McpServerConfig>, Vec<String>) {
        split_for_run(Self::user_sources(), Self::project_sources(repo_root), include_project)
    }

    /// Every file that may declare a server, lowest precedence first.
    pub fn sources(repo_root: &Path) -> Vec<PathBuf> {
        let mut sources = Self::user_sources();
        sources.extend(Self::project_sources(repo_root));
        sources
    }

    /// The files a repository can ship, lowest precedence first.
    fn project_sources(repo_root: &Path) -> [PathBuf; 2] {
        [repo_root.join(".mcp.json"), repo_root.join(".luxe").join("mcp.json")]
    }

    /// The shared `mcp/mcp.json` other clients read, then Luxe's own.
    fn user_sources() -> Vec<PathBuf> {
        let shared = crate::user_dirs::config_home().map(|home| home.join("mcp").join("mcp.json"));
        let own = crate::user_dirs::luxe_config_home().map(|luxe| luxe.join("mcp.json"));
        shared.into_iter().chain(own).collect()
    }

    /// Persist the given config atomically, keeping the file's mode or starting a new one private,
    /// since a server's auth can hold a client secret.
    pub fn save(&self, file: &McpConfigFile) -> Result<(), McpConfigError> {
        let json = serde_json::to_string_pretty(file)?;
        crate::fs_atomic::replace_preserving_mode(&self.path, format!("{json}\n").as_bytes())?;
        Ok(())
    }

    /// Add a server, rejecting a duplicate id.
    pub fn add(&self, config: McpServerConfig) -> Result<(), McpConfigError> {
        let mut file = self.load()?;
        if file.servers.iter().any(|s| s.id == config.id) {
            return Err(McpConfigError::Duplicate(config.id));
        }
        file.servers.push(config);
        self.save(&file)
    }

    /// Remove a server by id. Returns whether an entry was removed.
    pub fn remove(&self, id: &str) -> Result<bool, McpConfigError> {
        let mut file = self.load()?;
        let before = file.servers.len();
        file.servers.retain(|s| s.id != id);
        let removed = file.servers.len() != before;
        if removed {
            self.save(&file)?;
        }
        Ok(removed)
    }
}

#[cfg(test)]
mod tests;
