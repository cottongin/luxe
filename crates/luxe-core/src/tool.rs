//! Tool trait + registry — the agent's capability surface.
//!
//! Tools are async functions with a JSON-schema description. The registry maps names to
//! tool instances and produces the `tools` array for provider requests.

use async_trait::async_trait;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

use crate::message::ContentBlock;
use crate::provider::ToolSchema;

/// Name of the built-in meta-tool that activates deferred tools on demand (lazy-tools mode).
pub const ACTIVATE_TOOLS_NAME: &str = "activate_tools";

/// The phrases a write guard's refusal carries.
///
/// A tool card names a guard failure by finding one of these in the result
/// ([`crate::activity::failure_reason`]), so the tools format their messages from them rather than
/// restating the words: rewording one side alone would turn the card back into a bare "Failed".
pub mod guards {
    /// An `edit` of a file with no read on record
    pub const READ_BEFORE_EDIT: &str = "read-before-edit guard";
    /// A `write` over an existing file with no read on record
    pub const READ_BEFORE_WRITE: &str = "read-before-write guard";
    /// A write to a file that changed after it was read
    pub const CHANGED_ON_DISK: &str = "changed on disk since";
}

/// How the result of a call that never ran opens, by who stopped it.
///
/// Read back by [`crate::activity::failure_reason`] to say who refused the call.
pub mod denial {
    /// The reader turned the call down
    pub const BY_USER: &str = "Tool call denied by user";
    /// The permission rules refused it
    pub const BY_POLICY: &str = "Tool call denied by policy";
}

/// The bound for a tool that states no preference of its own.
///
/// Generous, because tripping a legitimate call is the worse failure: an LSP sweep or an `ast_grep`
/// pass over a large repo can take a while, and losing that work to an impatient harness is not an
/// improvement. Short enough that a wedged call costs two minutes instead of the session.
pub const DEFAULT_TOOL_TIMEOUT: Duration = Duration::from_secs(120);

/// The bound for a tool whose work is a network round-trip.
///
/// Tighter than [`DEFAULT_TOOL_TIMEOUT`], because there is no such thing as a slow-but-fine page
/// render: past a minute the far end is not answering and the only thing left to do is say so. The
/// case that earned this constant is a host that completes the TCP handshake and then goes silent —
/// nothing below the socket ever returns.
pub const NETWORK_TOOL_TIMEOUT: Duration = Duration::from_secs(60);

/// Environment override for every declared tool bound.
const TIMEOUT_ENV: &str = "LUXE_TOOL_TIMEOUT_SECS";

/// The deadline actually applied to a call of `tool`.
///
/// `TIMEOUT_ENV` replaces every declared bound and grants none: a tool that opted out did so
/// because nothing the harness can measure applies to it, and a blanket override must not start
/// killing a permission prompt the user is still reading. An invalid or zero value is ignored, so a
/// typo cannot silently remove the bounds.
pub fn effective_timeout(tool: &dyn Tool) -> Option<Duration> {
    let declared = tool.timeout()?;
    let from_env = std::env::var(TIMEOUT_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|secs| *secs > 0);
    Some(from_env.map_or(declared, Duration::from_secs))
}

/// The first spelling of an argument that a call actually carries, canonical name first.
///
/// Every advertised parameter is camelCase, because a name ending in `_text` corrupted two in five
/// `edit` calls on the wire, and the same underscore is a needless risk everywhere else. A
/// snake_case name from an older build therefore stays *accepted*: a resumed session replays the
/// retired spelling out of its own transcript, and the failure mode for an optional argument is not
/// an error but a silent drop — the tool does something subtly different and says nothing.
///
/// One owner rather than a two-line rediscovery at each call site, for the reason every other
/// cross-cutting question here has one: the site that forgets is the site nobody looks at.
pub fn arg<'a>(args: &'a serde_json::Value, names: &[&str]) -> Option<&'a serde_json::Value> {
    names.iter().find_map(|name| args.get(*name))
}

/// Lazy-tools tiering policy: which tools are always exposed (`core`). Everything else is deferred.
///
/// Deferring situational tools shrinks the request's `tools` array (the dominant floor cost) and
/// improves tool-selection accuracy (which degrades past ~30–50 tools); the model activates a
/// deferred tool on demand and then calls it normally. Which tools a session has activated is not
/// held here: it's read off the transcript ([`crate::tool_activation`]), so a resumed or forked
/// session carries exactly what its own history loaded.
#[derive(Clone)]
pub struct ToolTiers {
    core: HashSet<String>,
}

/// Result data from a tool invocation.
#[derive(Debug, Clone)]
pub struct ToolResultData {
    /// Content chunks produced by the tool.
    pub content: Vec<ContentBlock>,
    /// Whether this is an error result.
    pub is_error: bool,
}

impl ToolResultData {
    /// A successful result of one block of text.
    pub fn text(text: impl Into<String>) -> Self {
        Self { content: vec![ContentBlock::Text(text.into())], is_error: false }
    }

    /// A failed result of one block of text, said as written: a caller that wants an `Error:`
    /// prefix writes it.
    pub fn error(message: impl Into<String>) -> Self {
        Self { content: vec![ContentBlock::Text(message.into())], is_error: true }
    }
}

/// Read-policy hook for files a tool discovers after its initial scope check.
///
/// A tool must consult this before opening discovered content, not after returning it. `Ask`
/// remains an approval requirement; it is not an implicit allowance.
pub trait FileReadHook: Send + Sync {
    /// The `read` decision for `path`, a concrete file about to be opened, relative or absolute.
    fn decision(&self, path: &std::path::Path) -> crate::permission::PermissionAction;
}

/// Context passed to tools at runtime.
#[derive(Debug, Clone)]
pub struct ToolContext {
    /// Current working directory.
    pub cwd: std::path::PathBuf,
    /// Session ID for tracking read-guard coverage.
    pub session_id: String,
    /// Cancellation flag (checked periodically by long-running tools).
    pub cancelled: Arc<tokio::sync::watch::Receiver<bool>>,
    /// Event sink for emitting progress updates.
    pub event_tx: mpsc::Sender<crate::event::Event>,
    /// Hook context for emitting cross-cutting events (optional).
    pub hook_ctx: Option<crate::hook::HookCtx>,
    /// Interactive-question broker. `Some` only when a front-end can prompt (the interactive TUI);
    /// headless/loop runs leave it `None` so the `question` tool resolves to its default instead of
    /// blocking on a human.
    pub question_broker: Option<Arc<crate::question::QuestionBroker>>,
}

impl ToolContext {
    /// Where a tool runs, and therefore where a shell command starts.
    ///
    /// One owner, because two unrelated places need the same answer and a drift between them is
    /// invisible: dispatch builds every [`ToolContext`] from this, and the card that describes a
    /// `bash` call reads it to tell a real `cd` apart from one into the directory the command was
    /// already in.
    pub fn working_dir() -> std::path::PathBuf {
        std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
    }

    /// Fire `FileChanged` for a file this call just wrote, returning any context a hook injected.
    ///
    /// Format-on-save and diagnostics run here, before it returns, so fingerprint the file after.
    /// Every tool that writes goes through this one call, so none can forget the hook.
    pub async fn file_changed(&self, path: &std::path::Path, operation: &str) -> Option<String> {
        let hook_ctx = self.hook_ctx.as_ref()?;
        let event = crate::hook::HookEvent::FileChanged {
            path: crate::paths::display(path),
            operation: operation.to_string(),
        };
        match hook_ctx
            .hook_registry
            .invoke(crate::hook::HookPoint::FileChanged, &event, hook_ctx)
            .await
        {
            crate::hook::HookOutcome::InjectContext(context) => Some(context),
            _ => None,
        }
    }
}

/// The Tool trait — implements a callable capability.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Human-readable tool name (must be unique in the registry).
    fn name(&self) -> &str;

    /// JSON-schema description of this tool's arguments.
    fn schema(&self) -> ToolSchema;

    /// How long one call may run before the engine stops waiting for it.
    ///
    /// Enforced at the dispatch choke point rather than inside each tool, because "every tool
    /// remembers to bound itself" is the arrangement that produced a headless browser aimed at a
    /// silent host with nothing to stop it. Overriding this is a claim about *this tool's* work.
    ///
    /// `None` opts out, and exactly two kinds of tool have earned it: one that waits on a human, and
    /// one that owns a process **group** the engine cannot reach, which therefore has to enforce its
    /// own deadline and do its own killing.
    fn timeout(&self) -> Option<Duration> {
        Some(DEFAULT_TOOL_TIMEOUT)
    }

    /// Argument names this tool accepts beyond those its schema declares: another harness's
    /// spelling it reads on purpose, like `edit`'s `file_path`.
    ///
    /// Declared so [`stray_arguments`] does not report them as ignored. Spellings that differ only
    /// in case or underscores (`active_form` for `activeForm`) need no entry.
    fn argument_aliases(&self) -> &'static [&'static str] {
        &[]
    }

    /// Run the tool with the given arguments.
    async fn run(&self, args: serde_json::Value, cx: &ToolContext) -> ToolResultData;

    /// Run with read policy for files discovered during execution.
    ///
    /// Override when a bulk operation opens files beyond the initially named roots. The default
    /// preserves existing single-target tools. Args: `(args, cx, read_hook)`.
    async fn run_with_read_hook(
        &self,
        args: serde_json::Value,
        cx: &ToolContext,
        _read_hook: &dyn FileReadHook,
    ) -> ToolResultData {
        self.run(args, cx).await
    }
}

/// The arguments a call carries that its tool neither declares nor reads as an alias, each with the
/// declared argument it most likely meant.
///
/// Every tool reads its arguments by name, so one it does not know is dropped without a word, and
/// a misspelled scope is the worst case: `ast_grep` given `path` instead of `paths` searched the
/// whole project and answered confidently. Reported rather than refused, because the call may well
/// have done what was wanted with the arguments it did understand.
pub fn stray_arguments(
    schema: &crate::provider::ToolSchema,
    aliases: &[&str],
    args: &serde_json::Value,
) -> Vec<(String, Option<String>)> {
    let fold = |name: &str| name.replace('_', "").to_lowercase();
    let (Some(given), Some(declared)) = (
        args.as_object(),
        schema.parameters.get("properties").and_then(serde_json::Value::as_object),
    ) else {
        return Vec::new();
    };
    let known: Vec<String> = declared
        .keys()
        .map(|name| fold(name))
        .chain(aliases.iter().map(|name| fold(name)))
        .collect();
    given
        .keys()
        .filter(|name| !known.contains(&fold(name)))
        .map(|name| (name.clone(), nearest_name(name, declared.keys().map(String::as_str))))
        .collect()
}

/// The note a result carries for [`stray_arguments`], or `None` when there are none.
pub fn stray_note(tool: &str, stray: &[(String, Option<String>)]) -> Option<String> {
    if stray.is_empty() {
        return None;
    }
    let lines: Vec<String> = stray
        .iter()
        .map(|(name, nearest)| {
            let hint =
                nearest.as_ref().map(|real| format!(" Did you mean `{real}`?")).unwrap_or_default();
            format!("Note: `{name}` is not an argument of `{tool}`, so it was ignored.{hint}")
        })
        .collect();
    Some(lines.join("\n"))
}

/// The declared name closest to `name`, when one is close enough to be what was meant.
fn nearest_name<'a>(name: &str, declared: impl Iterator<Item = &'a str>) -> Option<String> {
    let name = name.to_lowercase();
    declared
        .map(|candidate| {
            let lower = candidate.to_lowercase();
            let contained = lower.contains(&name) || name.contains(&lower);
            let distance = if contained { 1 } else { edit_distance(&name, &lower) };
            (distance, candidate)
        })
        .filter(|(distance, candidate)| *distance <= (candidate.len() / 3).max(2))
        .min_by_key(|(distance, _)| *distance)
        .map(|(_, candidate)| candidate.to_string())
}

/// Levenshtein distance, for names a few characters long.
fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let substitute = previous + usize::from(ca != *cb);
            previous = row[j + 1];
            row[j + 1] = substitute.min(row[j] + 1).min(row[j + 1] + 1);
        }
    }
    row[b.len()]
}

/// Registry mapping tool names to instances.
#[derive(Default)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
    /// Lazy tiering; `None` (the default) exposes every tool in every request.
    tiers: Option<ToolTiers>,
}

impl ToolRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self { tools: HashMap::new(), tiers: None }
    }

    /// Enable lazy tiering: only `core` tools (plus the `activate_tools` meta-tool) are always in
    /// request schemas; the rest are listed in the prompt as available on demand and loaded when
    /// the model activates them. Idempotent; safe to call once at wiring time.
    pub fn set_tiers(&mut self, core: HashSet<String>) {
        let mut core = core;
        core.insert(ACTIVATE_TOOLS_NAME.to_string());
        self.tiers = Some(ToolTiers { core });
    }

    /// Whether lazy tiering is active.
    pub fn is_tiered(&self) -> bool {
        self.tiers.is_some()
    }

    /// Register a tool instance.
    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    /// Get a tool by name.
    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(|t| t.as_ref())
    }

    /// Remove a tool by name, returning whether it was registered. Used to honor the config's
    /// per-tool deny-list after the built-ins register.
    pub fn unregister(&mut self, name: &str) -> bool {
        self.tools.remove(name).is_some()
    }

    /// List all registered tool names, sorted.
    ///
    /// Sorted so the derived `## Tools` prompt section is byte-stable across sessions; the
    /// backing map is a `HashMap` with per-process randomized iteration order.
    pub fn names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.tools.keys().map(|s| s.as_str()).collect();
        names.sort_unstable();
        names
    }

    /// Produce the full `tools` array (every registered tool), sorted by name.
    ///
    /// Deterministic ordering keeps the request prefix byte-identical across sessions so
    /// provider prompt-caching actually hits — `HashMap` iteration order is randomized per
    /// process, which would otherwise make the ~5k-token tool block a cache miss every session.
    pub fn schemas(&self) -> Vec<ToolSchema> {
        let mut schemas: Vec<ToolSchema> = self.tools.values().map(|t| t.schema()).collect();
        schemas.sort_by(|a, b| a.name.cmp(&b.name));
        schemas
    }

    /// The `tools` list for a request: every tool when untiered, else core, the activate meta-tool,
    /// and whichever deferred tools are in `activated`.
    ///
    /// `activated` comes off the transcript ([`crate::tool_activation::activated_in`]), so it's
    /// recomputed per request and a mid-turn activation shows up on the next provider call. Names
    /// that aren't registered deferred tools are ignored.
    pub fn request_schemas(&self, activated: &BTreeSet<String>) -> Vec<ToolSchema> {
        let Some(tiers) = &self.tiers else { return self.schemas() };
        let mut schemas: Vec<ToolSchema> = self
            .tools
            .values()
            .filter(|t| tiers.core.contains(t.name()) || activated.contains(t.name()))
            .map(|t| t.schema())
            .collect();
        schemas.sort_by(|a, b| a.name.cmp(&b.name));
        schemas
    }

    /// Every deferred tool's schema, sorted by name, activated or not. Empty when untiered.
    ///
    /// What [`crate::provider::CompletionRequest::deferred_tools`] carries: an adapter that can
    /// load a tool where `activate_tools` asked for it sends these without growing the cached
    /// `tools` list.
    pub fn deferred_schemas(&self) -> Vec<ToolSchema> {
        let Some(tiers) = &self.tiers else { return Vec::new() };
        let mut schemas: Vec<ToolSchema> = self
            .tools
            .values()
            .filter(|t| !tiers.core.contains(t.name()))
            .map(|t| t.schema())
            .collect();
        schemas.sort_by(|a, b| a.name.cmp(&b.name));
        schemas
    }

    /// Names the system prompt may reason about: all when untiered, else the **core** tier alone.
    ///
    /// Deliberately not the activated set, which grows as the model loads tools. The prompt has to
    /// be byte-identical between turns of a session or every activation re-bills the cached prefix
    /// at the write rate, and `luxe wire` caught exactly that: the `## Tools` section rewritten
    /// mid-session because one more tool had become callable. The core tier is fixed when the
    /// registry is tiered, so a prompt built from it says the same thing on turn one and turn fifty
    /// — which is also what it said before anything was activated.
    pub fn prompt_names(&self) -> Vec<&str> {
        let Some(tiers) = &self.tiers else { return self.names() };
        let mut names: Vec<&str> = self
            .tools
            .keys()
            .filter(|n| tiers.core.contains(n.as_str()))
            .map(|s| s.as_str())
            .collect();
        names.sort_unstable();
        names
    }

    /// `(name, description)` for every deferred tool — the prompt's on-demand catalog. Empty when
    /// untiered. Description is the tool's full schema description; the prompt truncates it to a
    /// short line.
    ///
    /// Activation does not remove an entry, for the same reason [`Self::prompt_names`] ignores it:
    /// a catalog that shrinks as the session goes rewrites the system prompt underneath a cached
    /// prefix. An activated tool listed here is still true — it exists, and it is now loaded.
    pub fn deferred_catalog(&self) -> Vec<(String, String)> {
        let Some(tiers) = &self.tiers else { return Vec::new() };
        let mut out: Vec<(String, String)> = self
            .tools
            .values()
            .filter(|t| !tiers.core.contains(t.name()))
            .map(|t| {
                let schema = t.schema();
                (schema.name, schema.description)
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }
}

#[cfg(test)]
mod tests;
