//! Compaction — keep long conversations inside the model's context window.
//!
//! Implements threshold-based auto-compaction and manual `/compact` command.
//! Walks back from the newest message, accumulating token estimates until
//! `keep_recent_tokens` is reached, then summarizes everything older than
//! that cut-point into a structured summary.

use crate::provider::ModelInfo;
use crate::session::{Entry, EntryId, SessionError};
use std::collections::HashSet;

mod budget;
mod context;
mod planner;
mod prune;
mod summary;

pub use budget::{
    clamp_max_tokens_to_context, compaction_threshold, context_fullness_percent, context_tokens,
    estimate_calibration, estimate_message_tokens, estimate_prefix_tokens, estimate_tokens,
    last_observed_context_tokens, prompt_tokens, usable_context_window, ContextBudget,
    CONTEXT_SAFETY_TOKENS,
};
pub(crate) use budget::{summary_max_tokens, IMAGE_TOKEN_ESTIMATE};
pub use context::{context_entries, context_messages};
pub use planner::find_compaction_point;
pub use prune::{prune_tool_outputs, ContextPressure};
pub(crate) use summary::SUMMARIZER_PROMPT;
pub use summary::{format_structured_summary, serialize_for_summary, summary_carries_content};

/// Settings controlling compaction behavior.
#[derive(Debug, Clone)]
pub struct CompactionSettings {
    /// Whether compaction is enabled.
    pub enabled: bool,
    /// Cap on the output slice reserved below the context window. The real reserve is
    /// `min(max_tokens, reserve_tokens)` — a reply/summary rarely needs the model's full max
    /// output, so we never surrender a whole 128K-output budget (Claude Code and OpenCode cap the
    /// same way, ~20K).
    pub reserve_tokens: u32,
    /// Tokens of recent context to keep verbatim.
    pub keep_recent_tokens: u32,
    /// Fraction of the usable window at which auto-compaction fires (0.0–1.0). A fraction, not a
    /// fixed buffer, so the trigger scales sanely across 128K…1M windows.
    pub compact_at_fraction: f64,
    /// Tool output within the most-recent this-many tokens stays verbatim; older tool results are
    /// pruned to a placeholder. Mirrors OpenCode's 40K protect window.
    pub prune_protect_tokens: u32,
    /// The quantum the elision boundary moves in, and the smallest gain worth moving it for.
    /// Eliding rewrites the middle of the transcript, so it happens in deliberate steps rather
    /// than continuously — see [`prune_tool_outputs`].
    pub prune_min_tokens: u32,
    /// Fraction of the usable window at or above which stale tool output is elided at all.
    /// Below it the window is not scarce and eliding costs far more than it saves.
    pub prune_at_fraction: f64,
    /// The model that writes summaries, with the window and price the front-end resolved for it;
    /// `None` uses the active model.
    pub summarizer: Option<ModelInfo>,
}

impl Default for CompactionSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            reserve_tokens: 20_000,
            keep_recent_tokens: 20_000,
            compact_at_fraction: 0.9,
            prune_protect_tokens: 40_000,
            prune_min_tokens: 20_000,
            prune_at_fraction: 0.8,
            summarizer: None,
        }
    }
}

/// A plan for what to compact.
#[derive(Debug)]
pub struct CompactionPlan {
    /// Messages to summarize (everything before the cut-point).
    pub messages_to_summarize: Vec<Entry>,
    /// Non-empty only on split-turn: a turn prefix that exceeds keep_recent.
    pub turn_prefix: Vec<Entry>,
    /// The first entry to keep verbatim.
    pub first_kept: EntryId,
    /// Previous summary text, if any (for cumulative file tracking).
    pub previous_summary: Option<String>,
    /// Files this session has read or changed, carried across compactions.
    pub file_ops: HashSet<String>,
    /// Estimated tokens before compaction.
    pub tokens_before: u64,
}

/// Errors that can occur during compaction.
#[derive(Debug, thiserror::Error)]
pub enum CompactionError {
    #[error("session error: {0}")]
    Session(SessionError),
    #[error("provider error: {0}")]
    Provider(String),
    #[error("compaction cancelled")]
    Cancelled,
}

crate::error_cause::from_cause!(CompactionError {
    SessionError => Session,
});

/// After this many consecutive summarization failures, auto-compaction gives up for the rest of
/// the session instead of retrying a failing provider call every turn (Claude Code trips the same
/// kind of breaker after 3). A success resets the count.
pub const MAX_COMPACTION_FAILURES: u32 = 3;

/// Whether the auto-compaction circuit breaker has tripped for `consecutive_failures`.
pub fn compaction_circuit_open(consecutive_failures: u32) -> bool {
    consecutive_failures >= MAX_COMPACTION_FAILURES
}

/// The most of a conversation a compaction may leave behind and still count as having worked.
///
/// A summary that frees a twentieth of the transcript has spent a provider call to buy nothing, and
/// the occupancy that triggered it is still over the line, so the next check — every tool
/// iteration — fires too. It happens on small windows, where the protected tail (capped at half the
/// usable window) is most of what there was to cut.
const COMPACTION_MUST_FREE: f64 = 0.2;

/// Whether a compaction freed enough of the transcript to have been worth making.
///
/// Both figures are the same char/4 estimate over the same shape, so the ratio is meaningful even
/// though neither number is exact.
pub fn compaction_was_effective(before: u64, after: u64) -> bool {
    if before == 0 {
        return true;
    }
    (before.saturating_sub(after) as f64 / before as f64) >= COMPACTION_MUST_FREE
}

/// Pick the model that writes the compaction summary: the configured `summarizer` when set, else
/// the caller's active model.
pub fn summarizer_model(summarizer: Option<&ModelInfo>, active: &ModelInfo) -> ModelInfo {
    summarizer.unwrap_or(active).clone()
}

/// Why compaction is being considered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionTrigger {
    /// Occupancy crossed the configured fraction of the usable window.
    Threshold,
    /// The user asked for it, so the threshold is not the question — only whether there is
    /// anything older than the recent tail worth summarizing.
    Manual,
}

#[cfg(test)]
mod tests;
