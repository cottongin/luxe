//! Tool activation — which deferred tools a conversation has loaded, read off its transcript.
//!
//! With lazy tools on, a deferred tool's schema reaches the model only after `activate_tools` asks
//! for it, and a successful `activate_tools` call is the one durable record of that. Reading the
//! state from there instead of from a set held in memory keeps every consumer in agreement:
//!
//! - an adapter that grows the request's `tools` list reads the activated set off the path, so a
//!   resumed, forked or freshly started session carries exactly what its own history loaded;
//! - an adapter that loads a tool *in place* — Anthropic's `tool_reference`, OpenAI's
//!   `additional_tools` — finds each activation where it happened, and leaves the cached prefix
//!   above it alone. That's the whole point: changing `tools` mid-session re-bills every cached
//!   token after it at the write rate.
//!
//! [`restore_after_compaction`] covers the one moment an in-place load could be lost: a summary that
//! replaces the call it hung on.

use std::collections::{BTreeSet, HashMap};

use crate::message::{ContentBlock, Message, Role, ToolOutput};
use crate::session::{Entry, EntryKind};
use crate::tool::ACTIVATE_TOOLS_NAME;

/// The tool names an `activate_tools` call asked for, in the order given, without repeats.
///
/// Anything that isn't a string in a `names` array is ignored, the way the tool itself ignores it.
pub fn requested_names(arguments: &serde_json::Value) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for name in arguments
        .get("names")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
    {
        if !names.iter().any(|seen| seen == name) {
            names.push(name.to_string());
        }
    }
    names
}

/// Every name a successful `activate_tools` call in `messages` asked for, sorted.
///
/// A call whose result is an error loaded nothing, and neither did one with no result at all. The
/// names are as the model wrote them, so a caller filters them against what it can actually load.
pub fn activated_tools<'a>(messages: impl IntoIterator<Item = &'a Message>) -> BTreeSet<String> {
    let mut requested: HashMap<&str, Vec<String>> = HashMap::new();
    let mut activated = BTreeSet::new();
    for message in messages {
        for block in &message.content {
            match block {
                ContentBlock::ToolCall { id, name, arguments } if name == ACTIVATE_TOOLS_NAME => {
                    requested.insert(id.as_str(), requested_names(arguments));
                }
                ContentBlock::ToolResult { call_id, is_error: false, .. } => {
                    if let Some(names) = requested.remove(call_id.as_str()) {
                        activated.extend(names);
                    }
                }
                _ => {}
            }
        }
    }
    activated
}

/// [`activated_tools`] over a session path's messages.
///
/// Reads the whole path, including what a compaction replaced, because a tool loaded before the
/// summary is still loaded after it ([`restore_after_compaction`] sees to that on the wire).
pub fn activated_in(entries: &[Entry]) -> BTreeSet<String> {
    activated_tools(entries.iter().filter_map(|entry| match &entry.kind {
        EntryKind::Message(message) => Some(message),
        _ => None,
    }))
}

/// Re-loads, right after a compaction summary, the tools activated in the part it replaced.
///
/// An in-place adapter loads a deferred tool where its `activate_tools` call sits, so a summary
/// that replaces the call unloads the tool, while the prompt promises a loaded tool stays loaded
/// for the rest of the session. This puts a synthetic `activate_tools` exchange directly after the
/// summary naming everything the replaced part loaded. Elsewhere it's a harmless restatement.
///
/// Everything it reads was frozen by the compaction (the summary's own id and the entries before
/// its kept tail), so the exchange is byte-identical on every request until the next compaction,
/// and the cache keeps everything up to it.
///
/// `messages` is [`crate::compaction::context_messages`] of the same `entries`, which leads with
/// the summaries whenever there are any: the newest last, behind any older one a newer cut left on
/// the kept side. The exchange goes behind the newest. Does nothing without a compaction or
/// without activations.
pub fn restore_after_compaction(entries: &[Entry], messages: &mut Vec<Message>) {
    let Some(cut) =
        entries.iter().rposition(|entry| matches!(entry.kind, EntryKind::Compaction { .. }))
    else {
        return;
    };
    let EntryKind::Compaction { first_kept, .. } = &entries[cut].kind else { return };
    let replaced_until =
        entries[..cut].iter().position(|entry| entry.id == *first_kept).unwrap_or(cut);
    let names = activated_in(&entries[..replaced_until]);
    // an older summary a newer cut left on the kept side reads first
    let older_summaries = entries[replaced_until..cut]
        .iter()
        .filter(|entry| matches!(entry.kind, EntryKind::Compaction { .. }))
        .count();
    if names.is_empty() || messages.len() <= older_summaries {
        return;
    }

    let names: Vec<String> = names.into_iter().collect();
    let call_id = format!("restore-{}", entries[cut].id.0);
    let call = Message {
        role: Role::Assistant,
        content: vec![ContentBlock::ToolCall {
            id: call_id.clone(),
            name: ACTIVATE_TOOLS_NAME.to_string(),
            arguments: serde_json::json!({ "names": names }),
        }],
    };
    let result = Message {
        role: Role::Tool,
        content: vec![ContentBlock::ToolResult {
            call_id,
            content: vec![ToolOutput::Text(format!(
                "Activated: {}. They were loaded before the summary above and stay available to \
                 call directly.",
                names.join(", ")
            ))],
            is_error: false,
        }],
    };
    let after_newest = older_summaries + 1;
    messages.splice(after_newest..after_newest, [call, result]);
}

#[cfg(test)]
mod tests;
