//! Reply — one provider call's output gathered into the blocks the turn commits.

use super::*;

/// Everything one provider call produced, before it becomes a committed message.
pub(crate) struct StreamedReply {
    /// The answer text, concatenated across its deltas.
    pub text: String,
    /// Reasoning keyed by the provider's content-block index. A block's signature and id arrive
    /// as separate events from its text, so they are gathered per index rather than in sequence.
    pub thinking: HashMap<usize, ThinkingAccum>,
    /// First-seen order of every block. The committed message — and every later replay of it —
    /// has to match the exact order the model produced, which both providers require.
    pub order: Vec<BlockRef>,
    /// Tool calls in the order the model opened them, arguments accumulated from their deltas.
    pub tool_calls: Vec<ToolCallInfo>,
    /// Why the provider stopped, when it said.
    pub stop_reason: Option<crate::provider::StopReason>,
}

impl StreamedReply {
    /// Drops every call but the ones in `ran`, for a reply the provider cut short after some of
    /// its calls had already run: those have to be committed with their results, and the rest
    /// never happened.
    pub(in crate::engine::turn) fn keep_only(&mut self, ran: &[ContentBlock]) {
        let ran: Vec<&str> = ran
            .iter()
            .filter_map(|block| match block {
                ContentBlock::ToolResult { call_id, .. } => Some(call_id.as_str()),
                _ => None,
            })
            .collect();
        self.tool_calls.retain(|call| ran.contains(&call.id.as_str()));
    }

    /// Whether a tool call's arguments were cut off mid-JSON.
    ///
    /// Usually this model's `max_tokens` set too low. The half-streamed arguments would otherwise
    /// parse to `{}` and run as a confusing "missing argument" error, then loop — so the turn
    /// stops on a clear signal instead of dispatching garbage.
    ///
    /// Arguments that do not parse are only *cut off* when the reply stopped early. A reply that
    /// finished normally and still carries bad JSON is the model's own slip — Anthropic streams
    /// arguments unvalidated once asked to stream them as written — and that call is answered with
    /// [`crate::engine::malformed_arguments`] so the model can send it again.
    pub(in crate::engine::turn) fn has_truncated_tool_args(&self) -> bool {
        let finished = matches!(
            self.stop_reason,
            Some(crate::provider::StopReason::ToolCall | crate::provider::StopReason::EndTurn)
        );
        !finished
            && self
                .tool_calls
                .iter()
                .any(|call| crate::engine::malformed_arguments(&call.arguments).is_some())
    }

    /// The content blocks to commit, in the model's exact output order.
    ///
    /// Preserving reasoning↔tool-call adjacency is not cosmetic: both Anthropic and the OpenAI
    /// Responses API require it on replay. Each reasoning block stays separate and verbatim.
    ///
    /// Empty when the call produced nothing, and the caller commits nothing in that case. It used
    /// to stand an empty text block up in place of the missing message, on the reasoning that a
    /// message with no content is not a message either wire format accepts — but neither is a
    /// message holding an empty text block, and unlike a skipped commit that one is *history*:
    /// Anthropic answers it with `400 text content blocks must be non-empty` on every later
    /// request, so the session and every resume of it is over.
    pub(in crate::engine::turn) fn into_blocks(mut self) -> Vec<ContentBlock> {
        let mut blocks = Vec::new();
        let mut text = Some(self.text);
        for block_ref in &self.order {
            match block_ref {
                BlockRef::Thinking(index) => {
                    let Some(accumulated) = self.thinking.remove(index) else { continue };
                    if let Some(data) = accumulated.redacted {
                        blocks.push(ContentBlock::RedactedThinking { data });
                    } else if !accumulated.text.is_empty()
                        || accumulated.signature.is_some()
                        || accumulated.id.is_some()
                        || accumulated.encrypted.is_some()
                        || accumulated.opaque.is_some()
                    {
                        blocks.push(ContentBlock::Thinking {
                            text: accumulated.text,
                            signature: accumulated.signature,
                            id: accumulated.id,
                            encrypted: accumulated.encrypted,
                            opaque: accumulated.opaque,
                        });
                    }
                }
                // Whitespace-only counts as empty: Anthropic rejects `" "` for the same reason
                // it rejects `""`, and an answer that is one space is not an answer.
                BlockRef::Text => {
                    if let Some(text) = text.take().filter(|text| !text.trim().is_empty()) {
                        blocks.push(ContentBlock::Text(text));
                    }
                }
                BlockRef::Tool(index) => {
                    if let Some(call) = self.tool_calls.iter().find(|call| call.index == *index) {
                        blocks.push(ContentBlock::ToolCall {
                            id: call.id.clone(),
                            name: call.name.clone(),
                            arguments: parse_tool_arguments(&call.arguments),
                        });
                    }
                }
            }
        }
        blocks
    }
}

impl StreamedReply {
    /// The accumulator for reasoning block `index`, recording its position on first sight.
    pub(super) fn note_thinking_block(&mut self, index: usize) -> &mut ThinkingAccum {
        if !self.thinking.contains_key(&index) {
            self.order.push(BlockRef::Thinking(index));
        }
        self.thinking.entry(index).or_default()
    }
}

/// Accumulates one streamed reasoning block. Text, signature, and OpenAI item id/encrypted content
/// arrive as separate stream events (and redacted blocks carry opaque `data` instead of text), so
/// they're gathered per content-block index before being committed as a single [`ContentBlock`].
#[derive(Default)]
pub(crate) struct ThinkingAccum {
    pub text: String,
    pub signature: Option<String>,
    pub id: Option<String>,
    pub encrypted: Option<String>,
    pub opaque: Option<String>,
    pub redacted: Option<String>,
}

/// A reference into the per-turn accumulators, recorded in first-seen order so the committed
/// message preserves the model's exact block ordering. `Thinking`/`Tool` carry the provider's
/// content-block index; `Text` is the single accumulated answer block.
pub(crate) enum BlockRef {
    Thinking(usize),
    Text,
    Tool(usize),
}
