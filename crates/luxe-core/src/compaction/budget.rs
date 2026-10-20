//! Budget — what a request occupies, and how much of the window it may use.

use super::context::context_entries;
use super::CompactionSettings;
use crate::message::{ContentBlock, Message, ToolOutput};
use crate::provider::ToolSchema;
use crate::session::Entry;
use std::borrow::Borrow;
use tracing::{debug, warn};

/// Estimate the token count for a set of entries.
///
/// ~1 token per 4 characters, over **every** kind of content that reaches the model. That breadth is
/// the whole point: a coding session's context is almost entirely tool results and reasoning, so
/// counting only `Text` blocks is off by two orders of magnitude and reads a full window as empty.
///
/// Still only an estimate. Where the provider has told us the real occupancy, prefer that:
/// [`ContextBudget::observed_tokens`] and [`last_observed_context_tokens`].
pub fn estimate_tokens<E: Borrow<Entry>>(entries: &[E]) -> u64 {
    entries.iter().map(|entry| entry_to_token_estimate(entry.borrow())).sum()
}

/// Estimate the tokens a provider-facing message list occupies.
///
/// The counterpart to [`estimate_tokens`] for the copy that is actually sent — after tool-call
/// pairing and [`super::prune_tool_outputs`] have edited it, and for the synthetic one-message
/// prompts the summarizer and the review guide build, which never existed as session entries.
pub fn estimate_message_tokens(messages: &[Message]) -> u64 {
    messages.iter().map(msg_to_token_estimate).sum()
}

/// Estimate the tokens a request spends before any conversation: the system prompt and the tool
/// schemas.
///
/// Far from negligible — luxe's harness plus two dozen JSON schemas is tens of thousands of
/// tokens, and leaving it out of a prompt figure is how a request that "fits" turns into a 400.
pub fn estimate_prefix_tokens(system: Option<&str>, tools: &[ToolSchema]) -> u64 {
    let system = system.map(|text| text.len() as u64 / 4).unwrap_or(0);
    let tools: u64 = tools
        .iter()
        .map(|tool| {
            (tool.name.len() as u64
                + tool.description.len() as u64
                + tool.parameters.to_string().len() as u64)
                / 4
        })
        .sum();
    system + tools
}

/// Estimate tokens for a single entry.
pub(super) fn entry_to_token_estimate(entry: &Entry) -> u64 {
    match &entry.kind {
        crate::session::EntryKind::Message(msg) => msg_to_token_estimate(msg),
        crate::session::EntryKind::Compaction { summary, .. } => summary.len() as u64 / 4,
        _ => 0,
    }
}

/// Estimate tokens for a single message.
fn msg_to_token_estimate(msg: &Message) -> u64 {
    msg.content.iter().map(block_token_estimate).sum()
}

/// Estimate tokens for one content block.
///
/// A tool call costs its name plus its serialized arguments; reasoning costs its text, and its opaque
/// replay payloads cost real tokens on the wire even though no one can read them. Images are the one
/// thing characters cannot describe, so they take the same flat figure [`tool_result_tokens`] uses.
fn block_token_estimate(block: &ContentBlock) -> u64 {
    match block {
        ContentBlock::Text(text) => text.len() as u64 / 4,
        ContentBlock::Thinking { text, signature, id, encrypted, opaque } => {
            let replayed = [signature, id, encrypted, opaque]
                .into_iter()
                .flatten()
                .map(|value| value.len() as u64)
                .sum::<u64>();
            (text.len() as u64 + replayed) / 4
        }
        ContentBlock::RedactedThinking { data } => data.len() as u64 / 4,
        ContentBlock::ToolCall { name, arguments, .. } => {
            (name.len() as u64 + arguments.to_string().len() as u64) / 4
        }
        ContentBlock::ToolResult { content, .. } => tool_result_tokens(content),
        ContentBlock::Image { .. } => IMAGE_TOKEN_ESTIMATE,
    }
}

/// The most recent context occupancy the provider actually reported, if this transcript carries one.
///
/// Reads the persisted [`crate::session::EntryKind::Usage`] ledger backwards, which is a measurement
/// rather than a guess — the number the model billed for. Two rules make it trustworthy:
///
/// - **Stop at a compaction.** A usage record written before the cut describes the context that
///   compaction just replaced, so carrying it forward would report a window that no longer exists and
///   re-trigger compaction on a session that had just finished one.
/// - **One call only.** A usage entry folds together every call in its flush, and while the turn loop
///   writes one call per entry, a detached pass can batch several; summing their prompt tokens is not
///   an occupancy figure of anything.
///
/// Returns `None` when the transcript has nothing measured to offer, and the caller falls back to
/// [`estimate_tokens`].
pub fn last_observed_context_tokens<E: Borrow<Entry>>(entries: &[E]) -> Option<u64> {
    last_observed_measurement(entries).map(|(_, occupancy)| occupancy)
}

/// [`last_observed_context_tokens`], plus where in the transcript it was measured, so a caller can
/// estimate only what has been appended since.
fn last_observed_measurement<E: Borrow<Entry>>(entries: &[E]) -> Option<(usize, u64)> {
    for (index, entry) in entries.iter().enumerate().rev() {
        match &entry.borrow().kind {
            crate::session::EntryKind::Compaction { .. } => return None,
            crate::session::EntryKind::Usage { source, totals, .. }
                if *source == crate::usage::UsageSource::Agent && totals.calls == 1 =>
            {
                let occupancy = totals.prompt_tokens + totals.completion_tokens;
                if occupancy > 0 {
                    return Some((index, occupancy));
                }
            }
            _ => {}
        }
    }
    None
}

/// What the next request built from this transcript will occupy on the way in, in tokens.
///
/// Prefers the provider's own last measurement, which already counts the system prompt and tool
/// schemas, plus an estimate of the entries appended since. With nothing measured (a fresh session,
/// or one whose last usage predates a compaction) it estimates [`context_entries`] plus the prefix:
/// what a compaction replaced is not sent, so it is not counted.
pub fn prompt_tokens<E: Borrow<Entry>>(
    entries: &[E],
    system: Option<&str>,
    tools: &[ToolSchema],
) -> u64 {
    match last_observed_measurement(entries) {
        // The measurement already includes the prefix that call was sent with.
        Some((index, observed)) => observed + estimate_tokens(&entries[index + 1..]),
        None => context_tokens(entries) + estimate_prefix_tokens(system, tools),
    }
}

/// Estimated tokens for the conversation a request would carry — [`estimate_tokens`] over
/// [`context_entries`].
pub fn context_tokens<E: Borrow<Entry>>(entries: &[E]) -> u64 {
    context_entries(entries).into_iter().map(entry_to_token_estimate).sum()
}

/// The occupancy at which a conversation gets summarized, or `None` for a window too small (or too
/// unknown) to reason about.
///
/// One owner, because two callers need the same answer: the planner, which is about to load the
/// whole active path to work out where to cut, and the turn loop, which asks once per tool
/// iteration and must be able to say "nowhere near it" without paying for that load.
pub fn compaction_threshold(usable_window: u64, settings: &CompactionSettings) -> Option<u64> {
    (usable_window > 0).then_some((usable_window as f64 * settings.compact_at_fraction) as u64)
}

/// The usable input budget: the real context window minus a capped output reserve.
///
/// The reserve is `min(max_tokens, reserve_tokens)`, so we leave room for the model's reply without
/// ever sacrificing the whole window to a large `max_tokens` (a 128K-output model would otherwise
/// reserve 128K). When `max_tokens` is unknown (0), reserve the full cap. Mirrors Claude Code's
/// `min(max_output, 20K)` and OpenCode's `context − maxOutput` fallback.
pub fn usable_context_window(context_window: u64, max_tokens: u32, reserve_tokens: u32) -> u64 {
    let reserve = if max_tokens == 0 { reserve_tokens } else { max_tokens.min(reserve_tokens) };
    context_window.saturating_sub(reserve as u64)
}

/// Held back from the window when capping a request's output.
///
/// The prompt figure is part measurement, part estimate, and a tokenizer that disagrees by a few
/// percent on a 200K prompt is thousands of tokens out. Mirrors Pi's margin.
pub const CONTEXT_SAFETY_TOKENS: u32 = 4_096;

/// The smallest output cap luxe will ask for once the window is full. Useless as an answer, but a
/// request that asks for zero (or a negative) output is a hard rejection rather than a short reply.
const MIN_MAX_TOKENS: u32 = 1;

/// Cap one request's output so the prompt and the reply both fit inside the context window.
///
/// `max_tokens` is a ceiling, not an allocation, and must never claim room the prompt already
/// occupies: llama.cpp and LM Studio silently clamp an oversized cap, while vLLM and OpenAI reject
/// the request outright.
///
/// Returns `None` for a configured `0`, which every adapter but Anthropic reads as "omit the
/// field". An unknown window (`0`) can't be reasoned about, so the cap passes through untouched.
///
/// Deliberately independent of [`usable_context_window`]: that one decides when to *summarize* a
/// conversation, this one decides what a single request may ask for, and tying them together would
/// make either knob move the other.
pub fn clamp_max_tokens_to_context(
    context_window: u32,
    prompt_tokens: u64,
    max_tokens: u32,
) -> Option<u32> {
    if max_tokens == 0 || context_window == 0 {
        return (max_tokens > 0).then_some(max_tokens);
    }
    let available = u64::from(context_window)
        .saturating_sub(prompt_tokens)
        .saturating_sub(u64::from(CONTEXT_SAFETY_TOKENS))
        .min(u64::from(u32::MAX)) as u32;
    if available < MIN_MAX_TOKENS {
        warn!(
            context_window,
            prompt_tokens, "the prompt fills the context window; asking for a minimal reply"
        );
    }
    let capped = max_tokens.min(available.max(MIN_MAX_TOKENS));
    if capped < max_tokens {
        debug!(
            max_tokens,
            capped, prompt_tokens, context_window, "capped output to fit the window"
        );
    }
    Some(capped)
}

/// Output allowance for one summary, before the model's limit and the window cut it down.
///
/// Room for the six sections over a whole window's worth of transcript *and* for the reasoning a
/// model that always thinks spends first, even asked not to: that comes out of the same allowance,
/// and a summary that hits the ceiling is refused whole, so a tight one fails every time on exactly
/// the long sessions that need it. Generous because only what's written is billed, while a refused
/// summary has already paid for reading the whole window.
pub(crate) const SUMMARY_MAX_TOKENS: u32 = 32_768;

/// What one summary request may ask for: [`SUMMARY_MAX_TOKENS`], within `model`'s own output limit
/// (0 for unknown) and what `prompt_tokens` leaves of its window.
pub(crate) fn summary_max_tokens(
    model: &crate::provider::ModelInfo,
    prompt_tokens: u64,
) -> Option<u32> {
    let ceiling = match model.max_tokens {
        0 => SUMMARY_MAX_TOKENS,
        limit => limit.min(SUMMARY_MAX_TOKENS),
    };
    clamp_max_tokens_to_context(model.context_window, prompt_tokens, ceiling)
}

/// Percent of the usable window currently occupied. Returns 0.0 when either input is 0 (e.g. no
/// usage observed yet), so callers read "unknown" as empty rather than full.
pub fn context_fullness_percent(used_tokens: u64, usable: u64) -> f64 {
    if usable == 0 || used_tokens == 0 {
        return 0.0;
    }
    (used_tokens as f64 / usable as f64) * 100.0
}

/// Rough per-image context cost. Images aren't char/4 — providers price them by dimensions
/// (~1K–1.6K tokens for a typical screenshot); a flat estimate is close enough for prune accounting.
///
/// Shared with the `/context` measurement, so a picture is worth the same in both.
pub(crate) const IMAGE_TOKEN_ESTIMATE: u64 = 1_500;

/// Token estimate for a tool result's payload: text via char/4, images at a flat rate.
pub(super) fn tool_result_tokens(content: &[ToolOutput]) -> u64 {
    content
        .iter()
        .map(|o| match o {
            ToolOutput::Text(t) => t.len() as u64 / 4,
            ToolOutput::Image { .. } => IMAGE_TOKEN_ESTIMATE,
        })
        .sum()
}

/// The live model context budget used to decide compaction.
#[derive(Debug, Clone, Copy, Default)]
pub struct ContextBudget {
    /// The model's real context window, in tokens.
    pub context_window: u64,
    /// The model's max output; caps the reserved reply slice.
    pub max_tokens: u32,
    /// Exact occupancy from the provider's last usage event, if known. Preferred over the char/4
    /// estimate — both Claude Code and OpenCode trigger on real usage.
    pub observed_tokens: Option<u64>,
    /// Estimated tokens the last request spent before any conversation: system prompt and tool
    /// schemas.
    ///
    /// Needed for two things the cut cannot be made correctly without — splitting a measured
    /// occupancy into prefix and conversation, and remembering that the prefix occupies the window
    /// the kept tail has to share.
    pub prefix_tokens: u64,
}

/// What the char/4 estimate has to be multiplied by to land on real tokens.
///
/// The estimate is a constant and tokenizers are not: prose runs near four characters per token,
/// source nearer 2.4. That matters wherever the two units meet — `keep_recent_tokens` is a
/// real-token budget, and the walk that honors it counts in estimated ones, so an uncalibrated tail
/// comes out far larger than asked and leaves compaction nothing to free.
///
/// Derived from the one measurement available: what the provider said the last request cost against
/// what this crate would have guessed for it. Blended across the prefix and the conversation,
/// because splitting them would need a second measurement nobody reports, and tool schemas are JSON
/// that tokenizes about as badly as code anyway.
///
/// Clamped, because it multiplies a cut: a wild ratio must not be able to summarize the whole
/// session or none of it. `1.0` — believe the estimate — whenever nothing has been measured yet,
/// and never less: a ratio below it would make the kept tail *larger* than the setting says, which
/// is the failure this exists to fix.
pub fn estimate_calibration(observed: Option<u64>, estimated: u64) -> f64 {
    /// Believe the estimate at worst; above this, no real tokenizer.
    const RANGE: std::ops::RangeInclusive<f64> = 1.0..=4.0;

    match observed {
        Some(observed) if observed > 0 && estimated > 0 => {
            (observed as f64 / estimated as f64).clamp(*RANGE.start(), *RANGE.end())
        }
        _ => 1.0,
    }
}

#[cfg(test)]
mod tests;
