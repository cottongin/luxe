//! Warm, process-wide service singletons — the LSP client, code graph, DAP debugger, and
//! diagnostics cache — plus the per-project session store.
//!
//! Luxe runs one project per process, so each service is a `OnceLock` singleton reused across
//! every tool call and hook instead of being spawned per request.

use std::sync::Arc;

use luxe_core::session::JsonlSessionStore;

use super::config::project_hash;

/// Open the project's session store (session dir + project hash) without creating a session.
///
/// Shared by `build_engine` and the `sessions`/resume paths so they agree on where
/// sessions live and how the project is hashed.
pub(crate) fn open_session_store(repo_root: &std::path::Path) -> JsonlSessionStore {
    let session_dir = repo_root.join(".luxe").join("sessions");
    let mut store = JsonlSessionStore::new(&session_dir);
    store.set_project_hash(&project_hash(repo_root));
    store
}

/// A warm, per-workspace LSP service shared by the diagnostics/navigation tools and the
/// diagnostics-on-edit hook. One long-lived instance reuses language servers across every call
/// (no spawn-per-call).
pub(crate) type SharedLspService = Arc<luxe_lsp::WorkspaceLspClient>;

/// Whether this run may launch a language server the project installed for itself.
///
/// A `OnceLock` rather than a parameter because [`build_lsp_service`] is reached from three places
/// that do not all hold the settings — startup, the workspace collector, and shutdown — and only
/// the first of them knows the answer. Unset reads as refused, so an ordering mistake fails closed
/// rather than granting something nobody asked for.
static PROJECT_SERVERS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

/// What every tool ended up doing, published once the registry is assembled.
///
/// Same shape and same reason as [`PROJECT_SERVERS`]: the answer is settled at startup by the code
/// that holds the registry, and read later by the workspace collector, which does not. Unset reads
/// as "nothing to report", so a front-end built without an engine simply shows no tool section.
static TOOL_STATUSES: std::sync::OnceLock<Vec<luxe_core::ToolStatus>> = std::sync::OnceLock::new();

/// Record what `/tools` should say. Called once, from registry assembly.
pub(crate) fn note_tool_statuses(statuses: Vec<luxe_core::ToolStatus>) {
    let _ = TOOL_STATUSES.set(statuses);
}

/// What `/tools` should say, or empty before the registry has been built.
pub(crate) fn tool_statuses() -> Vec<luxe_core::ToolStatus> {
    TOOL_STATUSES.get().cloned().unwrap_or_default()
}

/// Resolve whether this run may launch a server out of the project's `node_modules/.bin`.
///
/// Three inputs, and the judgement is here rather than in `luxe-lsp` so it can be read and tested
/// in one place: the user's preference, whether a front-end can prompt, and `--trust-project`.
///
/// `interactive` carries the whole argument for the default. Luxe is a coding agent someone sits
/// in front of; the loop is a feature, not the usual way it runs. With a person there, using the
/// server the project pinned is ordinary work — and `node_modules/.bin` exists only because
/// `npm install` already ran that project's install scripts, which is arbitrary code and a wider
/// door than one language server. Unattended there is nobody to notice, so that case wants the
/// explicit yes `--trust-project` already means everywhere else.
pub(crate) fn project_servers_allowed(
    preference: luxe_core::config::ProjectServers,
    interactive: bool,
    trust_project: bool,
) -> bool {
    use luxe_core::config::ProjectServers;
    match preference {
        ProjectServers::Never => false,
        ProjectServers::Always => true,
        ProjectServers::Auto => interactive || trust_project,
    }
}

/// Record the resolved answer for the services built later in this run.
///
/// Called once, before anything builds the LSP service. Later calls are ignored, which is what
/// keeps the answer from changing under a service that has already been handed out.
pub(crate) fn note_project_servers(allowed: bool) {
    let _ = PROJECT_SERVERS.set(allowed);
}

/// Whether this run may execute a binary the repository supplied.
///
/// The same answer [`build_lsp_service`] uses, read by format-on-save, because it is the same
/// question: a `.venv/bin/ruff` or a `node_modules/.bin/biome` got there from the repository. One
/// decision rather than two — a user who said "do not run this project's binaries" did not mean
/// only the language servers, and one who pinned a formatter version means the formatter too.
/// Unset reads as refused, the same fail-closed default.
pub(crate) fn project_tooling_allowed() -> bool {
    PROJECT_SERVERS.get().copied().unwrap_or(false)
}

/// The process-wide warm LSP service. A singleton (like [`shared_diagnostics_store`]) so the tools,
/// the diagnostics-on-edit hook, and the workspace collector (which reads running-server state for
/// `/lsp`) all share one instance. Luxe runs one project per process; the collector scopes
/// running servers to the active root.
pub(crate) fn build_lsp_service() -> SharedLspService {
    static SERVICE: std::sync::OnceLock<SharedLspService> = std::sync::OnceLock::new();
    SERVICE
        .get_or_init(|| {
            Arc::new(
                luxe_lsp::WorkspaceLspClient::with_servers(load_lsp_servers())
                    .with_project_servers(PROJECT_SERVERS.get().copied().unwrap_or(false)),
            )
        })
        .clone()
}

/// The process-wide warm code graph service, shared by the `impact` / `module_report` tools and the
/// graph-invalidation hook. Singleton like the LSP/DAP services; rooted at the workspace on first
/// use (Luxe runs one project per process).
pub(crate) fn build_graph_service(
    repo_root: &std::path::Path,
) -> Arc<luxe_lsp::graph::GraphService> {
    static SERVICE: std::sync::OnceLock<Arc<luxe_lsp::graph::GraphService>> =
        std::sync::OnceLock::new();
    SERVICE.get_or_init(|| Arc::new(luxe_lsp::graph::GraphService::new(repo_root))).clone()
}

/// The process-wide warm symbol index, shared by `symbol_search` and the hook that forgets it when
/// a file changes. Singleton for the same reason as the graph service.
pub(super) fn shared_symbol_index() -> Arc<luxe_lsp::SymbolIndexCache> {
    static INDEX: std::sync::OnceLock<Arc<luxe_lsp::SymbolIndexCache>> = std::sync::OnceLock::new();
    INDEX.get_or_init(Arc::default).clone()
}

/// The process-wide warm DAP service, shared by the `debug` tool and the workspace collector (which
/// lists live debug sessions for `/debug`). Singleton for the same reason as the LSP service.
pub(crate) fn shared_dap_service() -> Arc<luxe_dap::DapService> {
    static SERVICE: std::sync::OnceLock<Arc<luxe_dap::DapService>> = std::sync::OnceLock::new();
    SERVICE
        .get_or_init(|| Arc::new(luxe_dap::DapService::with_adapters(load_debug_adapters())))
        .clone()
}

/// Loads user-configured language servers from `~/.config/luxe/lsp-servers.json`.
/// A missing file is no servers; an invalid one is logged and ignored (built-ins still work).
pub(crate) fn load_lsp_servers() -> std::collections::HashMap<String, luxe_lsp::ServerEntryPatch> {
    read_lsp_servers().unwrap_or_else(|error| {
        tracing::warn!(%error, "ignoring invalid lsp-servers.json");
        std::collections::HashMap::new()
    })
}

/// [`load_lsp_servers`], keeping the reason an invalid file was ignored — for `luxe doctor`,
/// which is where someone goes to find out why their server is not there.
pub(crate) fn read_lsp_servers(
) -> Result<std::collections::HashMap<String, luxe_lsp::ServerEntryPatch>, String> {
    let path = luxe_core::config::Config::lsp_servers_path();
    match read_config_text(&path)? {
        Some(text) => serde_json::from_str(&text)
            .map_err(|error| format!("{}: {error}", luxe_core::paths::display(&path))),
        None => Ok(std::collections::HashMap::new()),
    }
}

/// Whether the first word of `command` is a program this machine can run.
///
/// Delegates to [`luxe_core::program::is_available`], which is the one owner of "can I run this"
/// (`AGENTS.md`): it also tries the Windows executable suffixes, so a bare `rg` finds `rg.exe`
/// instead of reading as "ripgrep is not installed" on a machine where it plainly is. The split is
/// here because these entries are whole command lines (`python3 -m debugpy.adapter`), not names.
pub(crate) fn command_on_path(command: &str) -> bool {
    let program = command.split_whitespace().next().unwrap_or(command);
    !program.is_empty() && luxe_core::program::is_available(program)
}

/// Loads user-configured debug adapters from `~/.config/luxe/debug-adapters.json` (a JSON array).
/// A missing file is no adapters; an invalid one is logged and ignored (built-ins still work).
pub(crate) fn load_debug_adapters() -> Vec<luxe_dap::CustomAdapter> {
    read_debug_adapters().unwrap_or_else(|error| {
        tracing::warn!(%error, "ignoring invalid debug-adapters.json");
        Vec::new()
    })
}

/// [`load_debug_adapters`], keeping the reason an invalid file was ignored.
pub(crate) fn read_debug_adapters() -> Result<Vec<luxe_dap::CustomAdapter>, String> {
    let path = luxe_core::config::Config::debug_adapters_path();
    match read_config_text(&path)? {
        Some(text) => serde_json::from_str(&text)
            .map_err(|error| format!("{}: {error}", luxe_core::paths::display(&path))),
        None => Ok(Vec::new()),
    }
}

/// Every debug adapter a session can use: the built-ins a user entry does not shadow, then the
/// user's own. Args of each: `(adapter, from_config)`.
///
/// The one list, for the `/debug` view and `luxe doctor` alike, so an adapter added in config
/// appears in the checkup meant to confirm it.
pub(crate) fn merged_debug_adapters(
    user: &[luxe_dap::CustomAdapter],
) -> Vec<(luxe_dap::adapter::ConfiguredAdapter, bool)> {
    let user_ids: std::collections::HashSet<&str> =
        user.iter().map(|adapter| adapter.id.as_str()).collect();
    luxe_dap::builtin_adapters()
        .into_iter()
        .filter(|builtin| !user_ids.contains(builtin.spec.id.as_str()))
        .map(|builtin| (builtin, false))
        .chain(
            user.iter()
                .map(|custom| (luxe_dap::adapter::ConfiguredAdapter::from_custom(custom), true)),
        )
        .collect()
}

/// A config file's text, `None` when it does not exist; any other failure names the path.
fn read_config_text(path: &std::path::Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(path) {
        Ok(contents) => Ok(Some(contents)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("{}: {error}", luxe_core::paths::display(path))),
    }
}

/// The process-wide diagnostics cache shared by the diagnostics-on-edit hook (writer) and the
/// workspace collector (reader).
///
/// A shared handle at the binary boundary: it lets the (core) hook publish per-file diagnostics
/// that the (core-agnostic) collector reads for the Context Dock without threading a channel
/// through every front-end. Luxe runs one project per process; the collector filters entries to the
/// active root, so a project switch never leaks another tree's diagnostics.
pub(crate) fn shared_diagnostics_store() -> luxe_core::lsp::DiagnosticsStore {
    static STORE: std::sync::OnceLock<luxe_core::lsp::DiagnosticsStore> =
        std::sync::OnceLock::new();
    STORE.get_or_init(luxe_core::lsp::DiagnosticsStore::new).clone()
}

/// The process-wide record of work done outside the project, written by the external-work hook and
/// read by the workspace collector.
///
/// Shares the diagnostics cache's arrangement and its reason: one project per process, a hook that
/// writes and a collector that reads, and no channel threaded through every front-end to join them.
pub(crate) fn shared_external_work() -> luxe_core::external_work::ExternalWork {
    static WORK: std::sync::OnceLock<luxe_core::external_work::ExternalWork> =
        std::sync::OnceLock::new();
    WORK.get_or_init(luxe_core::external_work::ExternalWork::new).clone()
}

#[cfg(test)]
mod tests {
    use super::{merged_debug_adapters, project_servers_allowed};
    use luxe_core::config::ProjectServers;

    /// A configured adapter is in the list every surface reads, and one reusing a built-in id
    /// replaces it rather than appearing twice.
    ///
    /// `luxe doctor` listed the built-ins alone, so the adapter someone had just added never showed
    /// up in the checkup they ran to confirm it.
    #[test]
    fn configured_adapters_join_the_built_ins() {
        let user: Vec<luxe_dap::CustomAdapter> = serde_json::from_str(
            r#"[
                { "id": "ruby", "command": ["rdbg"], "extensions": ["rb"], "dialect": "debugpy" },
                { "id": "gdb", "command": ["my-gdb"], "dialect": "debugpy" }
            ]"#,
        )
        .expect("valid adapters");
        let merged = merged_debug_adapters(&user);
        let find = |id: &str| merged.iter().filter(|(a, _)| a.spec.id == id).collect::<Vec<_>>();
        assert!(matches!(find("ruby").as_slice(), [(_, true)]), "the configured adapter is listed");
        let gdb = find("gdb");
        assert_eq!(gdb.len(), 1, "a shadowed built-in appears once");
        assert_eq!(gdb[0].0.spec.command, ["my-gdb"], "as the user's");
        assert!(matches!(find("debugpy").as_slice(), [(_, false)]), "untouched built-ins stay");
    }

    /// The default answer turns on who is watching, and nothing else.
    #[test]
    fn auto_follows_whether_anyone_is_there_to_notice() {
        assert!(project_servers_allowed(ProjectServers::Auto, true, false), "a TUI session");
        assert!(!project_servers_allowed(ProjectServers::Auto, false, false), "headless or a loop");
        assert!(
            project_servers_allowed(ProjectServers::Auto, false, true),
            "unattended, but the run said it trusts this project"
        );
    }

    /// Both explicit settings mean it everywhere, including where the default would disagree.
    #[test]
    fn an_explicit_preference_wins_over_the_situation() {
        for (interactive, trust) in [(true, true), (true, false), (false, true), (false, false)] {
            assert!(project_servers_allowed(ProjectServers::Always, interactive, trust));
            assert!(!project_servers_allowed(ProjectServers::Never, interactive, trust));
        }
    }

    /// `never` outranks `--trust-project`: the flag is what a *repository* may do, not an
    /// instruction to override a preference the user set for themselves.
    #[test]
    fn never_is_not_overridden_by_trusting_the_project() {
        assert!(!project_servers_allowed(ProjectServers::Never, true, true));
    }
}
