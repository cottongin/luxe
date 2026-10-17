//! Prune — eliding stale tool output from the outgoing copy, only under real window pressure.

use super::budget::{context_tokens, estimate_prefix_tokens, tool_result_tokens};
use super::CompactionSettings;
use crate::message::{ContentBlock, Message, ToolOutput};
use crate::provider::ToolSchema;
use crate::session::Entry;
use tracing::debug;

/// The fullest this session's window has been since its last compaction.
///
/// A high-water mark rather than current occupancy, because the prune decision would otherwise read
/// its own output: eliding drops occupancy back under the gate, the next request keeps everything,
/// occupancy climbs again — and each flip rewrites the transcript and re-bills the conversation
/// behind it. A mark only rises within an epoch, so the gate opens once and stays open.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ContextPressure {
    /// The largest prompt the provider has measured this epoch, or the estimate until it has
    /// measured one.
    pub peak_tokens: u64,
    /// The usable input budget: the window minus the output reserve.
    pub usable_window: u64,
}

impl ContextPressure {
    /// Measures how full this transcript has had the window, against the budget it has to fit in.
    ///
    /// Prefers the provider's own figures: the char/4 estimate reads a prompt of code as about two
    /// thirds of its size, and a threshold hung off it fires late.
    pub fn measure(
        entries: &[Entry],
        system: Option<&str>,
        tools: &[ToolSchema],
        usable_window: u64,
    ) -> Self {
        let peak_tokens = observed_context_peak(entries)
            .unwrap_or_else(|| context_tokens(entries) + estimate_prefix_tokens(system, tools));
        Self { peak_tokens, usable_window }
    }

    /// The share of the budget the peak took, or `0.0` when the window is unknown — an unknown
    /// window is not evidence of pressure.
    pub fn fraction(self) -> f64 {
        if self.usable_window == 0 {
            return 0.0;
        }
        self.peak_tokens as f64 / self.usable_window as f64
    }
}

/// The largest input the provider has reported since the newest compaction, if any.
///
/// Same reading rules as [`super::last_observed_context_tokens`] — stop at a compaction, because a
/// measurement from before the cut describes a context that no longer exists, and count only
/// single-call records, because a batched flush's summed prompt tokens are not an occupancy figure
/// of anything.
fn observed_context_peak(entries: &[Entry]) -> Option<u64> {
    let mut peak = None;
    for entry in entries.iter().rev() {
        match &entry.kind {
            crate::session::EntryKind::Compaction { .. } => break,
            crate::session::EntryKind::Usage { source, totals, .. }
                if *source == crate::usage::UsageSource::Agent && totals.calls == 1 =>
            {
                peak = peak.max(Some(totals.prompt_tokens));
            }
            _ => {}
        }
    }
    peak.filter(|tokens| *tokens > 0)
}

/// Non-destructively shrink stale tool-result bodies in the model-facing message list.
///
/// Every tool result older than the newest `prune_protect_tokens` of tool output is replaced with a
/// compact placeholder. Only the provider-facing copy is edited; the durable session keeps the full
/// text. `call_id`/`is_error` are preserved so the tool_use↔tool_result pairing stays valid, and
/// images go too (a stale screenshot rarely earns its place once it is past the protected tail).
///
/// # Why it is so reluctant
///
/// Eliding rewrites the *middle* of the transcript, and every provider that caches does it by exact
/// prefix match — so one elision re-bills the entire conversation after it as a cache write, at
/// 1.25x input rather than 0.1x. At Anthropic's rates that is break-even only after roughly
/// `12.5 × conversation ÷ elided` further calls, typically hundreds. Eliding is
/// therefore never a saving in itself: it buys *runway*, and it is worth paying for only when the
/// window is genuinely running out.
///
/// Two rules keep it honest, and between them the outgoing transcript is append-only except at a
/// handful of deliberate steps:
///
/// - **A pressure gate.** Nothing is touched below `prune_at_fraction` of the usable window, where
///   eliding would spend most of a session's cost on runway it does not need.
/// - **A quantized boundary.** The cut is snapped to a multiple of `prune_min_tokens` and measured
///   from the *oldest* end, so it holds still between steps. Measured from the newest end it would
///   slide by one result per turn, rewriting the transcript on every single call.
pub fn prune_tool_outputs(
    messages: &mut [Message],
    settings: &CompactionSettings,
    pressure: ContextPressure,
) {
    if !settings.enabled || pressure.fraction() < settings.prune_at_fraction {
        return;
    }
    let boundary = prune_boundary(messages, settings);
    if boundary == 0 {
        return;
    }
    debug!(
        boundary,
        peak = pressure.peak_tokens,
        window = pressure.usable_window,
        "eliding stale tool output"
    );
    elide_tool_output_before(messages, boundary);
}

/// Tokens of tool output, counted from the oldest, that are stale enough to elide.
///
/// `0` means "leave the transcript alone": either the protected tail covers everything, or what is
/// left over has not yet reached a full `prune_min_tokens` step.
fn prune_boundary(messages: &[Message], settings: &CompactionSettings) -> u64 {
    let step = (settings.prune_min_tokens as u64).max(1);
    let total: u64 = tool_output_tokens(messages).map(|(_, _, tokens)| tokens).sum();
    total.saturating_sub(settings.prune_protect_tokens as u64) / step * step
}

/// Replace every tool result that ends at or before `boundary` tokens of tool output with a
/// placeholder naming what it cost.
fn elide_tool_output_before(messages: &mut [Message], boundary: u64) {
    let stale: Vec<(usize, usize, u64)> = tool_output_tokens(messages)
        .scan(0u64, |seen, (mi, bi, tokens)| {
            *seen += tokens;
            Some((mi, bi, *seen, tokens))
        })
        .take_while(|(_, _, seen, _)| *seen <= boundary)
        .map(|(mi, bi, _, tokens)| (mi, bi, tokens))
        .collect();
    for (mi, bi, tokens) in stale {
        if let ContentBlock::ToolResult { content, .. } = &mut messages[mi].content[bi] {
            *content = vec![ToolOutput::Text(format!(
                "[{tokens} tokens of earlier tool output elided to save context]"
            ))];
        }
    }
}

/// Every non-empty tool result in the list as `(message, block, tokens)`, oldest first.
fn tool_output_tokens(messages: &[Message]) -> impl Iterator<Item = (usize, usize, u64)> + '_ {
    messages.iter().enumerate().flat_map(|(mi, msg)| {
        msg.content.iter().enumerate().filter_map(move |(bi, block)| match block {
            ContentBlock::ToolResult { content, .. } => match tool_result_tokens(content) {
                0 => None,
                tokens => Some((mi, bi, tokens)),
            },
            _ => None,
        })
    })
}

#[cfg(test)]
mod tests;
