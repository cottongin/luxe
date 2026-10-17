//! Built-in hooks for cross-cutting behavior.

pub mod completion_challenger;
pub mod context_aware_termination;
pub mod file_changed;
pub mod fresh;
pub mod herdr;
pub mod journal_bounding;
mod plan_reminder;
pub mod review_gate;
pub mod review_integrator;
pub mod runaway_guards;
pub mod self_continuation;

/// The first `max` characters, cut on a line boundary and marked when something was left out.
///
/// Shared because two hooks bound a repository document for the same reason: the plan, the journal
/// and a phase's own section all grow without limit, and a prompt that carries one whole is not a
/// briefing, it is the context window.
pub(crate) fn head(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max).collect();
    // On a line boundary: half a sentence reads as a file that ends there.
    let kept = cut.rsplit_once('\n').map(|(head, _)| head).unwrap_or(&cut);
    format!("{kept}\n\n[… truncated; read the file for the rest]")
}

/// The last `max` characters, marked when something was left out.
///
/// The tail rather than the head, because a journal is append-only: what happened most recently is
/// what the next session needs, and the beginning is the part it has already acted on.
pub(crate) fn tail(text: &str, max: usize) -> String {
    let text = text.trim();
    let total = text.chars().count();
    if total <= max {
        return text.to_string();
    }
    let kept: String = text.chars().skip(total - max).collect();
    format!("[… earlier entries omitted; read the file for the rest]\n\n{kept}")
}

/// The first line of `text`, bounded, for a notice that stands in for a longer message.
///
/// A gate's refusal is a fix list the model has to read in full; a person watching the run needs to
/// know it happened and roughly why, on one line, without the terminal filling up.
pub(crate) fn first_line(text: &str) -> String {
    const MAX: usize = 140;
    let line = text.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or_default();
    match line.chars().count() > MAX {
        true => format!("{}\u{2026}", line.chars().take(MAX).collect::<String>().trim_end()),
        false => line.to_string(),
    }
}

pub use completion_challenger::CompletionChallengerHook;
pub use context_aware_termination::ContextAwareTerminationHook;
pub use file_changed::FileChangedHook;
pub use fresh::FreshHook;
pub use herdr::HerdrHook;
pub use journal_bounding::JournalBoundingHook;
pub use plan_reminder::PlanReminderHook;
pub use review_gate::ReviewGateHook;
pub use review_integrator::ReviewIntegratorHook;
pub use runaway_guards::RunawayGuardsHook;
pub use self_continuation::SelfContinuationHook;
