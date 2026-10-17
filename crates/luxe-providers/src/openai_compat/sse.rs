//! Chat-completions SSE decoding — `data:` records to [`ProviderEvent`]s.

use luxe_core::provider::{ProviderEvent, StopReason, Usage};
use serde::Deserialize;
use tracing::warn;

/// SSE event from the OpenAI API.
#[derive(Debug, Deserialize)]
pub(super) struct SseEvent {
    #[serde(default)]
    choices: Option<Vec<SseChoice>>,
    #[serde(default)]
    usage: Option<SseUsage>,
    #[serde(default)]
    error: Option<SseError>,
}

/// A choice in an SSE event.
#[derive(Debug, Deserialize)]
struct SseChoice {
    #[serde(default)]
    index: Option<usize>,
    #[serde(default)]
    delta: Option<SseDelta>,
    #[serde(default)]
    finish_reason: Option<String>,
}

/// Delta in a streaming response.
#[derive(Debug, Deserialize, Default)]
struct SseDelta {
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    content: Option<String>,
    /// Reasoning text, DeepSeek/Qwen/LM Studio spelling.
    #[serde(default)]
    reasoning_content: Option<String>,
    /// Reasoning text, OpenRouter/Together spelling.
    #[serde(default)]
    reasoning: Option<String>,
    /// Reasoning text, GitHub Copilot spelling.
    #[serde(default)]
    reasoning_text: Option<String>,
    /// GitHub Copilot's reasoning state, replayed with the message it came on.
    #[serde(default)]
    reasoning_opaque: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<SseToolCall>>,
}

impl SseDelta {
    /// The reasoning fragment in this delta, whichever field the server chose to put it in.
    ///
    /// There is no standard: DeepSeek-lineage servers (and LM Studio proxying them) send
    /// `reasoning_content`, OpenRouter and Together send `reasoning`, GitHub Copilot sends
    /// `reasoning_text`. All three mean the same thing.
    fn reasoning_delta(&self) -> Option<&str> {
        self.reasoning_content
            .as_deref()
            .or(self.reasoning.as_deref())
            .or(self.reasoning_text.as_deref())
            .filter(|text| !text.is_empty())
    }
}

/// A tool call in a streaming response.
#[derive(Debug, Deserialize)]
struct SseToolCall {
    #[serde(default)]
    index: Option<usize>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<SseFunction>,
}

/// A function call in a streaming response.
#[derive(Debug, Deserialize, Default)]
struct SseFunction {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

/// Usage statistics from the API.
#[derive(Debug, Deserialize, Default)]
struct SseUsage {
    #[serde(default)]
    prompt_tokens: Option<u32>,
    #[serde(default)]
    completion_tokens: Option<u32>,
    #[serde(default)]
    total_tokens: Option<u32>,
    /// Cached-input breakdown; `prompt_tokens` already includes these.
    #[serde(default)]
    prompt_tokens_details: Option<TokenDetails>,
    /// Reasoning-output breakdown; `completion_tokens` already includes these.
    #[serde(default)]
    completion_tokens_details: Option<TokenDetails>,
    /// Reasoning tokens reported beside the counts rather than inside a breakdown, as GitHub
    /// Copilot does.
    #[serde(default)]
    reasoning_tokens: Option<u32>,
}

/// The `*_tokens_details` sub-object (OpenAI reports cached/reasoning subsets here).
#[derive(Debug, Deserialize, Default)]
struct TokenDetails {
    #[serde(default)]
    cached_tokens: Option<u32>,
    /// Tokens written to the cache, which OpenRouter reports and bills at a premium. Counted
    /// inside `prompt_tokens` like the read side, so it needs no adjustment to fit [`Usage`].
    #[serde(default)]
    cache_write_tokens: Option<u32>,
    #[serde(default)]
    reasoning_tokens: Option<u32>,
}

/// Error from the API.
#[derive(Debug, Deserialize)]
struct SseError {
    message: String,
    #[serde(default)]
    r#type: Option<String>,
    #[serde(default)]
    code: Option<serde_json::Value>,
}

/// Translate one decoded chat-completions chunk into `ProviderEvent`s.
pub(super) fn push_sse_events(event: &SseEvent, events: &mut Vec<ProviderEvent>) {
    if let Some(error) = &event.error {
        let (named, code) = (error.r#type.as_deref(), error.code.as_ref());
        let kind = crate::http::kind_of_frame(named, code, &error.message);
        events.push(ProviderEvent::Error { message: error.message.clone(), kind });
        return;
    }

    // Emit usage BEFORE the choices' `Done` (the engine stops the stream on `Done`, so a trailing
    // usage would be missed). Also covers usage-only final chunks with no choices.
    if let Some(usage) = &event.usage {
        events.push(usage_event(usage));
    }

    for choice in event.choices.iter().flatten() {
        let index = choice.index.unwrap_or(0);
        if let Some(delta) = &choice.delta {
            push_delta_events(index, delta, events);
        }
        if let Some(finish_reason) = &choice.finish_reason {
            push_finish_events(index, finish_reason, events);
        }
    }
}

/// One usage chunk as the normalized [`Usage`] the engine counts.
fn usage_event(usage: &SseUsage) -> ProviderEvent {
    let prompt_details = usage.prompt_tokens_details.as_ref();
    let cache_read = prompt_details.and_then(|d| d.cached_tokens).unwrap_or(0);
    let cache_write = prompt_details.and_then(|d| d.cache_write_tokens).unwrap_or(0);
    let reasoning = usage
        .completion_tokens_details
        .as_ref()
        .and_then(|d| d.reasoning_tokens)
        .or(usage.reasoning_tokens)
        .unwrap_or(0);
    let prompt = usage.prompt_tokens.unwrap_or(0);
    let total = usage.total_tokens.unwrap_or(0);
    // Reasoning is output, and inside `completion_tokens` by convention. Copilot's Gemini
    // leaves it out and puts it in the total instead (19 prompt + 1 completion of a total of
    // 1,460, with 1,440 reasoning), which priced a turn's output at a fraction of what it was.
    // Only a total that holds the reasoning on top of both counts says so
    let mut completion = usage.completion_tokens.unwrap_or(0);
    if reasoning > 0
        && u64::from(total) >= u64::from(prompt) + u64::from(completion) + u64::from(reasoning)
    {
        completion += reasoning;
    }
    ProviderEvent::Usage(Usage {
        prompt_tokens: prompt,
        completion_tokens: completion,
        total_tokens: total,
        cache_read_tokens: cache_read,
        cache_write_tokens: cache_write,
        reasoning_tokens: reasoning,
        cache_write_1h_tokens: 0,
    })
}

fn push_delta_events(index: usize, delta: &SseDelta, events: &mut Vec<ProviderEvent>) {
    // Role delta (usually first event).
    if delta.role.is_some() {
        events.push(ProviderEvent::TextStart { index });
    }

    // Reasoning delta, before the content delta of the same chunk: a server that packs
    // both into one chunk produced the reasoning first.
    if let Some(reasoning) = delta.reasoning_delta() {
        events.push(ProviderEvent::ThinkingDelta { index, delta: reasoning.to_string() });
    }
    if let Some(opaque) = delta.reasoning_opaque.as_ref().filter(|opaque| !opaque.is_empty()) {
        events.push(ProviderEvent::ReasoningOpaque { index, opaque: opaque.clone() });
    }

    if let Some(content) = &delta.content {
        events.push(ProviderEvent::TextDelta { index, delta: content.clone() });
    }

    for call in delta.tool_calls.iter().flatten() {
        push_tool_call_events(call, events);
    }
}

fn push_tool_call_events(call: &SseToolCall, events: &mut Vec<ProviderEvent>) {
    let index = call.index.unwrap_or(0);
    if let Some(id) = &call.id {
        events.push(ProviderEvent::ToolCallStart { index, id: id.clone(), name: String::new() });
    }
    let Some(function) = &call.function else { return };
    // The name arrives on the same chunk as the id, so it patches the `ToolCallStart` just
    // pushed rather than emitting an event of its own.
    if let Some(name) = &function.name {
        if let Some(ProviderEvent::ToolCallStart { name: slot, .. }) = events.last_mut() {
            *slot = name.clone();
        }
    }
    if let Some(arguments) = &function.arguments {
        events.push(ProviderEvent::ToolCallDelta { index, json_delta: arguments.clone() });
    }
}

/// A tool-call finish ends the turn too, and says why. Emitting only `ToolCallEnd` left the
/// decoder with nothing deferred, so a body that then ended without `[DONE]` terminated the turn
/// with no `Done` at all — and every tool-call turn that *did* get one reported `EndTurn`, while
/// the Anthropic and Responses adapters reported `ToolCall` for the identical model.
fn push_finish_events(index: usize, finish_reason: &str, events: &mut Vec<ProviderEvent>) {
    let reason = match finish_reason {
        "tool_calls" => {
            events.push(ProviderEvent::ToolCallEnd { index });
            StopReason::ToolCall
        }
        other => {
            events.push(ProviderEvent::TextEnd { index });
            match other {
                "stop" => StopReason::EndTurn,
                "length" => StopReason::Length,
                "content_filter" => StopReason::ContentFilter,
                _ => StopReason::Other(finish_reason.to_string()),
            }
        }
    };
    events.push(ProviderEvent::Done { reason });
}

/// Record-at-a-time decoder for the chat-completions SSE shape.
///
/// `Done` is held back until the `[DONE]` sentinel (or the end of the body) because
/// OpenAI-compatible servers report `include_usage` in a chunk *after* the `finish_reason` chunk,
/// and the turn loop stops on the first `Done` — emitting it eagerly loses the turn's usage.
#[derive(Default)]
pub(crate) struct CompatDecoder {
    deferred_done: Option<StopReason>,
    done_emitted: bool,
}

impl CompatDecoder {
    fn flush_done(&mut self, out: &mut Vec<ProviderEvent>) {
        if self.done_emitted {
            return;
        }
        self.done_emitted = true;
        out.push(ProviderEvent::Done {
            reason: self.deferred_done.take().unwrap_or(StopReason::EndTurn),
        });
    }
}

impl crate::sse::SseDecoder for CompatDecoder {
    fn record(&mut self, record: &str, out: &mut Vec<ProviderEvent>) {
        for line in record.lines() {
            let Some(data) = line.trim().strip_prefix("data:").map(str::trim) else { continue };
            if data == "[DONE]" {
                return self.flush_done(out);
            }
            match serde_json::from_str::<SseEvent>(data) {
                Ok(event) => {
                    let mut decoded = Vec::new();
                    push_sse_events(&event, &mut decoded);
                    for event in decoded {
                        match event {
                            ProviderEvent::Done { reason } => self.deferred_done = Some(reason),
                            other => out.push(other),
                        }
                    }
                }
                Err(error) => warn!(error = %error, data = %data, "failed to parse SSE event"),
            }
        }
    }

    fn finish(&mut self, out: &mut Vec<ProviderEvent>) {
        // A body that ended without `[DONE]` still finished the turn — unconditionally, because
        // the caller cannot tell "the stream ended" from "the stream stopped" and a turn that
        // never terminates leaves the engine with no stop reason to report. A transport failure
        // does not come through here: it is a terminal `Error` event instead.
        self.flush_done(out);
    }
}
