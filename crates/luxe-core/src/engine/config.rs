//! Config — what an engine is spawned with.

use super::*;

/// Configuration for the engine.
pub struct EngineConfig {
    /// Session store for persisting messages.
    pub session_store: std::sync::RwLock<Box<dyn SessionStore + Send + Sync>>,
    /// Provider instance (e.g., openai_compat). The default the engine speaks through and the
    /// fallback when `providers` has no adapter for the active model's provider.
    pub provider: Arc<dyn Provider + Send + Sync>,
    /// Additional provider adapters keyed by provider name (e.g. `anthropic`, `openai`).
    ///
    /// Built by the front-end from every configured provider so an in-session model switch
    /// (`Action::SetModel`) that also changes provider can route to the right adapter without
    /// rebuilding the engine. Empty in tests/embedders, which then always use `provider`.
    pub providers: HashMap<String, Arc<dyn Provider + Send + Sync>>,
    /// Base model the provider speaks through, until `SetModel` or a mode with a model of its own
    /// replaces it.
    pub model: crate::provider::ModelInfo,
    pub tool_registry: ToolRegistry,
    /// Hook registry for cross-cutting behavior.
    pub hook_registry: HookRegistry,
    pub session_id: SessionId,
    pub system_prompt: Option<String>,
    /// What the front-end found wrong before the engine existed, said once when the session opens.
    ///
    /// A CLI cannot print these itself and be read by everyone: the TUI owns the terminal, so a
    /// line on stderr lands on the drawing surface. Emitted as [`Event::Notice`] after the session
    /// has loaded — anything earlier is wiped by `SessionLoaded` — so every front-end gets them
    /// the same way it gets every other notice.
    pub startup_notices: Vec<String>,
    /// Durable todo store, read for the one-time resume briefing.
    pub todo_store: Option<TodoStore>,
    pub compaction_settings: CompactionSettings,
    /// Permission policy + how an `ask` is resolved. Defaults to permissive-with-guardrails +
    /// auto-allow (never blocks); the CLI sets prompt mode for the interactive TUI.
    pub permission: crate::permission::PermissionSettings,
    /// The modes the session can be in; it starts in the first.
    pub modes: crate::mode::Modes,
    /// Who answers [`Action::RequestReview`]. `None` makes a request say no reviewer is set up,
    /// which is right for embedders, tests and a review engine itself.
    pub reviewer: Option<OnDemandReview>,
    /// Where this engine's spend is reported in addition to its own session ledger.
    ///
    /// A ledger measures one session and is rebuilt whenever a session is loaded, so a run that
    /// spans sessions — a continuous loop and the separate engine its phase reviews run in — needs
    /// somewhere else to add up. `None` for an ordinary engine.
    pub spend_sink: Option<std::sync::Arc<dyn crate::usage::SpendSink>>,
    /// Formatter registry for format-on-save (optional).
    pub formatter_registry: Option<crate::formatter::FormatterRegistry>,
    /// Project root used to capture immutable Guided Review sources.
    ///
    /// `None` disables runtime review loading for embedding and test callers.
    pub guided_review_project_root: Option<std::path::PathBuf>,
    /// The directory Luxe was launched in, when that is not the project root.
    ///
    /// The one thing the *shell's* directory still answers after [`crate::project::discover`] has
    /// moved the process to the top of the working copy: which instruction files apply. A monorepo
    /// keeps shared rules at the root and specific ones beside the code, so someone who ran `luxe`
    /// in `packages/app` gets both — general first, nearest last. `None` reads the process's own
    /// directory, which is the right answer for an embedder that never moved.
    pub invoked_from: Option<std::path::PathBuf>,
    /// This session's record of the work it did outside the project.
    ///
    /// Read only to answer a peek at a file that belongs to no repository: Git cannot say what such
    /// a file held before, and the copy this record retained before the first write can. `None`
    /// leaves those peeks reporting that there is nothing to compare against — the honest answer
    /// for an embedder that never collected one.
    pub external_work: Option<crate::external_work::ExternalWork>,
    /// Extra project-relative planning files a review reads for context (`review.context_files`).
    pub review_context_files: Vec<String>,
    /// Project-relative globs a review leaves out (`review.exclude`), on top of `.luxe/`.
    pub review_exclude: Vec<String>,
    /// Whether an in-session model/thinking choice is remembered for the next launch
    /// (`agent.remember_model`). Off for embedders and tests, which should leave no trace.
    pub remember_runtime_state: bool,
    /// When the prompt cache is kept warm through a long wait (`agent.cache_warming`).
    ///
    /// Only matters for a provider whose `prompt_cache_ttl` answers, so an embedder's or a test's
    /// mock provider never sends a refresh whatever this says.
    pub cache_warming: crate::cache_warming::CacheWarming,
    /// Optional provider of advisory graph-derived review insight (relevant files + test gaps),
    /// injected by the front-end (it owns the code graph). `None` leaves reviews ungrounded.
    pub review_insight_provider: Option<Arc<dyn crate::guided_review::ReviewInsightProvider>>,
    /// Where pull requests are read from, for a review of one. `None` makes a pull-request review
    /// unavailable, which is right for embedders and tests that have no business on the network.
    pub github: Option<Arc<dyn crate::github::GitHub>>,
    /// Where a "remember this" permission approval is written (the *global* permissions file).
    ///
    /// `None` keeps every approval session-scoped — the default for embedders and tests, which
    /// have no business writing to a user's config.
    pub permissions_file: Option<std::path::PathBuf>,
    /// Optional bound on provider round-trips inside a single turn's tool loop.
    ///
    /// Each iteration is one provider call. `None` (the default) lets a turn run until the model
    /// stops calling tools or the user cancels — a blind counter is a poor runaway guard, and the
    /// real ones (the doom-loop check, cancellation, context termination) react to actual failure
    /// signals. Setting a bound is a cost control: the last iteration runs without tools so the
    /// model wraps up in prose instead of the turn being cut off mid-flight.
    pub max_tool_iterations: Option<usize>,
    /// Action sender for self-continuation (set by spawn).
    pub action_tx: Option<mpsc::Sender<Action>>,
}

/// A reviewer in a fresh context, and where the evidence it is handed comes from.
///
/// Injected because a reviewer is a second engine on its own model, and building engines is the
/// front-end's job; the engine only asks the question and hands the answer back to the agent.
#[derive(Clone)]
pub struct OnDemandReview {
    /// The model the reviewer runs on, as a front-end names it
    pub label: String,
    /// Runs one review to a verdict
    pub reviewer: Arc<dyn crate::phase_review::PhaseReviewer>,
    /// The diff, the plan and the files the review opens with
    pub context: Arc<dyn crate::phase_gate::PhaseReviewContext>,
}

impl std::fmt::Debug for OnDemandReview {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("OnDemandReview").finish_non_exhaustive()
    }
}

/// Default per-turn round-trip cap: none (see [`EngineConfig::max_tool_iterations`]).
pub const DEFAULT_MAX_TOOL_ITERATIONS: Option<usize> = None;

/// The per-turn round-trip cap from the environment, falling back to `configured`.
///
/// `LUXE_MAX_TOOL_ITERATIONS` accepts a positive integer, or `0`/`off`/`none`/`unlimited` to
/// remove a configured bound for one run.
pub fn max_tool_iterations_from_env(configured: Option<usize>) -> Option<usize> {
    let Ok(raw) = std::env::var("LUXE_MAX_TOOL_ITERATIONS") else {
        return configured;
    };
    let raw = raw.trim();
    if matches!(raw.to_ascii_lowercase().as_str(), "0" | "off" | "none" | "unlimited") {
        return None;
    }
    raw.parse::<usize>().ok().filter(|n| *n > 0).or(configured)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `LUXE_MAX_TOOL_ITERATIONS` overrides the configured cap, including lifting it entirely.
    /// Serialized behind one test because it mutates process env.
    #[test]
    fn env_overrides_the_configured_tool_iteration_cap() {
        std::env::remove_var("LUXE_MAX_TOOL_ITERATIONS");
        assert_eq!(max_tool_iterations_from_env(None), None, "unset + unconfigured = no cap");
        assert_eq!(
            max_tool_iterations_from_env(Some(40)),
            Some(40),
            "config wins when env is unset"
        );

        std::env::set_var("LUXE_MAX_TOOL_ITERATIONS", "12");
        assert_eq!(max_tool_iterations_from_env(None), Some(12));
        assert_eq!(max_tool_iterations_from_env(Some(40)), Some(12), "env beats config");

        for lifted in ["0", "off", "none", "unlimited", " UNLIMITED "] {
            std::env::set_var("LUXE_MAX_TOOL_ITERATIONS", lifted);
            assert_eq!(max_tool_iterations_from_env(Some(40)), None, "{lifted} lifts the cap");
        }

        std::env::set_var("LUXE_MAX_TOOL_ITERATIONS", "not-a-number");
        assert_eq!(max_tool_iterations_from_env(Some(40)), Some(40), "garbage falls back");
        std::env::remove_var("LUXE_MAX_TOOL_ITERATIONS");
    }
}
