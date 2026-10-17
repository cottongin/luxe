//! Planner — where a compaction cuts, and what it hands the summarizer.

use super::budget::{
    compaction_threshold, context_tokens, entry_to_token_estimate, estimate_calibration,
    usable_context_window, ContextBudget,
};
use super::summary::{contributes_to_summary, files_listed_in, files_touched};
use super::{CompactionError, CompactionPlan, CompactionSettings, CompactionTrigger};
use crate::session::Entry;
use tracing::debug;

/// Find the compaction point in loaded entries, given the active model's real context budget.
///
/// The budget comes from the live model so the trigger tracks the actual window, not a hardcoded
/// default. Returns `Cancelled` when compaction isn't needed — or when the window is unknown (0),
/// since compacting blind is worse than not compacting (OpenCode does the same).
pub fn find_compaction_point(
    entries: &[Entry],
    settings: &CompactionSettings,
    budget: ContextBudget,
    trigger: CompactionTrigger,
) -> Result<CompactionPlan, CompactionError> {
    // Prefer the provider's exact usage when we have it; fall back to the estimate over what a
    // request would actually carry, so a compaction that just landed reads as the smaller context
    // it produced rather than re-triggering on the history it replaced.
    let tokens_before =
        budget.observed_tokens.filter(|t| *t > 0).unwrap_or_else(|| context_tokens(entries));

    let usable =
        usable_context_window(budget.context_window, budget.max_tokens, settings.reserve_tokens);
    if trigger == CompactionTrigger::Threshold {
        let Some(threshold) = compaction_threshold(usable, settings) else {
            return Err(CompactionError::Cancelled);
        };
        if tokens_before < threshold {
            debug!(tokens_before, threshold, "below compaction threshold");
            return Err(CompactionError::Cancelled);
        }
    }
    let conversation = context_tokens(entries);
    let calibration =
        estimate_calibration(budget.observed_tokens, conversation + budget.prefix_tokens);
    let prefix = (budget.prefix_tokens as f64 * calibration) as u64;

    // `keep_recent_tokens` is a fixed count and the window is not: on a small model the default
    // tail is more than the whole window, and the walk below would never find a cut. So the tail
    // and the prefix (which shares that window) may hold at most half of it.
    let keep_recent = match usable {
        0 => u64::from(settings.keep_recent_tokens),
        usable => u64::from(settings.keep_recent_tokens).min((usable / 2).saturating_sub(prefix)),
    };

    // Everything a previous compaction already replaced is out of scope: summarizing it again
    // would re-read the whole session on every compaction and fold the same history in twice.
    // The span starts at the entries that compaction *kept*, which are the ones still in play.
    let (boundary, previous_summary) = summarized_span_start(entries);

    // Walk back from the end until the tail is worth `keep_recent` *real* tokens, which is
    // `keep_recent / calibration` of the estimated ones this walk can actually count.
    let target = (keep_recent as f64 / calibration) as u64;
    let mut accumulated = 0u64;
    let mut first_kept_idx = entries.len();

    for (i, entry) in entries.iter().enumerate().skip(boundary).rev() {
        accumulated += entry_to_token_estimate(entry);
        if accumulated >= target {
            first_kept_idx = i;
            break;
        }
    }

    // If the whole tail fits within keep_recent, there's nothing older to summarize — bail rather
    // than index past the end. This happens when occupancy is dominated by content that isn't in
    // the transcript entries (tool schemas, system prompt), so real usage can trip the threshold
    // while the entries themselves are small.
    if first_kept_idx >= entries.len() {
        debug!(entries = entries.len(), "nothing older than keep_recent to summarize");
        return Err(CompactionError::Cancelled);
    }
    // A cut that lands on a tool result keeps the result and summarizes away the call it answers,
    // which is an unpaired half on the wire and a hard 400 on both formats. Move the boundary
    // forward to the next entry that can legally start a request.
    let Some(first_kept_idx) =
        (first_kept_idx..entries.len()).find(|i| starts_context(&entries[*i]))
    else {
        debug!("no valid cut point in the recent tail");
        return Err(CompactionError::Cancelled);
    };

    let messages_to_summarize = entries[boundary..first_kept_idx].to_vec();
    // Not merely non-empty: non-empty *of conversation*. A span of cost records and model switches
    // reaches the summarizer as a blank document, which is a paid call to summarize nothing
    if !messages_to_summarize.iter().any(contributes_to_summary) {
        debug!(entries = messages_to_summarize.len(), "nothing in the span reaches a summarizer");
        return Err(CompactionError::Cancelled);
    }
    let first_kept = entries[first_kept_idx].id.clone();

    // What the last summary recorded, plus what this segment touched — the union is the running
    // record, and the second half is the only part that can ever add to it.
    let mut file_ops = previous_summary.as_deref().map(files_listed_in).unwrap_or_default();
    file_ops.extend(files_touched(&messages_to_summarize));

    Ok(CompactionPlan {
        messages_to_summarize,
        turn_prefix: vec![],
        first_kept,
        previous_summary,
        file_ops,
        tokens_before,
    })
}

/// Where the next summary's input starts, and the summary it continues from.
///
/// The entry after a previous compaction's `first_kept`, so each pass folds the last summary plus
/// the messages that survived it into one — rather than starting from the beginning of a session
/// whose early messages were replaced several compactions ago. A `first_kept` that isn't on the
/// path falls back to the entry after the compaction itself.
pub(super) fn summarized_span_start(entries: &[Entry]) -> (usize, Option<String>) {
    let Some(cut) = entries
        .iter()
        .rposition(|entry| matches!(entry.kind, crate::session::EntryKind::Compaction { .. }))
    else {
        return (0, None);
    };
    let crate::session::EntryKind::Compaction { summary, first_kept, .. } = &entries[cut].kind
    else {
        return (0, None);
    };
    let start = entries[..cut].iter().position(|entry| entry.id == *first_kept).unwrap_or(cut + 1);
    (start, Some(summary.clone()))
}

/// Whether the kept side of a cut may begin with this entry.
///
/// A tool result may not: it answers a call that would be on the summarized side. Anything that
/// carries no message at all may not either — the cut names an entry, and naming one the request
/// never sees leaves the first *message* free to be a tool result again.
fn starts_context(entry: &Entry) -> bool {
    match &entry.kind {
        crate::session::EntryKind::Message(message) => message.role != crate::message::Role::Tool,
        _ => false,
    }
}

#[cfg(test)]
mod tests;
