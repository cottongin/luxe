//! Workspace LSP multiplexer — one server per (language, workspace root).
//!
//! Implements [`DiagnosticsProvider`] for the `file_changed` hook: on each changed file it
//! detects the language, finds the workspace root, lazily starts (and caches) the right
//! language server, and returns diagnostics. Anything unsupported or unavailable degrades
//! to "no diagnostics" so editing is never blocked by a missing server.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use luxe_core::lsp::{Diagnostic, DiagnosticsError, DiagnosticsProvider, Severity};
use tokio::sync::Mutex;
use tracing::{debug, warn};

use crate::client::{LspClient, LspConfig};
use crate::registry::{ServerEntryPatch, ServerRegistry};

/// Base cooldown after a failed handshake; doubles per consecutive failure.
const RETRY_BASE_COOLDOWN: Duration = Duration::from_secs(30);
/// Longest a failing (language, root) is skipped before it gets another chance.
const RETRY_MAX_COOLDOWN: Duration = Duration::from_secs(600);

/// Why a file in a fetched repository has no language server.
const FETCHED_REPOSITORY: &str = "language servers do not start inside a fetched repository \
    (`.luxe/gh-cache/`), because starting one runs that repository's build scripts; read it with \
    read, grep and module_report's outline instead";

/// A failed handshake for one (language, root), and when it may be tried again.
struct StartFailure {
    attempts: u32,
    retry_after: Instant,
    /// What went wrong, kept so the next caller is told *that* rather than "install it".
    ///
    /// The server is usually installed. It is a shim with no version set, a runtime it cannot
    /// find, a config it will not accept — and being sent to install what is already there is the
    /// one instruction guaranteed not to help.
    reason: String,
}

/// A language and the workspace root its server runs for.
type Workspace = (String, PathBuf);

/// What is warm and what failed, which is all the service's lock guards.
#[derive(Default)]
struct Warm {
    /// (language, root) -> live client. Reused across edits.
    clients: HashMap<Workspace, Arc<Mutex<LspClient>>>,
    /// Failed handshakes, keyed like `clients`.
    ///
    /// Keyed by *(language, root)* rather than language: a cold-start timeout on one huge or
    /// malformed root must not disable that language for every other root in the session. Each
    /// entry carries a backoff, so a recoverable failure (a slow first index, a server installed a
    /// minute later) is retried instead of cached forever.
    unavailable: HashMap<Workspace, StartFailure>,
    /// How many times the service has been shut down. A start that began before one must not add
    /// its server to the map afterwards: nothing would ever stop it.
    epoch: u64,
}

/// What a lookup found for a workspace.
enum Lookup {
    Warm(Arc<Mutex<LspClient>>),
    /// A start failed lately and its cooldown has not run out
    CoolingDown,
    Cold,
}

/// A hold on one workspace's start gate, which takes the gate out of the map when the last holder
/// lets go, so `starting` holds only starts in flight.
struct GateLease<'a> {
    starting: &'a std::sync::Mutex<HashMap<Workspace, Arc<Mutex<()>>>>,
    key: Workspace,
    /// `None` only inside `drop`
    gate: Option<Arc<Mutex<()>>>,
}

impl GateLease<'_> {
    fn gate(&self) -> &Mutex<()> {
        self.gate.as_ref().expect("a lease holds its gate until it drops")
    }
}

impl Drop for GateLease<'_> {
    fn drop(&mut self) {
        let mut starting = self.starting.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        // let go before counting, so two leases dropped together cannot each count the other
        drop(self.gate.take());
        // only the map's handle left: nobody holds or waits on the gate, and nobody can take it
        // while the map is locked
        if starting.get(&self.key).is_some_and(|gate| Arc::strong_count(gate) == 1) {
            starting.remove(&self.key);
        }
    }
}

/// Routes diagnostics for a changed file to a per-workspace language server.
///
/// Shared by every tool and the file-changed hook, so nothing here is held across slow work: the
/// maps sit behind a lock taken for a lookup or an insert and never across an `.await`, each client
/// has its own lock, and a start is serialized only against another start of the same workspace.
/// A cold server taking twenty seconds does not hold up `/lsp`, another language, or an edit.
pub struct WorkspaceLspClient {
    registry: ServerRegistry,
    warm: std::sync::Mutex<Warm>,
    /// One gate per workspace, held for the whole of its start, so two requests for a cold root
    /// start one server and the second takes what the first made; gone once nobody holds it
    starting: std::sync::Mutex<HashMap<Workspace, Arc<Mutex<()>>>>,
    /// Handshake timeout for a freshly started server.
    init_timeout: Duration,
    /// Whether a server the *project* supplies may be launched.
    ///
    /// A decided answer, not a policy: who is watching, what the user configured, and
    /// `--trust-project` are weighed by the embedder, because they are its facts. Off by default,
    /// so a caller that says nothing gets the safe direction. What a server does once started is a
    /// separate matter and not this flag's to promise — see [`Self::client_for`].
    project_servers: bool,
}

impl WorkspaceLspClient {
    /// Create a multiplexer over the default server registry (rust-analyzer, pyright, …).
    pub fn new() -> Self {
        Self::over(ServerRegistry::new())
    }

    fn over(registry: ServerRegistry) -> Self {
        Self {
            registry,
            warm: Default::default(),
            starting: Default::default(),
            init_timeout: Duration::from_secs(20),
            project_servers: false,
        }
    }

    /// Allow servers the project installed into `node_modules/.bin` to be launched.
    #[must_use]
    pub fn with_project_servers(mut self, allowed: bool) -> Self {
        self.project_servers = allowed;
        self
    }

    /// Create a multiplexer whose registry is the built-ins plus user-configured servers (from
    /// `~/.config/luxe/lsp-servers.json`), so any LSP is plug-and-play by extension.
    pub fn with_servers(custom: HashMap<String, ServerEntryPatch>) -> Self {
        let mut registry = ServerRegistry::new();
        for rejected in registry.merge_patches(custom) {
            tracing::warn!(
                language = %rejected,
                "lsp-servers.json entry needs a `command`: luxe ships no default for this language"
            );
        }
        Self::over(registry)
    }

    /// An actionable message when no server handles a file: the install hint for a known-but-
    /// missing server, or a pointer to `lsp-servers.json` for an unconfigured extension.
    pub fn missing_server_hint(&self, path: &Path) -> String {
        if luxe_core::fetched::contains(path) {
            return FETCHED_REPOSITORY.to_string();
        }
        if let Some(reason) = self.start_failure(path) {
            return reason;
        }
        match self.registry.detect_language(path) {
            Some(lang) => match self.registry.install_hint(&lang) {
                Some(hint) => format!(
                    "no {lang} language server running — install it (`{hint}`) or check it's on PATH"
                ),
                None => format!("no {lang} language server available (check it's installed + on PATH)"),
            },
            None => {
                let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("(none)");
                format!(
                    "no language server configured for .{ext} files — add one to {}",
                    luxe_core::user_dirs::config_path_display("lsp-servers.json")
                )
            }
        }
    }

    /// Why this file's server is not running, when it was tried this session and refused.
    ///
    /// Looked up the same way `client_for_path` resolves a client, so the answer describes the
    /// server that would actually have served this file.
    fn start_failure(&self, path: &Path) -> Option<String> {
        let workspace = self.workspace_of(path)?;
        self.warm().unavailable.get(&workspace).map(|failure| failure.reason.clone())
    }

    /// The `(language, workspace_root)` a file belongs to: its detected language, and the nearest
    /// directory holding one of that language's root markers, else the file's own directory.
    fn workspace_of(&self, path: &Path) -> Option<Workspace> {
        let language = self.registry.detect_language(path)?;
        let markers =
            self.registry.get(&language).map(|e| e.root_markers.clone()).unwrap_or_default();
        let root = self
            .registry
            .find_root(path, &markers)
            .or_else(|| path.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("."));
        Some((language, root))
    }

    /// The `(language, workspace_root)` of every warm language-server client, for status surfaces
    /// like `/lsp`. A server appears here once it has been lazily started for a file this session.
    pub fn running_servers(&self) -> Vec<(String, PathBuf)> {
        let mut out: Vec<Workspace> = self.warm().clients.keys().cloned().collect();
        out.sort();
        out
    }

    /// Gracefully shut down every warm language-server client, then forget them.
    ///
    /// Each server gets a bounded window to handle `shutdown` + `exit`; if it stalls (some servers
    /// answer `shutdown` with requests of their own before exiting), we move on and `kill_on_drop`
    /// reaps the child. Called when leaving a project so its servers don't linger into the next one.
    pub async fn shutdown_all(&self) {
        let clients = {
            let mut warm = self.warm();
            warm.unavailable.clear();
            warm.epoch += 1;
            std::mem::take(&mut warm.clients)
        };
        for ((language, _root), client) in clients {
            let bounded = tokio::time::timeout(Duration::from_secs(3), async {
                client.lock().await.shutdown().await
            })
            .await;
            if bounded.is_err() {
                warn!(%language, "language server slow to shut down; killed on drop");
            }
            // `client` drops here — kill_on_drop reaps the child if shutdown didn't finish.
        }
    }

    /// Get (or lazily start) the warm client for a file, keyed by (detected language, workspace
    /// root). Returns `None` for unsupported languages, servers that failed to start this session,
    /// and files inside a fetched repository. The returned handle is reused across tool calls and
    /// edits.
    pub async fn client_for_path(&self, path: &Path) -> Option<Arc<Mutex<LspClient>>> {
        // The root would be the clone's own, and starting a server there runs its build scripts
        if luxe_core::fetched::contains(path) {
            return None;
        }
        let (language, root) = self.workspace_of(path)?;
        self.client_for(&language, root).await
    }

    /// The maps, for a lookup or an insert. A panic elsewhere must not take the service down with it.
    fn warm(&self) -> std::sync::MutexGuard<'_, Warm> {
        self.warm.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn lookup(&self, key: &Workspace) -> Lookup {
        let warm = self.warm();
        if let Some(client) = warm.clients.get(key) {
            return Lookup::Warm(client.clone());
        }
        match warm.unavailable.get(key) {
            Some(failure) if Instant::now() < failure.retry_after => Lookup::CoolingDown,
            _ => Lookup::Cold,
        }
    }

    /// Get (or lazily start) the client for a language + root, caching the result.
    ///
    /// A previous failure for this exact (language, root) is honored only until its cooldown
    /// expires; other roots of the same language are unaffected. A start holds only its own
    /// workspace's gate, so a second caller for the same root waits for it and shares the result.
    async fn client_for(&self, language: &str, root: PathBuf) -> Option<Arc<Mutex<LspClient>>> {
        let key = (language.to_string(), root.clone());
        let epoch = self.warm().epoch;
        match self.lookup(&key) {
            Lookup::Warm(client) => return Some(client),
            Lookup::CoolingDown => return None,
            Lookup::Cold => {}
        }
        let lease = self.gate(&key);
        let _starting = lease.gate().lock().await;
        // Whoever held the gate before us may have settled it either way
        match self.lookup(&key) {
            Lookup::Warm(client) => return Some(client),
            Lookup::CoolingDown => return None,
            Lookup::Cold => {}
        }
        if self.warm().unavailable.contains_key(&key) {
            debug!(language, root = %root.display(), "retrying a language server that failed earlier");
        }
        self.start(language, root, key, epoch).await
    }

    /// This workspace's start gate, made if no start holds one.
    fn gate(&self, key: &Workspace) -> GateLease<'_> {
        let mut starting = self.starting.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let gate = Arc::clone(starting.entry(key.clone()).or_default());
        GateLease { starting: &self.starting, key: key.clone(), gate: Some(gate) }
    }

    /// Launches `language`'s server for `root`, trying each way of launching it in order.
    async fn start(
        &self,
        language: &str,
        root: PathBuf,
        key: Workspace,
        epoch: u64,
    ) -> Option<Arc<Mutex<LspClient>>> {
        let entry = self.registry.get(language)?.clone();
        // Try each way of launching this language, in order, falling through on a failed
        // *handshake* rather than only a missing binary: `tsc` is on PATH for both TypeScript 6
        // and 7 and only one of them answers, so resolution alone cannot choose. Cheap now that a
        // server which refuses reports it immediately instead of costing a 30-second timeout.
        let mut refusals: Vec<String> = Vec::new();
        let mut unpermitted: Vec<String> = Vec::new();
        for candidate in entry.candidates() {
            if self.warm().epoch != epoch {
                return None;
            }
            match self.try_candidate(language, &root, &candidate).await {
                Tried::Started(client) => {
                    let handle = Arc::new(Mutex::new(*client));
                    let mut warm = self.warm();
                    if warm.epoch != epoch {
                        // Shut down while it was starting: dropping it kills the process
                        debug!(
                            language,
                            "a language server finished starting after shutdown; stopped"
                        );
                        return None;
                    }
                    warm.unavailable.remove(&key);
                    warm.clients.insert(key, handle.clone());
                    return Some(handle);
                }
                Tried::Refused(refusal) => refusals.push(refusal),
                Tried::NotPermitted(local) => unpermitted.push(local),
                Tried::Absent => {}
            }
        }

        let reason = self.failure_reason(language, &refusals, unpermitted.first());
        let (attempts, cooldown) = {
            let mut warm = self.warm();
            // Nothing is recorded for a start the service was shut down under: it would be a
            // cooldown for a project that is gone. Checked in the lock that records, so a shutdown
            // cannot land between the two.
            if warm.epoch != epoch {
                return None;
            }
            let attempts =
                warm.unavailable.get(&key).map_or(1, |failure| failure.attempts.saturating_add(1));
            let cooldown = next_cooldown(attempts);
            let failure = StartFailure {
                attempts,
                retry_after: Instant::now() + cooldown,
                reason: reason.clone(),
            };
            warm.unavailable.insert(key, failure);
            (attempts, cooldown)
        };
        warn!(
            language,
            root = %root.display(),
            attempts,
            retry_in_secs = cooldown.as_secs(),
            %reason,
            "no language server could be started for this workspace root; retrying after a cooldown"
        );
        None
    }

    /// Forget every recorded start failure so the next request retries immediately.
    ///
    /// The manual escape hatch for an embedder that knows a server was just installed or fixed and
    /// shouldn't have to wait out its cooldown.
    pub fn clear_start_failures(&self) -> usize {
        let mut warm = self.warm();
        let cleared = warm.unavailable.len();
        warm.unavailable.clear();
        cleared
    }
}

impl Default for WorkspaceLspClient {
    fn default() -> Self {
        Self::new()
    }
}

/// The core severity for a protocol one; the numeric LSP mapping stops here.
fn severity_of(severity: crate::protocol::DiagnosticSeverity) -> Severity {
    use crate::protocol::DiagnosticSeverity;
    match severity {
        DiagnosticSeverity::Error => Severity::Error,
        DiagnosticSeverity::Warning => Severity::Warning,
        DiagnosticSeverity::Information => Severity::Information,
        DiagnosticSeverity::Hint => Severity::Hint,
    }
}

#[async_trait::async_trait]
impl DiagnosticsProvider for WorkspaceLspClient {
    async fn run_diagnostics(&self, path: &Path) -> Result<Vec<Diagnostic>, DiagnosticsError> {
        let Some(client) = self.client_for_path(path).await else {
            return Ok(Vec::new()); // unsupported language or unavailable server
        };
        let uri = crate::uri::from_path(path);
        let content = std::fs::read_to_string(path).map_err(|source| DiagnosticsError::Read {
            path: luxe_core::paths::display(path),
            error: source,
        })?;

        let mut guard = client.lock().await;
        match guard.get_diagnostics(&uri, &content).await {
            Ok(diags) => Ok(diags
                .into_iter()
                .map(|d| Diagnostic {
                    severity: d.severity.map(severity_of),
                    line: d.range.start.line,
                    character: d.range.start.character,
                    message: d.message,
                })
                .collect()),
            Err(e) => {
                debug!(uri, error = %e, "workspace lsp: diagnostics failed");
                Ok(Vec::new())
            }
        }
    }
}

mod start;

use start::{next_cooldown, Tried};

#[cfg(test)]
mod tests;
