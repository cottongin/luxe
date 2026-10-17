//! Tool registry assembly — built-ins, the LSP discovery funnel, the DAP debugger, opt-in MCP,
//! the config deny-list, and optional lazy-tool tiering.

use std::sync::Arc;

use luxe_core::ToolRegistry;

use super::config::env_disabled;
use super::services::{build_graph_service, shared_dap_service, SharedLspService};

/// Where a deny-list entry came from — and so what the reader has to edit to change it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DenySource {
    /// `--no-tool <name>` on this command line
    Flag,
    /// `tools.disabled` in a config file
    Config,
}

impl DenySource {
    /// How `/tools` names this source in the status list.
    fn label(self) -> &'static str {
        match self {
            Self::Flag => "--no-tool",
            Self::Config => "config",
        }
    }

    /// How a notice quotes the instruction, so the reader can find it and fix it.
    fn quoted(self, name: &str) -> String {
        match self {
            Self::Flag => format!("--no-tool {name}"),
            Self::Config => format!("tools.disabled \"{name}\""),
        }
    }
}

/// What the deny-list actually did with one name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeniedTool {
    /// The name as written
    name: String,
    /// Where it was written
    source: DenySource,
}

/// What the deny-list did, in the four outcomes worth telling apart.
///
/// A list of removals cannot explain an absence: `--no-tool websearch` and `--no-tool screenshot`
/// on a machine with no browser both removed nothing, and only one of them is a typo.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ToolDenyReport {
    /// Unregistered, with what asked for it — what `/tools` reports as `disabled`
    removed: Vec<DeniedTool>,
    /// Essential, so the request was refused
    refused: Vec<DeniedTool>,
    /// A tool Luxe knows, not registered on this machine
    unavailable: Vec<DeniedTool>,
    /// No tool of that name anywhere, with the nearest one when there is a near miss
    unknown: Vec<(DeniedTool, Option<&'static str>)>,
}

impl ToolDenyReport {
    /// The removals, in the shape [`collect_tool_statuses`] takes.
    pub(super) fn removals(&self) -> Vec<(String, String)> {
        self.removed
            .iter()
            .map(|denied| (denied.name.clone(), denied.source.label().to_string()))
            .collect()
    }

    /// What to say at startup, or nothing when the deny-list did exactly what it was asked.
    ///
    /// A removal is silent — it worked, and `/tools` lists it. The other three are the reader
    /// believing something happened that did not.
    pub(super) fn notices(&self) -> Vec<String> {
        let mut notices = Vec::new();
        for denied in &self.refused {
            notices.push(format!(
                "{}: refused — the agent cannot work without read, write, edit and bash.",
                denied.source.quoted(&denied.name)
            ));
        }
        for (denied, nearest) in &self.unknown {
            let suggestion = match nearest {
                Some(nearest) => format!(" Did you mean {nearest}?"),
                None => String::new(),
            };
            notices.push(format!(
                "{}: no tool by that name, so nothing was disabled.{suggestion}",
                denied.source.quoted(&denied.name)
            ));
        }
        for denied in &self.unavailable {
            notices.push(format!(
                "{}: nothing to disable — {} is not registered in this run.",
                denied.source.quoted(&denied.name),
                denied.name
            ));
        }
        notices
    }
}

/// Register the built-in tool set plus the LSP tools, wiring both LSP tools to the shared warm
/// service so they reuse running servers instead of spawning one per call.
///
/// `todo` is rooted at `<repo>/.luxe/todos` so a session's checklist persists.
/// Unregisters every tool the layered config disables, with `--no-tool`/`--with-tool` layered over
/// it (essential tools are never removed). Runs after built-ins + MCP register so a user can trim
/// any optional tool, including `mcp`.
///
/// Returns a [`ToolDenyReport`] rather than a list of names: `/tools` has to explain an absence,
/// and the reader has to be told when an instruction of theirs did nothing.
pub(crate) fn apply_tool_deny_list(
    registry: &mut ToolRegistry,
    repo_root: &std::path::Path,
    disable: &[String],
    enable: &[String],
) -> ToolDenyReport {
    let config = luxe_core::config::Config::load(repo_root).tools;
    let from_flag = |name: &String| disable.iter().any(|flag| flag == name);
    let mut report = ToolDenyReport::default();

    for name in config.effective_disabled(disable, enable) {
        let source = if from_flag(&name) { DenySource::Flag } else { DenySource::Config };
        let denied = DeniedTool { name, source };
        let name = denied.name.as_str();
        if luxe_core::config::ESSENTIAL_TOOLS.contains(&name) {
            // Refused rather than honored: an agent without these cannot do anything, so the
            // request is a misunderstanding of what the knob is for. Say so instead of
            // half-obeying.
            tracing::warn!(
                tool = %name,
                source = source.label(),
                "refusing to disable an essential tool (read/write/edit/bash)"
            );
            report.refused.push(denied);
        } else if registry.unregister(name) {
            tracing::info!(tool = %name, source = source.label(), "tool disabled");
            report.removed.push(denied);
        } else if is_known_tool(name) {
            // `debug`, `screenshot` and `ast_grep` register only where their program is installed,
            // and `mcp` only where a server is configured, so disabling one here is a no-op.
            tracing::debug!(
                tool = %name,
                source = source.label(),
                "nothing to disable: that tool is not registered on this machine"
            );
            report.unavailable.push(denied);
        } else {
            let nearest = nearest_tool(name);
            tracing::warn!(
                tool = %name,
                source = source.label(),
                nearest,
                "nothing to disable: no tool by that name"
            );
            report.unknown.push((denied, nearest));
        }
    }
    report
}

/// Every tool Luxe can register, whatever this machine happens to have installed.
///
/// The set that makes "no such tool" tellable from "not here": four tools register only where
/// their program exists (`debug`, `screenshot`, `ast_grep`) or where something is configured
/// (`mcp`), so a registry read on a bare CI box cannot answer the question a typo asks. The list
/// itself is [`luxe_core::tool_catalog`]; `the_catalog_covers_the_registry` keeps it honest.
fn is_known_tool(name: &str) -> bool {
    luxe_core::tool_catalog::get(name).is_some()
}

/// The tool `name` was probably meant to be, or `None` when it is nothing like any of them.
///
/// Punctuation and case are ignored first, because that is what the near misses look like:
/// `websearch`, `web-search` and `webSearch` are all one keystroke's worth of `web_search`.
/// A shared prefix catches the rest, the way an unrecognized hook point already does.
fn nearest_tool(name: &str) -> Option<&'static str> {
    let squash = |value: &str| {
        value
            .chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect::<String>()
    };
    let wanted = squash(name);
    if let Some(exact) = luxe_core::tool_catalog::names().find(|tool| squash(tool) == wanted) {
        return Some(exact);
    }
    let shared = |candidate: &str| {
        wanted.chars().zip(squash(candidate).chars()).take_while(|(a, b)| a == b).count()
    };
    luxe_core::tool_catalog::names()
        .filter(|candidate| shared(candidate) >= 3)
        .max_by_key(|candidate| shared(candidate))
}

/// What every tool is doing right now, for `/tools`.
///
/// Assembled after tiering so the three states are distinguishable from the outside: shipped in
/// every request, deferred behind `activate_tools`, or gone and by whose instruction.
pub(crate) fn collect_tool_statuses(
    registry: &ToolRegistry,
    removed: Vec<(String, String)>,
) -> Vec<luxe_core::ToolStatus> {
    use luxe_core::{ToolAvailability, ToolStatus};

    let core: std::collections::HashSet<&str> = registry.prompt_names().into_iter().collect();
    let registered: std::collections::HashSet<String> =
        registry.names().into_iter().map(str::to_string).collect();
    let mut statuses: Vec<ToolStatus> = registry
        .names()
        .into_iter()
        .map(|name| ToolStatus {
            availability: if !registry.is_tiered() || core.contains(name) {
                ToolAvailability::Active
            } else {
                ToolAvailability::Deferred
            },
            name: name.to_string(),
        })
        .collect();
    // A name cannot be both present and removed, and letting one be would list it twice and count
    // it in two totals. The registry is the authority; the removal list is a record of intent.
    statuses.extend(removed.into_iter().filter(|(name, _)| !registered.contains(name)).map(
        |(name, source)| ToolStatus { name, availability: ToolAvailability::Disabled { source } },
    ));
    statuses.sort_by(|a, b| a.name.cmp(&b.name));
    statuses
}

/// Build the callable tool set.
///
/// `loop_driven` is whether an autonomous loop is driving this engine. It gates `loop_control`,
/// which has nothing to control in an ordinary session — the same capability gate `ast_grep` and
/// `screenshot` use, since a tool that cannot work here should not be advertised here.
/// The checklist is state, kept with the project (`state_root`); the code graph indexes the files
/// the agent works on (`work_root`). They are one path unless the session has a worktree.
pub(super) fn build_tool_registry(
    state_root: &std::path::Path,
    work_root: &std::path::Path,
    lsp_service: SharedLspService,
    loop_driven: bool,
) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    let todo_root = state_root.join(".luxe").join("todos");
    // The shared read tracker also gates the LSP funnel reads below, so
    // `read_symbol`/`module_report` count toward the read-before-edit guard.
    let web = luxe_tools::web::WebConfig {
        clone_repositories: luxe_core::config::Config::load(state_root).tools.clone_repositories(),
        ..luxe_tools::web::WebConfig::from_env()
    };
    let read_tracker = luxe_tools::register_builtins(&mut registry, Some(todo_root), web);
    if loop_driven {
        registry.register(Arc::new(luxe_tools::loop_control::LoopControlTool::new()));
    }
    // Warm code graph shared by the impact tool and module_report's `blastRadius`.
    let graph_service = build_graph_service(work_root);
    registry.register(Arc::new(luxe_lsp::LspDiagnosticsTool::new(lsp_service.clone())));
    registry.register(Arc::new(
        luxe_lsp::LspNavigationTool::new(lsp_service.clone()).with_tracker(read_tracker.clone()),
    ));
    // Discovery funnel (symbol_search → module_report → read_symbol), LSP-backed. The two that
    // return file content share the read tracker so a subsequent edit isn't falsely blocked.
    registry.register(Arc::new(
        luxe_lsp::SymbolSearchTool::new(lsp_service.clone())
            .with_graph(graph_service.clone())
            .with_index(super::services::shared_symbol_index()),
    ));
    registry.register(Arc::new(
        luxe_lsp::ModuleReportTool::new(lsp_service.clone())
            .with_tracker(read_tracker.clone())
            .with_graph(graph_service.clone()),
    ));
    registry.register(Arc::new(
        luxe_lsp::ReadSymbolTool::new(lsp_service.clone()).with_tracker(read_tracker),
    ));
    // "What does changing X affect?" over the offline structural graph, with optional LSP-verified
    // callers (`verify: true`).
    registry.register(Arc::new(luxe_lsp::ImpactTool::new(graph_service).with_lsp(lsp_service)));
    // Native DAP debugger, backed by a warm per-workspace session service. Custom adapters from
    // ~/.config/luxe/debug-adapters.json extend the built-in language coverage.
    //
    // Registered only where a debug adapter is actually installed: it carries the largest schema in
    // the set (~780 tokens on every request), and with no adapter present every call it could make
    // would answer "no debug adapter found". This hides no capability — there is none to hide.
    let dap = shared_dap_service();
    if dap.has_adapter() {
        registry.register(Arc::new(luxe_dap::DebugTool::new(dap)));
    } else {
        tracing::debug!(
            "debug tool not registered: no DAP adapter installed (codelldb, lldb-dap, gdb, \
             debugpy, or dlv)"
        );
    }
    registry
}

/// Register the single `mcp` proxy tool over the servers this run may start.
///
/// Instead of eagerly connecting every server and registering every tool (thousands of context
/// tokens, whether used or not), we register one ~200-token proxy backed by a lazy
/// [`luxe_mcp::ServerRegistry`]: servers connect on first call, tool metadata is cached to
/// `.luxe/mcp-metadata.json` so discovery works cold, and idle servers disconnect. MCP servers
/// stay untrusted — calls are permission-gated and results are surfaced as data, not instructions.
pub(crate) async fn register_mcp_tools(registry: &mut ToolRegistry, repo_root: &std::path::Path) {
    use luxe_mcp::{McpProxyTool, ServerRegistry};

    let (servers, refused) = mcp_servers_for_run(repo_root);
    if !refused.is_empty() {
        tracing::warn!(
            servers = ?refused,
            "mcp: not starting servers this project declares; --trust-project, or \
             lsp.project_servers in the global config, allows them"
        );
    }
    let unlisted: Vec<&str> = servers
        .iter()
        .map(|server| server.id.as_str())
        .filter(|id| !luxe_core::mcp_client::is_plain_name(id))
        .collect();
    if !unlisted.is_empty() {
        tracing::warn!(
            servers = ?unlisted,
            "mcp: not listing these servers in the @ picker; an id there is letters, digits, \
             dots, dashes and underscores, at most 64"
        );
    }
    let cache_path = repo_root.join(".luxe").join("mcp-metadata.json");
    let manager = Arc::new(ServerRegistry::new(servers, Some(cache_path)));
    if !manager.has_servers() {
        return;
    }
    manager.start().await;
    tracing::info!(servers = ?manager.server_ids(), "mcp: registered proxy tool (lazy)");
    registry.register(Arc::new(McpProxyTool::new(manager)));
}

/// The MCP servers this run may start, and the ids of any the project declared that it may not.
///
/// A stdio server in `.mcp.json` is a command the repository chose, and an `eager` one must not
/// start the moment Luxe opens a cloned tree, headless included. It answers to the gate project
/// language servers and formatters already use, not a second one.
pub(crate) fn mcp_servers_for_run(
    repo_root: &std::path::Path,
) -> (Vec<luxe_core::mcp_client::McpServerConfig>, Vec<String>) {
    luxe_core::mcp_config::McpConfigStore::for_run(
        repo_root,
        super::services::project_tooling_allowed(),
    )
}

/// Default core tool set: every tool with demonstrated use, kept in every request. The rest are
/// deferred and listed in the prompt as available on demand. Override with `LUXE_CORE_TOOLS`
/// (comma-separated).
///
/// The split is drawn from measurement, not taste. A tool earns core by being called; one that is
/// rarely called and heavy in schema (`debug`, `lsp_navigation`, `screenshot`, `ast_grep`,
/// `impact`, `module_report`, `read_symbol`, `symbol_search`) is deferred. The sweeps found the
/// model rarely or never reached for these, and deferring them did not change what it answered;
/// `read_symbol` and `symbol_search` joined only once an adapter that loads a tool in place could
/// activate one without rewriting the cached prefix (where one cannot, see
/// [`default_core_tools`]). Deferred rather than removed: some models do reach for them now and
/// then, and a catalog line costs a fraction of a schema. `test` stays because deferring it
/// changed which tools the model reached for, which is the one thing this must not do.
///
/// `question` earns its place despite being rare: the communication contract asks the model to put
/// decisions through it, and deferring it means an activation round-trip at exactly the moment the
/// user is already waiting to be asked something.
///
/// `skill` is the one entry that is not in this list and is sometimes core anyway — see
/// [`apply_lazy_tools`].
const DEFAULT_CORE_TOOLS: &[&str] =
    &["read", "write", "edit", "bash", "grep", "glob", "todo", "git", "question", "test"];

/// Tier the registry: only the core set (plus the `activate_tools` meta-tool) ships in every
/// request, and the rest are deferred behind a one-line catalog in the prompt.
///
/// On by default — it cuts the per-request floor 29% (33,827 → 23,952 characters) and total turn
/// cost by about the same, with tool choice and task completion unchanged against the full set.
/// Set `LUXE_LAZY_TOOLS=0` to ship every schema every time.
pub(crate) fn apply_lazy_tools(registry: &mut ToolRegistry, repo_root: &std::path::Path) {
    if env_disabled("LUXE_LAZY_TOOLS") {
        return;
    }
    let core: std::collections::HashSet<String> = std::env::var("LUXE_CORE_TOOLS")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|value| {
            value.split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect()
        })
        .unwrap_or_else(|| default_core_tools(repo_root, registry));
    // Deferred = every currently-registered tool not in core (computed before adding the
    // meta-tool).
    let deferred: Vec<String> =
        registry.names().iter().filter(|n| !core.contains(**n)).map(|s| s.to_string()).collect();
    let count = deferred.len();
    registry.register(Arc::new(luxe_tools::activate::ActivateToolsTool::new(deferred)));
    registry.set_tiers(core);
    tracing::info!(deferred = count, "lazy tools enabled");
}

/// [`DEFAULT_CORE_TOOLS`], plus `skill` for anyone who has skills and `mcp` for anyone with a
/// server this run may reach.
///
/// The two tools whose tier depends on the machine rather than on measurement, and it is the same
/// cache arithmetic that says so. A deferred tool costs an `activate_tools` round-trip on first
/// use, and on a provider that can't load it in place (chat completions) that round-trip *changes
/// the tool list* — which invalidates the cached prompt prefix and re-bills everything in front of
/// it. Meanwhile the skill index is already in the prompt of anyone who has skills installed, so
/// the tool that index points at is not optional for them.
///
/// Decided at wiring time, at the same moment the index enters the prompt, so the two cannot
/// disagree within a session. An explicit `LUXE_CORE_TOOLS` is explicit and overrides both.
///
/// `mcp` follows the same arithmetic. It is 203 tokens, about 20 a request once cached, while
/// activating it mid-session where it has to be listed rewrites the whole cached prompt — 34,500
/// tokens' worth for a session 30,000 in — so the schema has to be carried for some 1,700 requests
/// to cost what one activation does. "Registered" is the definite test: [`register_mcp_tools`] adds
/// the tool only when the trust gate leaves a server, and the deny list has already had its say, so
/// a `--no-tool mcp` run never sees it here. A server whose login has expired still counts;
/// dropping it would change the tool list mid-session, the break this exists to avoid.
fn default_core_tools(
    repo_root: &std::path::Path,
    registry: &ToolRegistry,
) -> std::collections::HashSet<String> {
    let mut core: std::collections::HashSet<String> =
        DEFAULT_CORE_TOOLS.iter().map(|name| (*name).to_string()).collect();
    if !luxe_core::skills::discover(repo_root).is_empty() {
        core.insert("skill".to_string());
    }
    if registry.names().contains(&"mcp") {
        core.insert("mcp".to_string());
    }
    core
}

#[cfg(test)]
mod tests;
