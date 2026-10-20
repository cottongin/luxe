//! Anthropic cache markers — where the request asks for its prefix to be kept.

use super::super::*;

/// Add `cache_control` breakpoints so Anthropic serves the stable prefix (and the conversation so
/// far) from cache instead of re-reading it every turn.
///
/// Anthropic caches the whole prompt prefix up to and including each marked block, in the canonical
/// order tools → system → messages, and allows four markers. luxe spends them on:
///
///   1. the system prompt, at the end of its shared part,
///   2. the last listed tool definition — with (1), the ~static floor. A deferred one can't
///      carry a marker (it's outside the prefix, and Anthropic 400s on the pair),
///   3. the end of the *previous* turn, and
///   4. the end of this one, so the next request reads everything up to here at ~0.1x and writes
///      only its own delta.
///
/// (3) is the one that is easy to leave out and expensive to leave out. A lookup walks back at most
/// twenty content blocks from a marker looking for an entry to read, and a single agentic turn
/// appends more than that whenever the model calls tools in parallel — a reasoning block, a
/// `tool_use` per call, and a `tool_result` per call. Marking where the last request stopped puts
/// an exact entry at that position, so the read never depends on the lookback at all.
///
/// `shared` is how much of the system prompt every session shares; the breakpoint goes at its end
/// rather than the prompt's, so a session in another directory reads that much from cache instead
/// of writing the whole prompt again. The rest rides to the next breakpoint, on the conversation,
/// which keeps the count at Anthropic's four.
pub(crate) fn add_cache_control_markers(
    body: &mut serde_json::Value,
    marker: serde_json::Value,
    shared: usize,
) {
    if let Some(system) = body.get_mut("system") {
        if let Some(text) = system.as_str() {
            *system = match text.split_at_checked(shared) {
                Some((head, tail)) if !head.is_empty() && !tail.trim().is_empty() => {
                    serde_json::json!([
                        {"type": "text", "text": head, "cache_control": marker.clone()},
                        {"type": "text", "text": tail},
                    ])
                }
                _ => serde_json::json!([{
                    "type": "text", "text": text, "cache_control": marker.clone(),
                }]),
            };
        }
    }

    if let Some(tools) = body.get_mut("tools").and_then(|t| t.as_array_mut()) {
        if let Some(last) = tools.iter_mut().rev().find(|tool| tool.get("defer_loading").is_none())
        {
            last["cache_control"] = marker.clone();
        }
    }

    if let Some(messages) = body.get_mut("messages").and_then(|m| m.as_array_mut()) {
        for at in rolling_breakpoints(messages) {
            mark_message_content_cached(&mut messages[at], marker.clone());
        }
    }
}

/// The `cache_control` value every breakpoint in this request carries.
///
/// The hour-long tier is opt-in: it doubles the write premium on each turn's delta — pennies — to
/// stop a six-minute pause re-writing the entire conversation, which is the right trade for
/// interactive work and the wrong one for a burst that never idles.
pub(super) fn cache_marker(compat: &AnthropicCompat) -> serde_json::Value {
    match compat.long_cache_retention {
        true => serde_json::json!({"type": "ephemeral", "ttl": "1h"}),
        false => serde_json::json!({"type": "ephemeral"}),
    }
}

/// Where this request ends, and where the last one did — oldest first, at most two.
///
/// Tool results serialize as a `user` turn, so the loop alternates and the previous request ended
/// at the `user` turn before the last message. A transcript that ends on an assistant reply (a
/// resumed session, a remediation) still gets both: the nearest earlier user turn, then the end.
fn rolling_breakpoints(messages: &[serde_json::Value]) -> Vec<usize> {
    let Some(last) = messages.len().checked_sub(1) else { return Vec::new() };
    let is_user =
        |message: &serde_json::Value| message.get("role").and_then(|r| r.as_str()) == Some("user");
    match messages[..last].iter().rposition(is_user) {
        Some(previous) => vec![previous, last],
        None => vec![last],
    }
}

/// Attach `cache_control` to a message's last cacheable content block, normalizing string content
/// to a single text block first (Anthropic requires the marker on a content block, not the
/// message).
///
/// Reasoning blocks cannot carry one, so a turn that ends in thinking takes the marker on the last
/// block that can hold it rather than losing the breakpoint.
fn mark_message_content_cached(message: &mut serde_json::Value, marker: serde_json::Value) {
    match message.get_mut("content") {
        Some(serde_json::Value::String(text)) => {
            let text = std::mem::take(text);
            message["content"] =
                serde_json::json!([{"type": "text", "text": text, "cache_control": marker}]);
        }
        Some(serde_json::Value::Array(blocks)) => {
            let cacheable = blocks.iter().rposition(|block| {
                !matches!(
                    block.get("type").and_then(|t| t.as_str()),
                    Some("thinking") | Some("redacted_thinking")
                )
            });
            if let Some(block) = cacheable.and_then(|at| blocks[at].as_object_mut()) {
                block.insert("cache_control".to_string(), marker);
            }
        }
        _ => {}
    }
}
