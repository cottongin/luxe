//! Tool pairing — every tool call in a request reaches the provider with a result attached.
//!
//! Both provider wire formats treat a tool call and its result as one indivisible unit. Anthropic
//! rejects a `tool_use` with no matching `tool_result`; the OpenAI Responses API answers an
//! orphaned `function_call` with `400 No tool output found for function call <id>`. Neither error is
//! recoverable by retrying, because the offending pair is *history* — it replays on every
//! subsequent request, so one gap ends the session and every resume of it.
//!
//! A gap is easy to open. The turn commits the assistant message the moment the stream closes and
//! runs the tools afterwards, so anything between those two points — a quit, a crash, a cancelled
//! batch, a compaction cut landing mid-pair — leaves calls with nowhere to point. This module
//! closes the gap on the **model-facing copy** at request assembly, next to
//! [`crate::compaction::prune_tool_outputs`]. The durable session log is never rewritten: it is the
//! record of what happened, and what happened is that the call had no result.
//!
//! Repair runs in both directions, since a dropped assistant message strands results just as a
//! dropped tool message strands calls.

use std::collections::HashSet;

use crate::message::{ContentBlock, Message, Role, ToolOutput};

/// What a call's result says when the harness has to write it instead of the tool.
///
/// The three are not interchangeable, because they make different claims about **side effects**. A
/// call that was aborted after it started may have already written a file, pushed a branch, or
/// spent money; telling a model that call "never ran" is how the same `git push` happens twice. Only
/// [`Self::NeverStarted`] is a promise that nothing happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Unfinished {
    /// The call was in flight when the turn was cancelled or its deadline passed.
    Aborted,
    /// The call was still queued behind another when the turn stopped, so it never began.
    NeverStarted,
    /// The call's result is missing from the transcript and there is no telling why.
    Unrecorded,
}

impl Unfinished {
    /// The text the model reads in place of the tool's own output.
    pub(crate) const fn message(self) -> &'static str {
        match self {
            Self::Aborted => {
                "interrupted after it started: the turn was stopped mid-call, so this tool may have \
                 partially completed. Check the current state before retrying it."
            }
            Self::NeverStarted => {
                "not executed: the turn was stopped before this call started, so it had no effect. \
                 Safe to retry."
            }
            Self::Unrecorded => {
                "interrupted: this call's result was never recorded, so its outcome is unknown. \
                 Verify the current state rather than assuming it did or did not take effect."
            }
        }
    }

    /// A result block carrying this explanation for `call_id`.
    pub(crate) fn result_for(self, call_id: impl Into<String>) -> ContentBlock {
        ContentBlock::ToolResult {
            call_id: call_id.into(),
            content: vec![ToolOutput::Text(self.message().to_string())],
            is_error: true,
        }
    }
}

/// What one [`repair`] pass had to change.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Repair {
    /// Calls that were given a synthesized result.
    pub(crate) calls_answered: usize,
    /// Results dropped because the call they answered is no longer in the transcript.
    pub(crate) results_dropped: usize,
}

impl Repair {
    pub(crate) fn is_clean(&self) -> bool {
        self.calls_answered == 0 && self.results_dropped == 0
    }
}

/// Pair up every tool call and result in `messages`, in place.
///
/// Missing results are synthesized as [`Unfinished::Unrecorded`] — by the time a transcript is being
/// replayed, why the result is absent is no longer knowable, and the engine labels the cases it
/// *does* know about while they happen. Each synthesized block is appended to the `Tool` message
/// that already follows the call's assistant message when there is one (a partially dispatched
/// batch), so a repaired turn has the same shape as an ordinary one.
///
/// A valid transcript is left byte-identical, which matters more than it looks: the request prefix
/// has to stay stable for provider prompt caching to hit.
pub(crate) fn repair(messages: &mut Vec<Message>) -> Repair {
    let calls: HashSet<String> = messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            ContentBlock::ToolCall { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect();
    let answered: HashSet<String> = messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            ContentBlock::ToolResult { call_id, .. } => Some(call_id.clone()),
            _ => None,
        })
        .collect();

    let mut repair = Repair::default();
    if calls.is_subset(&answered) && answered.is_subset(&calls) {
        return repair;
    }

    // Drop results whose call is gone, then remove any message that emptied out — an empty message
    // is its own provider error, so replacing one gap with another would be no fix at all.
    for message in messages.iter_mut() {
        message.content.retain(|block| match block {
            ContentBlock::ToolResult { call_id, .. } => {
                let keep = calls.contains(call_id);
                repair.results_dropped += usize::from(!keep);
                keep
            }
            _ => true,
        });
    }
    messages.retain(|message| !message.content.is_empty());

    // Answer the calls nothing came back for. Walk forwards and rebuild, so an inserted `Tool`
    // message can't shift an index out from under the walk.
    let mut repaired: Vec<Message> = Vec::with_capacity(messages.len() + 1);
    let mut pending = std::mem::take(messages).into_iter().peekable();
    while let Some(message) = pending.next() {
        let missing: Vec<ContentBlock> = message
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::ToolCall { id, .. } if !answered.contains(id) => {
                    Some(Unfinished::Unrecorded.result_for(id))
                }
                _ => None,
            })
            .collect();
        repaired.push(message);
        if missing.is_empty() {
            continue;
        }
        repair.calls_answered += missing.len();
        match pending.peek() {
            // The batch was partially dispatched: extend the results it did produce.
            Some(next) if next.role == Role::Tool => {
                let mut tool_message = pending.next().expect("peeked");
                tool_message.content.extend(missing);
                repaired.push(tool_message);
            }
            _ => repaired.push(Message { role: Role::Tool, content: missing }),
        }
    }
    *messages = repaired;
    repair
}

#[cfg(test)]
mod tests;
