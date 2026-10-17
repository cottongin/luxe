//! Anthropic SSE decode — parse the event stream into `ProviderEvent`s.

use super::*;

/// Parse Anthropic SSE events from a whole response body into a vector of ProviderEvents.
///
/// The live path decodes record-by-record through [`AnthropicDecoder`]; this drives that same
/// decoder over an already-complete body, which is what the fixture tests exercise.
#[cfg(test)]
pub(crate) fn parse_anthropic_sse_text(text: &str) -> Vec<ProviderEvent> {
    use crate::sse::SseDecoder;

    let mut decoder = AnthropicDecoder::default();
    let mut events = Vec::new();
    for record in text.split("\n\n") {
        decoder.record(record, &mut events);
    }
    events
}

/// Record-at-a-time Anthropic decoder.
///
/// Anthropic splits usage across the stream: `message_start` carries input + cache tokens,
/// `message_delta` carries the final output count. Both accumulate here and are emitted as one
/// normalized `Usage` just before the terminal `Done` (the turn loop stops on `Done`).
#[derive(Default)]
pub(crate) struct AnthropicDecoder {
    usage: luxe_core::provider::Usage,
    have_usage: bool,
}

impl crate::sse::SseDecoder for AnthropicDecoder {
    fn record(&mut self, record: &str, out: &mut Vec<ProviderEvent>) {
        let Some(event) = parse_sse_event(record) else { return };
        match event {
            AnthropicSseEvent::MessageStart(start_usage) => {
                if let Some(u) = start_usage {
                    let read = u.cache_read_input_tokens.unwrap_or(0);
                    let write = u.cache_creation_input_tokens.unwrap_or(0);
                    // Normalize `prompt_tokens` to all input (Anthropic reports uncached only).
                    self.usage.prompt_tokens = u.input_tokens.unwrap_or(0) + read + write;
                    self.usage.cache_read_tokens = read;
                    self.usage.cache_write_tokens = write;
                    self.usage.cache_write_1h_tokens = u.hour_long_writes().min(write);
                    self.usage.completion_tokens = u.output_tokens.unwrap_or(0);
                    self.usage.reasoning_tokens = u.thinking_tokens();
                    self.have_usage = true;
                }
            }
            AnthropicSseEvent::MessageDelta(delta) => {
                if let Some(u) = &delta.usage {
                    if let Some(output) = u.output_tokens {
                        self.usage.completion_tokens = output;
                    }
                    // Thinking arrives with the final output count; a zero here would be the
                    // start-event value, so only a reported figure overwrites.
                    let thinking = u.thinking_tokens();
                    if thinking > 0 {
                        self.usage.reasoning_tokens = thinking;
                    }
                    self.have_usage = true;
                }
                if self.have_usage {
                    self.usage.total_tokens =
                        self.usage.prompt_tokens + self.usage.completion_tokens;
                    out.push(ProviderEvent::Usage(self.usage.clone()));
                    self.have_usage = false;
                }
                out.extend(map_anthropic_event(AnthropicSseEvent::MessageDelta(delta)));
            }
            other => out.extend(map_anthropic_event(other)),
        }
    }
}

/// Parse a single SSE event string.
pub(crate) fn parse_sse_event(event_str: &str) -> Option<AnthropicSseEvent> {
    let lines: Vec<&str> = event_str.lines().collect();
    if lines.is_empty() {
        return None;
    }

    // Extract the event type.
    let event_type = lines.iter().find(|l| l.starts_with("event: "))?.strip_prefix("event: ")?;

    // Extract the data.
    let data_lines: Vec<&str> = lines.iter().filter(|l| l.starts_with("data: ")).copied().collect();
    if data_lines.is_empty() {
        return None;
    }
    let data = data_lines[0].strip_prefix("data: ")?;

    // Parse based on event type.
    match event_type {
        "content_block_start" => {
            let block: AnthropicContentBlockStart = serde_json::from_str(data).ok()?;
            Some(AnthropicSseEvent::ContentBlockStart(block))
        }
        "content_block_delta" => {
            let delta: AnthropicContentBlockDelta = serde_json::from_str(data).ok()?;
            Some(AnthropicSseEvent::ContentBlockDelta(delta))
        }
        "content_block_stop" => {
            let stop: AnthropicContentBlockStop = serde_json::from_str(data).ok()?;
            Some(AnthropicSseEvent::ContentBlockStop(stop))
        }
        "message_delta" => {
            let delta: AnthropicMessageDelta = serde_json::from_str(data).ok()?;
            Some(AnthropicSseEvent::MessageDelta(delta))
        }
        "message_start" => {
            let start: AnthropicMessageStart = serde_json::from_str(data).ok()?;
            Some(AnthropicSseEvent::MessageStart(start.message.and_then(|m| m.usage)))
        }
        "message_stop" => Some(AnthropicSseEvent::MessageStop),
        "error" => {
            let error: AnthropicError = serde_json::from_str(data).ok()?;
            Some(AnthropicSseEvent::Error(error))
        }
        _ => None,
    }
}

/// Map an Anthropic SSE event to a core ProviderEvent, or `None` for one that carries nothing the
/// engine reads.
pub(crate) fn map_anthropic_event(event: AnthropicSseEvent) -> Option<ProviderEvent> {
    Some(match event {
        // Its usage is read by `AnthropicDecoder::record`, which never hands it on
        AnthropicSseEvent::MessageStart(_) => return None,
        AnthropicSseEvent::ContentBlockStart(block) => {
            // Use Anthropic's own content-block index so a tool call's later `input_json_delta`
            // chunks match the `ToolCallStart` they belong to (a per-event counter does NOT match).
            let index = block.index;
            let inner = block.content_block.unwrap_or(AnthropicBlockInner {
                inner_type: String::new(),
                id: None,
                name: None,
                text: None,
                thinking: None,
                data: None,
            });
            match inner.inner_type.as_str() {
                "text" => ProviderEvent::TextDelta { index, delta: inner.text.unwrap_or_default() },
                "thinking" => ProviderEvent::ThinkingDelta {
                    index,
                    delta: inner.thinking.unwrap_or_default(),
                },
                "redacted_thinking" => {
                    ProviderEvent::RedactedThinking { index, data: inner.data.unwrap_or_default() }
                }
                "tool_use" => ProviderEvent::ToolCallStart {
                    index,
                    id: inner.id.unwrap_or_default(),
                    name: inner.name.unwrap_or_default(),
                },
                _ => return None,
            }
        }
        AnthropicSseEvent::ContentBlockDelta(delta) => {
            let index = delta.index;
            let inner = delta.delta.unwrap_or(AnthropicDeltaInner {
                inner_type: String::new(),
                text: None,
                thinking: None,
                signature: None,
                partial_json: None,
            });
            match inner.inner_type.as_str() {
                "text_delta" => {
                    ProviderEvent::TextDelta { index, delta: inner.text.unwrap_or_default() }
                }
                "thinking_delta" => ProviderEvent::ThinkingDelta {
                    index,
                    delta: inner.thinking.unwrap_or_default(),
                },
                "signature_delta" => ProviderEvent::ThinkingSignature {
                    index,
                    signature: inner.signature.unwrap_or_default(),
                },
                "input_json_delta" => ProviderEvent::ToolCallDelta {
                    index,
                    json_delta: inner.partial_json.unwrap_or_default(),
                },
                _ => return None,
            }
        }
        AnthropicSseEvent::ContentBlockStop(stop) => {
            ProviderEvent::ToolCallEnd { index: stop.index }
        }
        AnthropicSseEvent::MessageDelta(delta) => {
            let stop_reason = delta
                .stop_reason
                .or_else(|| delta.delta.as_ref().and_then(|d| d.stop_reason.clone()));
            let reason = match stop_reason.as_deref() {
                Some("end_turn") | None => StopReason::EndTurn,
                Some("max_tokens") => StopReason::Length,
                Some("tool_use") => StopReason::ToolCall,
                // The model declined, which is not the same as finishing
                Some("refusal") => StopReason::ContentFilter,
                Some(other) => StopReason::Other(other.to_string()),
            };
            ProviderEvent::Done { reason }
        }
        AnthropicSseEvent::MessageStop => ProviderEvent::Done { reason: StopReason::EndTurn },
        AnthropicSseEvent::Error(error) => {
            let inner = error.error.as_ref();
            let kind = inner.and_then(|inner| inner.r#type.as_deref()).map_or_else(
                || luxe_core::provider::ProviderErrorKind::Other("anthropic error".to_string()),
                crate::http::kind_of_error_type,
            );
            let message = error.message.or_else(|| inner.and_then(|e| e.message.clone()));
            ProviderEvent::Error { message: message.unwrap_or_default(), kind }
        }
    })
}

/// Anthropic SSE event types.
#[derive(Debug)]
pub(crate) enum AnthropicSseEvent {
    /// `message_start` carries initial usage (input + cache tokens).
    MessageStart(Option<AnthropicUsage>),
    ContentBlockStart(AnthropicContentBlockStart),
    ContentBlockDelta(AnthropicContentBlockDelta),
    ContentBlockStop(AnthropicContentBlockStop),
    MessageDelta(AnthropicMessageDelta),
    MessageStop,
    Error(AnthropicError),
}

/// Anthropic `message_start` wrapper (only its usage block matters here).
#[derive(Debug, Deserialize)]
pub(crate) struct AnthropicMessageStart {
    #[serde(default)]
    pub(crate) message: Option<AnthropicMessageStartInner>,
}

/// Inner `message` object of a `message_start` event.
#[derive(Debug, Deserialize)]
pub(crate) struct AnthropicMessageStartInner {
    #[serde(default)]
    pub(crate) usage: Option<AnthropicUsage>,
}

/// Anthropic usage block (on `message_start` and `message_delta`). `input_tokens` is the
/// UNCACHED input; cache read/creation are reported separately and added in.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct AnthropicUsage {
    #[serde(default)]
    pub(crate) input_tokens: Option<u32>,
    #[serde(default)]
    pub(crate) output_tokens: Option<u32>,
    #[serde(default)]
    pub(crate) cache_creation_input_tokens: Option<u32>,
    #[serde(default)]
    pub(crate) cache_read_input_tokens: Option<u32>,
    /// Breakdown of `output_tokens`; carries the thinking subset.
    #[serde(default)]
    pub(crate) output_tokens_details: Option<AnthropicOutputDetails>,
    /// Breakdown of `cache_creation_input_tokens` by how long the entries are kept.
    #[serde(default)]
    pub(crate) cache_creation: Option<AnthropicCacheCreation>,
}

/// Anthropic's `cache_creation` — the cache writes split by lifetime, which are priced apart.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct AnthropicCacheCreation {
    #[serde(default)]
    pub(crate) ephemeral_1h_input_tokens: Option<u32>,
}

impl AnthropicUsage {
    /// Cache writes kept for an hour, or 0 when the API didn't break them out.
    pub(crate) fn hour_long_writes(&self) -> u32 {
        self.cache_creation.as_ref().and_then(|split| split.ephemeral_1h_input_tokens).unwrap_or(0)
    }

    /// Thinking tokens inside `output_tokens`, or 0 when the API didn't break them out.
    pub(crate) fn thinking_tokens(&self) -> u32 {
        self.output_tokens_details.as_ref().and_then(|details| details.thinking_tokens).unwrap_or(0)
    }
}

/// Anthropic's `output_tokens_details` — the reasoning slice of the output count.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct AnthropicOutputDetails {
    #[serde(default)]
    pub(crate) thinking_tokens: Option<u32>,
}

/// Anthropic content block start event.
#[derive(Debug, Deserialize)]
pub(crate) struct AnthropicContentBlockStart {
    /// The wire discriminator. Nothing branches on it — the envelope already routed by tag — but
    /// the parser tests assert it, which is how a shape change gets caught rather than ignored.
    #[serde(rename = "type")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) block_type: String,
    /// Anthropic's content-block position; ties a tool call's deltas back to its start.
    #[serde(default)]
    pub(crate) index: usize,
    #[serde(default)]
    pub(crate) content_block: Option<AnthropicBlockInner>,
}

/// Anthropic `content_block_stop` — carries the block index that finished.
#[derive(Debug, Deserialize)]
pub(crate) struct AnthropicContentBlockStop {
    #[serde(default)]
    pub(crate) index: usize,
}

/// Inner content block structure.
#[derive(Debug, Deserialize)]
pub(crate) struct AnthropicBlockInner {
    #[serde(rename = "type")]
    pub(crate) inner_type: String,
    #[serde(default)]
    pub(crate) id: Option<String>,
    #[serde(default)]
    pub(crate) name: Option<String>,
    #[serde(default)]
    pub(crate) text: Option<String>,
    #[serde(default)]
    pub(crate) thinking: Option<String>,
    /// Opaque encrypted reasoning for a `redacted_thinking` block.
    #[serde(default)]
    pub(crate) data: Option<String>,
}

/// Anthropic content block delta event.
#[derive(Debug, Deserialize)]
pub(crate) struct AnthropicContentBlockDelta {
    /// The wire discriminator; see [`AnthropicContentBlockStart::block_type`].
    #[serde(rename = "type")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) delta_type: String,
    /// Anthropic's content-block position (matches the `ToolCallStart` index).
    #[serde(default)]
    pub(crate) index: usize,
    #[serde(default)]
    pub(crate) delta: Option<AnthropicDeltaInner>,
}

/// Inner delta structure for content block deltas.
#[derive(Debug, Deserialize)]
pub(crate) struct AnthropicDeltaInner {
    #[serde(rename = "type")]
    pub(crate) inner_type: String,
    #[serde(default)]
    pub(crate) text: Option<String>,
    #[serde(default)]
    pub(crate) thinking: Option<String>,
    /// The block's opaque signature, delivered via `signature_delta`.
    #[serde(default)]
    pub(crate) signature: Option<String>,
    #[serde(default)]
    pub(crate) partial_json: Option<String>,
}

/// Anthropic message delta event.
#[derive(Debug, Deserialize)]
pub(crate) struct AnthropicMessageDelta {
    #[serde(default)]
    pub(crate) stop_reason: Option<String>,
    #[serde(default)]
    pub(crate) delta: Option<MessageDeltaInner>,
    /// Final output-token count for the message.
    #[serde(default)]
    pub(crate) usage: Option<AnthropicUsage>,
}

/// Inner message delta structure.
#[derive(Debug, Deserialize)]
pub(crate) struct MessageDeltaInner {
    #[serde(default)]
    pub(crate) stop_reason: Option<String>,
}

/// Anthropic error format.
#[derive(Debug, Deserialize)]
pub(crate) struct AnthropicError {
    #[serde(default)]
    pub(crate) message: Option<String>,
    #[serde(default)]
    pub(crate) error: Option<AnthropicErrorInner>,
}

/// Inner error structure.
#[derive(Debug, Deserialize)]
pub(crate) struct AnthropicErrorInner {
    #[serde(default)]
    pub(crate) message: Option<String>,
    /// `overloaded_error`, `rate_limit_error` and the like: what to do, without reading the words
    #[serde(default, rename = "type")]
    pub(crate) r#type: Option<String>,
}

#[cfg(test)]
mod tests;
