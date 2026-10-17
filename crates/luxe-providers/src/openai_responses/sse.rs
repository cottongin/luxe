//! Responses SSE decoding — `event:`/`data:` records to [`ProviderEvent`]s.

use luxe_core::provider::{ProviderErrorKind, ProviderEvent, StopReason, Usage};
use serde::Deserialize;
use tracing::warn;

/// Parsed representation of a single SSE event from the Responses API.
#[derive(Debug)]
enum ResponsesSseEvent {
    /// `response.output_text.delta` — streaming text chunk.
    OutputTextDelta { delta: String },
    /// `response.output_item.added` with `item.type == "function_call"`.
    FunctionCallAdded { output_index: usize, call_id: String, name: String },
    /// `response.function_call_arguments.delta` — streaming tool-call JSON chunk.
    FunctionCallArgumentsDelta { output_index: usize, delta: String },
    /// `response.reasoning_summary_text.delta` — streaming reasoning summary text at its item's
    /// output index (so it groups with that item's id/encrypted_content).
    ReasoningSummaryDelta { output_index: usize, delta: String },
    /// `response.reasoning_summary_part.added` — a new *paragraph* of the summary begins.
    ///
    /// One reasoning item's summary may arrive in several parts, and the deltas carry no separator
    /// between them, so concatenating them ran the last sentence of one paragraph into the first of
    /// the next. The part index is what tells them apart: part 0 opens the summary and needs no
    /// break, and every part after it does.
    ReasoningSummaryPartAdded { output_index: usize, summary_index: usize },
    /// `response.output_item.done` with `item.type == "reasoning"` — the finalized reasoning item,
    /// carrying its `id` and (when requested) `encrypted_content` for stateless replay.
    ReasoningItemDone { output_index: usize, id: Option<String>, encrypted: Option<String> },
    /// `response.completed` — the full response is done.
    Completed { usage: Option<RespUsage>, has_function_call: bool },
    /// `response.failed` / `response.error` / generic `error` — something went wrong.
    Error { message: String, code: Option<serde_json::Value> },
}

/// An error event's payload: `error` says it at the top, `response.failed` under `response.error`.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ErrorFrame {
    message: Option<String>,
    code: Option<serde_json::Value>,
    error: Option<Box<ErrorFrame>>,
    response: Option<Box<ErrorFrame>>,
}

/// Usage block inside `response.completed`.
#[derive(Debug, Deserialize)]
struct RespUsage {
    input_tokens: u32,
    output_tokens: u32,
    /// Cached-input breakdown; `input_tokens` already includes these.
    #[serde(default)]
    input_tokens_details: Option<RespTokenDetails>,
    /// Reasoning-output breakdown; `output_tokens` already includes these.
    #[serde(default)]
    output_tokens_details: Option<RespTokenDetails>,
}

/// The `*_tokens_details` sub-object on a Responses usage block.
#[derive(Debug, Deserialize, Default)]
struct RespTokenDetails {
    #[serde(default)]
    cached_tokens: u32,
    /// What the request wrote to the cache, which OpenAI now reports and the catalog prices
    #[serde(default)]
    cache_write_tokens: u32,
    #[serde(default)]
    reasoning_tokens: u32,
}

/// Parse all SSE events from the raw text body and return ProviderEvents.
///
/// Each SSE "message" is separated by a blank line and looks like:
/// ```text
/// event: response.output_text.delta
/// data: {"delta":"..."}
/// ```
#[cfg(test)]
pub(crate) fn parse_responses_sse(text: &str) -> Vec<ProviderEvent> {
    let mut events = Vec::new();
    for chunk in text.split("\n\n") {
        if let Some(ev) = parse_single_event(chunk) {
            push_provider_events(ev, &mut events);
        }
    }
    events
}

/// Parse one SSE chunk (potentially multi-line) into a typed event.
fn parse_single_event(chunk: &str) -> Option<ResponsesSseEvent> {
    let mut event_type: Option<&str> = None;
    let mut data: Option<&str> = None;

    for line in chunk.lines() {
        if let Some(t) = line.strip_prefix("event: ") {
            event_type = Some(t.trim());
        } else if let Some(d) = line.strip_prefix("data: ") {
            data = Some(d.trim());
        }
    }

    let event_type = event_type?;
    let data = data.unwrap_or("{}");

    match event_type {
        "response.output_text.delta" => parse_text_delta(data),
        "response.output_item.added" => parse_item_added(data),
        "response.function_call_arguments.delta" => parse_arguments_delta(data),
        "response.reasoning_summary_part.added" => parse_summary_part_added(data),
        "response.reasoning_summary_text.delta" => parse_summary_delta(data),
        "response.output_item.done" => parse_item_done(data),
        "response.completed" => Some(parse_completed(data)),
        "response.failed" | "response.error" | "error" => Some(parse_error(event_type, data)),
        _ => None,
    }
}

fn parse_text_delta(data: &str) -> Option<ResponsesSseEvent> {
    #[derive(Deserialize)]
    struct D {
        delta: String,
    }
    let d: D = serde_json::from_str(data).ok()?;
    Some(ResponsesSseEvent::OutputTextDelta { delta: d.delta })
}

/// Only a `function_call` item is of interest; any other kind starting is not an event.
fn parse_item_added(data: &str) -> Option<ResponsesSseEvent> {
    #[derive(Deserialize)]
    struct D {
        output_index: usize,
        item: Item,
    }
    #[derive(Deserialize)]
    struct Item {
        #[serde(rename = "type")]
        item_type: String,
        #[serde(default)]
        call_id: String,
        #[serde(default)]
        name: String,
    }
    let d: D = serde_json::from_str(data).ok()?;
    if d.item.item_type != "function_call" {
        return None;
    }
    Some(ResponsesSseEvent::FunctionCallAdded {
        output_index: d.output_index,
        call_id: d.item.call_id,
        name: d.item.name,
    })
}

fn parse_arguments_delta(data: &str) -> Option<ResponsesSseEvent> {
    #[derive(Deserialize)]
    struct D {
        output_index: usize,
        delta: String,
    }
    let d: D = serde_json::from_str(data).ok()?;
    Some(ResponsesSseEvent::FunctionCallArgumentsDelta {
        output_index: d.output_index,
        delta: d.delta,
    })
}

fn parse_summary_part_added(data: &str) -> Option<ResponsesSseEvent> {
    #[derive(Deserialize)]
    struct D {
        #[serde(default)]
        output_index: usize,
        #[serde(default)]
        summary_index: usize,
    }
    let d: D = serde_json::from_str(data).ok()?;
    Some(ResponsesSseEvent::ReasoningSummaryPartAdded {
        output_index: d.output_index,
        summary_index: d.summary_index,
    })
}

fn parse_summary_delta(data: &str) -> Option<ResponsesSseEvent> {
    #[derive(Deserialize)]
    struct D {
        #[serde(default)]
        output_index: usize,
        delta: String,
    }
    let d: D = serde_json::from_str(data).ok()?;
    Some(ResponsesSseEvent::ReasoningSummaryDelta { output_index: d.output_index, delta: d.delta })
}

/// The finalized reasoning item carries its id and (when `include` requested it) the
/// `encrypted_content` we must replay on later turns.
fn parse_item_done(data: &str) -> Option<ResponsesSseEvent> {
    #[derive(Deserialize)]
    struct D {
        #[serde(default)]
        output_index: usize,
        item: Item,
    }
    #[derive(Deserialize)]
    struct Item {
        #[serde(rename = "type")]
        item_type: String,
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        encrypted_content: Option<String>,
    }
    let d: D = serde_json::from_str(data).ok()?;
    if d.item.item_type != "reasoning" {
        return None;
    }
    Some(ResponsesSseEvent::ReasoningItemDone {
        output_index: d.output_index,
        id: d.item.id.filter(|id| !id.is_empty()),
        encrypted: d.item.encrypted_content.filter(|enc| !enc.is_empty()),
    })
}

/// Shape: `{ "response": { "usage": {...}, "output": [...] } }`.
fn parse_completed(data: &str) -> ResponsesSseEvent {
    #[derive(Deserialize)]
    struct D {
        response: Resp,
    }
    #[derive(Deserialize)]
    struct Resp {
        #[serde(default)]
        usage: Option<RespUsage>,
        #[serde(default)]
        output: Vec<OutputItem>,
    }
    #[derive(Deserialize)]
    struct OutputItem {
        #[serde(rename = "type")]
        item_type: String,
    }
    match serde_json::from_str::<D>(data) {
        Ok(d) => {
            let has_function_call =
                d.response.output.iter().any(|o| o.item_type == "function_call");
            ResponsesSseEvent::Completed { usage: d.response.usage, has_function_call }
        }
        Err(e) => {
            warn!(error = %e, "failed to parse response.completed data");
            // Still emit a completed event without usage.
            ResponsesSseEvent::Completed { usage: None, has_function_call: false }
        }
    }
}

fn parse_error(event_type: &str, data: &str) -> ResponsesSseEvent {
    let frame: ErrorFrame = serde_json::from_str(data).unwrap_or_default();
    let inner = frame.response.and_then(|response| response.error).or(frame.error);
    let inner = inner.map(|inner| *inner).unwrap_or_default();
    let fallback = if event_type == "error" { "unknown error" } else { event_type };
    let message = frame.message.or(inner.message).unwrap_or_else(|| fallback.to_string());
    ResponsesSseEvent::Error { message, code: frame.code.or(inner.code) }
}

/// Convert a typed Responses API SSE event into one or more ProviderEvents.
fn push_provider_events(event: ResponsesSseEvent, out: &mut Vec<ProviderEvent>) {
    match event {
        ResponsesSseEvent::OutputTextDelta { delta } => {
            out.push(ProviderEvent::TextDelta { index: 0, delta });
        }

        ResponsesSseEvent::FunctionCallAdded { output_index, call_id, name } => {
            out.push(ProviderEvent::ToolCallStart { index: output_index, id: call_id, name });
        }

        ResponsesSseEvent::FunctionCallArgumentsDelta { output_index, delta } => {
            out.push(ProviderEvent::ToolCallDelta { index: output_index, json_delta: delta });
        }

        ResponsesSseEvent::ReasoningSummaryDelta { output_index, delta } => {
            out.push(ProviderEvent::ThinkingDelta { index: output_index, delta });
        }

        // The break belongs *before* the part that follows it, and only from the second part on:
        // emitting it for part 0 would open every reasoning block with a blank line.
        ResponsesSseEvent::ReasoningSummaryPartAdded { output_index, summary_index } => {
            if summary_index > 0 {
                out.push(ProviderEvent::ThinkingDelta {
                    index: output_index,
                    delta: "\n\n".to_string(),
                });
            }
        }

        ResponsesSseEvent::ReasoningItemDone { output_index, id, encrypted } => {
            out.push(ProviderEvent::ReasoningItem { index: output_index, id, encrypted });
        }

        ResponsesSseEvent::Completed { usage, has_function_call } => {
            // Emit usage first (before Done; the turn loop stops on Done).
            if let Some(u) = usage {
                let cache_read = u.input_tokens_details.as_ref().map_or(0, |d| d.cached_tokens);
                let cache_write =
                    u.input_tokens_details.as_ref().map_or(0, |d| d.cache_write_tokens);
                let reasoning = u.output_tokens_details.as_ref().map_or(0, |d| d.reasoning_tokens);
                out.push(ProviderEvent::Usage(Usage {
                    prompt_tokens: u.input_tokens,
                    completion_tokens: u.output_tokens,
                    total_tokens: u.input_tokens + u.output_tokens,
                    cache_read_tokens: cache_read,
                    cache_write_tokens: cache_write,
                    reasoning_tokens: reasoning,
                    cache_write_1h_tokens: 0,
                }));
            }
            let reason = if has_function_call { StopReason::ToolCall } else { StopReason::EndTurn };
            out.push(ProviderEvent::Done { reason });
        }

        ResponsesSseEvent::Error { message, code } => {
            let kind = code.and_then(|code| crate::http::kind_of_code(&code, &message));
            let kind =
                kind.unwrap_or_else(|| ProviderErrorKind::Other("responses_api_error".into()));
            out.push(ProviderEvent::Error { message, kind });
        }
    }
}

/// Record-at-a-time decoder for the Responses API.
///
/// Every record on this API is self-contained (usage rides along with `response.completed`), so
/// unlike the chat-completions decoder this one needs no state.
pub(super) struct ResponsesDecoder;

impl crate::sse::SseDecoder for ResponsesDecoder {
    fn record(&mut self, record: &str, out: &mut Vec<ProviderEvent>) {
        if let Some(event) = parse_single_event(record) {
            push_provider_events(event, out);
        }
    }
}
