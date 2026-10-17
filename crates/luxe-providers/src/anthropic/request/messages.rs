//! Anthropic message serialization — the transcript as the wire's `messages`.

use super::super::*;
use std::collections::{HashMap, HashSet};

/// [`serialize_messages_loading`] with nothing loaded in place.
#[cfg(test)]
pub(crate) fn serialize_messages(
    messages: &[Message],
    accepts_images: bool,
) -> Vec<serde_json::Value> {
    serialize_messages_loading(messages, accepts_images, &HashSet::new())
}

/// Serialize core messages to Anthropic message format, loading the deferred tools named in
/// `in_place` where `activate_tools` asked for them.
///
/// Tool results map to a `user` turn, so a mid-turn steering message right after them would put
/// two `user` turns back to back — which the Messages API rejects. Adjacent same-role turns are
/// merged into one with their content blocks concatenated, which is exactly how the API treats
/// them anyway.
///
/// A turn that serializes to no blocks at all is dropped here rather than sent empty. This is the
/// one place that can know: blocks this adapter alone refuses — reasoning captured from another
/// provider, which has no Anthropic signature to replay — can leave a message with nothing in it,
/// and an empty `content` is a 400 that replays on every later request.
///
/// A successful activation's result becomes a `tool_result` of `tool_reference` blocks, which the
/// API expands into the full definitions at that point in the conversation. It's derived from the
/// transcript alone, so every later request carries the same bytes and the cache keeps them.
pub(crate) fn serialize_messages_loading(
    messages: &[Message],
    accepts_images: bool,
    in_place: &HashSet<&str>,
) -> Vec<serde_json::Value> {
    let activations = activations(messages, in_place);
    let serialized = messages
        .iter()
        .filter_map(|msg| {
            let (role, content) = match msg.role {
                luxe_core::message::Role::User => ("user", serialize_content_blocks(&msg.content)),
                luxe_core::message::Role::Assistant => {
                    ("assistant", serialize_content_blocks(&msg.content))
                }
                luxe_core::message::Role::Tool => (
                    "user",
                    serialize_tool_result_blocks(&msg.content, accepts_images, &activations),
                ),
                // The Messages API takes only `user` and `assistant` turns; a system message
                // rides in `system`, and here it was a 400
                luxe_core::message::Role::System => return None,
            };
            Some(serde_json::json!({ "role": role, "content": content }))
        })
        .filter(|message| {
            message
                .get("content")
                .and_then(serde_json::Value::as_array)
                .is_none_or(|content| !content.is_empty())
        })
        .collect::<Vec<_>>();
    merge_adjacent_roles(serialized)
}

/// The deferred tools each `activate_tools` call loads in place, by call id.
///
/// Only names in `in_place`: a reference to a tool the request doesn't define is a 400 that would
/// replay on every later request, and a core tool needs no loading.
fn activations<'a>(
    messages: &'a [Message],
    in_place: &HashSet<&str>,
) -> HashMap<&'a str, Vec<String>> {
    if in_place.is_empty() {
        return HashMap::new();
    }
    messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            luxe_core::message::ContentBlock::ToolCall { id, name, arguments }
                if name == luxe_core::tool::ACTIVATE_TOOLS_NAME =>
            {
                let names: Vec<String> = luxe_core::tool_activation::requested_names(arguments)
                    .into_iter()
                    .filter(|name| in_place.contains(name.as_str()))
                    .collect();
                (!names.is_empty()).then_some((id.as_str(), names))
            }
            _ => None,
        })
        .collect()
}

/// Folds consecutive same-role turns into one, concatenating their content arrays in order.
fn merge_adjacent_roles(messages: Vec<serde_json::Value>) -> Vec<serde_json::Value> {
    let mut out: Vec<serde_json::Value> = Vec::with_capacity(messages.len());
    for message in messages {
        let same_role = out.last().is_some_and(|last| last.get("role") == message.get("role"));
        if !same_role {
            out.push(message);
            continue;
        }
        let Some(previous) = out.last_mut() else { continue };
        let (Some(target), Some(extra)) = (
            previous.get_mut("content").and_then(serde_json::Value::as_array_mut),
            message.get("content").and_then(serde_json::Value::as_array),
        ) else {
            out.push(message);
            continue;
        };
        target.extend(extra.iter().cloned());
    }
    out
}

/// Serialize content blocks to Anthropic format.
pub(crate) fn serialize_content_blocks(
    blocks: &[luxe_core::message::ContentBlock],
) -> Vec<serde_json::Value> {
    blocks
        .iter()
        .filter_map(|block| match block {
            luxe_core::message::ContentBlock::Text(text) => Some(serde_json::json!({
                "type": "text",
                "text": text,
            })),
            // Reasoning must round-trip byte-for-byte or Anthropic rejects it. A block with no
            // signature can't be replayed safely (it would 400 as an invalid/empty signature), so
            // drop it — this also correctly skips reasoning captured from other providers.
            luxe_core::message::ContentBlock::Thinking { text, signature, .. } => {
                signature.as_ref().map(|signature| {
                    serde_json::json!({
                        "type": "thinking",
                        "thinking": text,
                        "signature": signature,
                    })
                })
            }
            luxe_core::message::ContentBlock::RedactedThinking { data } => {
                Some(serde_json::json!({
                    "type": "redacted_thinking",
                    "data": data,
                }))
            }
            luxe_core::message::ContentBlock::ToolCall { id, name, arguments } => {
                Some(serde_json::json!({
                    "type": "tool_use",
                    "id": id,
                    "name": name,
                    "input": arguments,
                }))
            }
            // Tool results are serialized separately (see `serialize_tool_result_blocks`).
            luxe_core::message::ContentBlock::ToolResult { .. } => None,
            luxe_core::message::ContentBlock::Image { data, mime, .. } => Some(serde_json::json!({
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": mime,
                    "data": data,
                },
            })),
        })
        .collect()
}

/// Serialize tool result blocks to Anthropic format.
///
/// `accepts_images` gates the pixels: this API takes image blocks inside a `tool_result`, so no
/// carrier message and no restructuring is needed — but a model that does not declare vision is
/// still billed for every base64 byte it is sent, so it never receives them.
///
/// A successful call in `activations` answers with its `tool_reference` blocks alone, so the tool's
/// own text is dropped. Nowhere else will take it: the API refuses a result that mixes references
/// with anything else, and a text block beside the results makes the turn a user message, which
/// strips earlier reasoning from the rendered prompt and misses the cache on everything after it.
/// The expanded definitions say what loaded anyway.
pub(crate) fn serialize_tool_result_blocks(
    blocks: &[luxe_core::message::ContentBlock],
    accepts_images: bool,
    activations: &HashMap<&str, Vec<String>>,
) -> Vec<serde_json::Value> {
    blocks
        .iter()
        .filter_map(|block| {
            let luxe_core::message::ContentBlock::ToolResult { call_id, content, is_error } = block
            else {
                return None;
            };
            if let Some(names) = activations.get(call_id.as_str()).filter(|_| !is_error) {
                let references: Vec<serde_json::Value> = names
                    .iter()
                    .map(|name| serde_json::json!({ "type": "tool_reference", "tool_name": name }))
                    .collect();
                return Some(serde_json::json!({
                    "type": "tool_result",
                    "tool_use_id": call_id,
                    "content": references,
                    "is_error": false,
                }));
            }
            let images: Vec<(&str, &str)> = if accepts_images {
                luxe_core::message::ToolOutput::images(content).collect()
            } else {
                Vec::new()
            };
            let text = luxe_core::message::ToolOutput::joined_text(content);

            // The plain string stays the shape for a text-only result, which is nearly all of
            // them: every transcript and wire test already reads that way, and there is nothing to
            // gain from churning it.
            let content = if images.is_empty() {
                serde_json::Value::String(text)
            } else {
                let mut parts = Vec::with_capacity(images.len() + 1);
                if !text.is_empty() {
                    parts.push(serde_json::json!({ "type": "text", "text": text }));
                }
                parts.extend(images.iter().map(|(mime, data)| {
                    serde_json::json!({
                        "type": "image",
                        "source": { "type": "base64", "media_type": mime, "data": data },
                    })
                }));
                serde_json::Value::Array(parts)
            };

            Some(serde_json::json!({
                "type": "tool_result",
                "tool_use_id": call_id,
                "content": content,
                "is_error": is_error,
            }))
        })
        .collect()
}

/// Find a `System`-role message's text, for a caller that passes the prompt that way.
///
/// Luxe's engine does not — it uses [`CompletionRequest::system`] — so this is a fallback.
pub(crate) fn find_system_message(messages: &[Message]) -> Option<String> {
    messages.iter().find(|m| m.role == luxe_core::message::Role::System).map(Message::text)
}
