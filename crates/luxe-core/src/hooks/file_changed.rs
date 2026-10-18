//! Built-in hook that runs diagnostics and formatters when a file changes.
//!
//! Listens for `FileChanged` events and, when configured:
//! - Runs the appropriate formatter for the changed file's language (format-on-save).
//! - Runs LSP diagnostics on the changed file, surfacing what the edit introduced to the agent.
//!
//! # New problems, not every problem
//!
//! Every diagnostic in the file after each edit is the wrong report: in a codebase the language
//! server only half understands — Django under pyright, without the virtualenv — it is a dozen
//! errors the edit had nothing to do with, on every edit, re-read on every later turn, with an
//! instruction to chase them. So before the first `edit` or `write` of a file the hook
//! records what was already wrong, and after each change it reports only what is new and counts
//! the rest. The comparison ignores line numbers, since an edit moves everything below it.

mod standards;

pub use standards::{FunctionSize, OutlineSource, Scope};

use crate::formatter::FormatterRegistry;
use crate::hook::{Hook, HookCtx, HookEvent, HookOutcome, HookPoint};
use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{debug, warn};

/// Reports changed symbols that no test covers, for the file just edited.
///
/// A test gap is *work*, not review information. Handing it to the human at review time makes them
/// find the warning, read it, and retype it as a prompt; telling the agent while it is still editing
/// lets it decide — write the test, or say why the change does not need one. Implemented outside
/// `luxe-core` (the code graph lives in `luxe-lsp`) and injected here, exactly like review insight.
#[async_trait]
pub trait TestGapAdvisor: Send + Sync {
    /// Changed symbols in `path` with no covering test, as `path::Symbol`. Empty when there are none
    /// or the graph cannot say.
    async fn untested_symbols(&self, path: &std::path::Path) -> Vec<String>;
}

/// A hook that runs formatters and diagnostics on file changes.
pub struct FileChangedHook {
    /// Optional formatter registry for format-on-save.
    formatter_registry: Option<FormatterRegistry>,
    /// Optional LSP client for running diagnostics
    lsp_client: Option<Arc<dyn crate::lsp::DiagnosticsProvider>>,
    /// Optional shared cache the latest per-file diagnostics are published to for the dock.
    diagnostics_store: Option<crate::lsp::DiagnosticsStore>,
    /// Optional source of test-coverage gaps for the file just edited.
    test_gaps: Option<Arc<dyn TestGapAdvisor>>,
    /// What each file's diagnostics were before the change being reported, by absolute path
    baselines: std::sync::Mutex<std::collections::HashMap<PathBuf, Vec<crate::lsp::Diagnostic>>>,
    /// Where function boundaries come from, for the size standards
    outline: Option<Arc<dyn OutlineSource>>,
    /// Each file's size before the change being reported, by absolute path
    sizes: std::sync::Mutex<std::collections::HashMap<PathBuf, standards::SizeSnapshot>>,
}

impl FileChangedHook {
    /// Create a new `FileChangedHook` with optional formatter registry and LSP client.
    pub fn new(
        formatter_registry: Option<FormatterRegistry>,
        lsp_client: Option<Arc<dyn crate::lsp::DiagnosticsProvider>>,
    ) -> Self {
        Self {
            formatter_registry,
            lsp_client,
            diagnostics_store: None,
            test_gaps: None,
            baselines: std::sync::Mutex::default(),
            outline: None,
            sizes: std::sync::Mutex::default(),
        }
    }

    /// Tell the agent when a change takes a file past 500 lines or a function past 20.
    pub fn with_outline(mut self, outline: Arc<dyn OutlineSource>) -> Self {
        self.outline = Some(outline);
        self
    }

    /// Report changed symbols with no covering test to the agent as it edits.
    pub fn with_test_gaps(mut self, advisor: Arc<dyn TestGapAdvisor>) -> Self {
        self.test_gaps = Some(advisor);
        self
    }

    /// Publish each changed file's diagnostics to `store` so the Context Dock can show them.
    pub fn with_diagnostics_store(mut self, store: crate::lsp::DiagnosticsStore) -> Self {
        self.diagnostics_store = Some(store);
        self
    }
}

#[async_trait]
impl Hook for FileChangedHook {
    fn name(&self) -> &'static str {
        "file-changed"
    }

    fn interest(&self) -> &[HookPoint] {
        &[HookPoint::FileChanged, HookPoint::BeforeToolUse]
    }

    async fn on(&self, _point: HookPoint, ev: &HookEvent, _cx: &HookCtx) -> HookOutcome {
        if let HookEvent::ToolUse { tool_name, args } = ev {
            self.record_baseline(tool_name, args).await;
            return HookOutcome::Continue;
        }
        let HookEvent::FileChanged { path, operation } = ev else {
            return HookOutcome::Continue;
        };

        let path_buf = PathBuf::from(path);
        // A formatter loads plugins named by the nearest config and a language server runs build
        // scripts, and in a fetched repository both of those came from a stranger
        if crate::fetched::contains(&path_buf) {
            debug!(path, "file_changed: inside a fetched repository; not formatting or checking");
            return HookOutcome::Continue;
        }
        debug!(path, operation, "file_changed: running formatter + diagnostics");

        // Run the formatter if configured and a formatter exists for this language.
        if let Some(registry) = &self.formatter_registry {
            if registry.formatter_for(&path_buf).is_some() {
                debug!(path, "running formatter");
                match crate::formatter::run_formatter(&path_buf, registry).await {
                    Ok(()) => debug!(path, "formatted successfully"),
                    Err(e) => {
                        // Formatter failure is non-fatal — skip silently with a note.
                        warn!(error = %e, path, "formatter failed; skipping");
                    }
                }
            }
        }

        // Diagnostics first: something being broken outranks something being too long, and both
        // outrank something being untested
        let mut notes = Vec::new();
        if let Some(lsp_client) = &self.lsp_client {
            match lsp_client.run_diagnostics(&path_buf).await {
                Ok(diagnostics) => {
                    // Publish to the dock cache even when empty, so a fixed file drops out.
                    if let Some(store) = &self.diagnostics_store {
                        store.set(path_buf.clone(), diagnostics.clone());
                    }
                    let before =
                        self.lock_baselines().insert(absolute(&path_buf), diagnostics.clone());
                    let report = match &before {
                        Some(before) => format_new_diagnostics(path, before, &diagnostics),
                        None if diagnostics.is_empty() => None,
                        None => Some(format_diagnostics(path, &diagnostics)),
                    };
                    notes.extend(report);
                }
                Err(e) => {
                    debug!(error = %e, path, "file_changed: failed to run diagnostics");
                }
            }
        }

        let broken = !notes.is_empty();
        notes.extend(self.size_note(path));

        // Nothing is broken; is anything untested? Reported once per edit, bounded, and phrased as a
        // fact rather than an instruction — the agent decides what it is worth.
        if let (false, Some(advisor)) = (broken, &self.test_gaps) {
            let untested = advisor.untested_symbols(&path_buf).await;
            notes.extend(format_test_gaps(path, &untested));
        }

        match notes.is_empty() {
            true => HookOutcome::Continue,
            false => HookOutcome::InjectContext(notes.join("\n")),
        }
    }
}

impl FileChangedHook {
    /// Records what is already wrong with the file an `edit` or `write` is about to change, the
    /// first time this session touches it, so the report afterwards can tell its problems apart.
    async fn record_baseline(&self, tool: &str, args: &serde_json::Value) {
        if !matches!(tool, "edit" | "write") {
            return;
        }
        let Some(path) = args.get("path").and_then(|path| path.as_str()) else {
            return;
        };
        let path = crate::tool::ToolContext::working_dir().join(path);
        let key = absolute(&path);
        if crate::fetched::contains(&path) {
            return;
        }
        if let Some(outline) = &self.outline {
            let mut sizes = self.lock_sizes();
            if !sizes.contains_key(&key) {
                let source = std::fs::read_to_string(&path).unwrap_or_default();
                sizes.insert(key.clone(), standards::snapshot(outline.as_ref(), &path, &source));
            }
        }
        let Some(lsp_client) = &self.lsp_client else {
            return;
        };
        if self.lock_baselines().contains_key(&key) {
            return;
        }
        // A file that does not exist yet has nothing wrong with it
        let before = if path.is_file() {
            match lsp_client.run_diagnostics(&path).await {
                Ok(diagnostics) => diagnostics,
                Err(error) => {
                    debug!(%error, path = %path.display(), "no diagnostics baseline");
                    return;
                }
            }
        } else {
            Vec::new()
        };
        self.lock_baselines().insert(key, before);
    }

    fn lock_sizes(
        &self,
    ) -> std::sync::MutexGuard<'_, std::collections::HashMap<PathBuf, standards::SizeSnapshot>>
    {
        self.sizes.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The size limits `path` crossed in this change, measured against the last one.
    fn size_note(&self, path: &str) -> Option<String> {
        let outline = self.outline.as_ref()?;
        let path_buf = PathBuf::from(path);
        let source = std::fs::read_to_string(&path_buf).ok()?;
        let after = standards::snapshot(outline.as_ref(), &path_buf, &source);
        // No record of the file before means no change was seen coming, so nothing to compare
        let before = self.lock_sizes().insert(absolute(&path_buf), after.clone())?;
        standards::crossings(path, &before, &after)
    }

    fn lock_baselines(
        &self,
    ) -> std::sync::MutexGuard<'_, std::collections::HashMap<PathBuf, Vec<crate::lsp::Diagnostic>>>
    {
        self.baselines.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// One spelling per file, so a relative argument and an absolute event path meet.
///
/// A file `write` has not created yet cannot canonicalize, so its folder does instead: otherwise
/// the record taken before it existed and the report after would land on different keys, which on
/// Windows, where canonical paths gain a `\\?\` prefix, they always did.
fn absolute(path: &std::path::Path) -> PathBuf {
    if let Ok(resolved) = path.canonicalize() {
        return resolved;
    }
    match (path.parent().map(std::path::Path::canonicalize), path.file_name()) {
        (Some(Ok(folder)), Some(name)) => folder.join(name),
        _ => path.to_path_buf(),
    }
}

/// The diagnostics in `after` that were not in `before`, matched on severity and message rather
/// than position, as a report — or `None` when the change introduced nothing.
fn format_new_diagnostics(
    path: &str,
    before: &[crate::lsp::Diagnostic],
    after: &[crate::lsp::Diagnostic],
) -> Option<String> {
    let mut remaining: Vec<&crate::lsp::Diagnostic> = before.iter().collect();
    let mut new = Vec::new();
    for diagnostic in after {
        let seen = remaining.iter().position(|old| {
            old.severity == diagnostic.severity && old.message == diagnostic.message
        });
        match seen {
            Some(at) => {
                remaining.swap_remove(at);
            }
            None => new.push(diagnostic.clone()),
        }
    }
    if new.is_empty() {
        return None;
    }
    let mut report = render_diagnostics(path, &new, "new");
    let already = after.len() - new.len();
    if already > 0 {
        report.push_str(&format!(
            "({already} more were in the file before this change and are not listed.)\n"
        ));
    }
    Some(report)
}

/// Most untested symbols named inline; beyond this the list is counted.
const MAX_INJECTED_TEST_GAPS: usize = 5;

/// `src/auth.rs: logout has no covering test.` — or `None` when everything changed is covered.
fn format_test_gaps(path: &str, untested: &[String]) -> Option<String> {
    if untested.is_empty() {
        return None;
    }
    let names: Vec<&str> = untested
        .iter()
        .take(MAX_INJECTED_TEST_GAPS)
        .map(|symbol| symbol.rsplit("::").next().unwrap_or(symbol))
        .collect();
    let overflow = untested.len().saturating_sub(names.len());
    let mut note = format!("{path}: no covering test for {}", names.join(", "));
    if overflow > 0 {
        note.push_str(&format!(" (+{overflow} more)"));
    }
    note.push_str(
        ". Add one, or say why this change does not need it — a reviewer will be told the same thing.",
    );
    Some(note)
}

/// Max diagnostics surfaced inline after an edit. The rest are summarized so a noisy file can't
/// flood the model's context; the `lsp_diagnostics` tool always returns the full list.
const MAX_INJECTED_DIAGNOSTICS: usize = 20;

/// Render diagnostics for injection: errors first, capped, with a summary tail when truncated.
///
/// Surfacing the just-edited file's diagnostics closes the edit→verify loop (the model sees what
/// its change broke without being told to check). Ordering errors first + a cap keeps a file with
/// many pre-existing warnings from drowning the real errors or flooding the context window.
fn format_diagnostics(path: &str, diagnostics: &[crate::lsp::Diagnostic]) -> String {
    render_diagnostics(path, diagnostics, "total")
}

/// [`format_diagnostics`], with what the count counts: `total`, or `new` for a delta.
fn render_diagnostics(path: &str, diagnostics: &[crate::lsp::Diagnostic], counted: &str) -> String {
    let mut sorted: Vec<&crate::lsp::Diagnostic> = diagnostics.iter().collect();
    sorted.sort_by_key(|d| (d.severity.is_none(), d.severity, d.line, d.character));
    let errors = sorted.iter().filter(|d| d.severity == Some(crate::lsp::Severity::Error)).count();
    let mut message = format!(
        "Diagnostics after editing {path} ({} {counted}, {errors} error(s)) — fix errors before continuing:\n",
        diagnostics.len()
    );
    for diag in sorted.iter().take(MAX_INJECTED_DIAGNOSTICS) {
        message.push_str(&format!(
            "- {} {}:{}: {}\n",
            diag.severity.map_or("note", crate::lsp::Severity::label),
            diag.line + 1,
            diag.character + 1,
            diag.message
        ));
    }
    if sorted.len() > MAX_INJECTED_DIAGNOSTICS {
        message.push_str(&format!(
            "… and {} more (run `lsp_diagnostics` on {path} for the full list)\n",
            sorted.len() - MAX_INJECTED_DIAGNOSTICS
        ));
    }
    message
}

#[cfg(test)]
mod tests;
