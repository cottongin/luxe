//! The captured calls — one provider request with what came back, read out of the log.

use std::collections::BTreeMap;

use super::cache::strip_cache_control;

/// One captured provider call: the request body plus whatever came back.
pub(super) struct Call {
    /// Position in the log. The provider's own call id restarts per process, so a log stitched
    /// from several `luxe` invocations would otherwise show `#0` three times.
    pub(super) index: usize,
    pub(super) provider: String,
    pub(super) url: String,
    pub(super) body: serde_json::Value,
    pub(super) response: Option<serde_json::Value>,
    /// The conversation, read out of `body` once: every section asks for it again
    messages: Vec<serde_json::Value>,
    /// The system prompt, whichever shape this API uses for it, found once
    system: Option<String>,
}

impl Call {
    pub(super) fn model(&self) -> &str {
        self.body.get("model").and_then(|m| m.as_str()).unwrap_or("?")
    }

    /// The `## Tools` section of the system prompt, where tool guidance belongs.
    pub(super) fn tools_section(&self) -> String {
        let system = self.system().unwrap_or_default();
        let Some(start) = system.find("## Tools") else { return String::new() };
        let rest = &system[start..];
        let end = rest[8..].find("\n## ").map(|at| at + 8).unwrap_or(rest.len());
        rest[..end].to_string()
    }

    pub(super) fn new(
        index: usize,
        provider: String,
        url: String,
        body: serde_json::Value,
        response: Option<serde_json::Value>,
    ) -> Self {
        let messages = messages_of(&body);
        let system = system_of(&body, &messages);
        Self { index, provider, url, body, response, messages, system }
    }

    /// The system prompt, whichever shape this API uses for it.
    pub(super) fn system(&self) -> Option<&str> {
        self.system.as_deref()
    }

    pub(super) fn messages(&self) -> &[serde_json::Value] {
        &self.messages
    }

    /// The conversation as a cache sees it: one rendering per message, with the breakpoint markers
    /// stripped. `cache_control` is metadata rather than content, and the rolling one is *supposed*
    /// to move every turn — comparing it would report churn on every transition and hide the real
    /// thing this is looking for.
    pub(super) fn cached_messages(&self) -> Vec<String> {
        self.messages()
            .iter()
            .map(|message| {
                let mut message = message.clone();
                strip_cache_control(&mut message);
                serde_json::to_string(&message).unwrap_or_default()
            })
            .collect()
    }

    pub(super) fn tools(&self) -> Vec<serde_json::Value> {
        self.body.get("tools").and_then(|v| v.as_array()).cloned().unwrap_or_default()
    }

    /// The tools the model actually has in context: every one but those sent with Anthropic's
    /// `defer_loading`, which load only where a `tool_reference` asks for them.
    pub(super) fn loaded_tools(&self) -> Vec<serde_json::Value> {
        self.tools().into_iter().filter(|tool| !is_deferred(tool)).collect()
    }

    /// `(name, description, whole-schema-json)` per loaded tool, across all three wire shapes.
    pub(super) fn tool_entries(&self) -> Vec<(String, String, String)> {
        entries(&self.loaded_tools())
    }

    /// [`Self::tool_entries`] for the deferred tools: on the wire, outside the context.
    pub(super) fn deferred_entries(&self) -> Vec<(String, String, String)> {
        entries(&self.tools().into_iter().filter(is_deferred).collect::<Vec<_>>())
    }

    pub(super) fn body_chars(&self) -> usize {
        serde_json::to_string(&self.body).map(|s| s.chars().count()).unwrap_or(0)
    }

    /// [`Self::body_chars`] less the deferred tool definitions, which the provider doesn't bill.
    pub(super) fn context_chars(&self) -> usize {
        let deferred: usize =
            self.deferred_entries().iter().map(|(_, _, json)| json.chars().count()).sum();
        self.body_chars().saturating_sub(deferred)
    }

    pub(super) fn prompt_tokens(&self) -> Option<u64> {
        self.response
            .as_ref()?
            .get("usage")?
            .get("prompt_tokens")
            .and_then(serde_json::Value::as_u64)
    }
}

/// The messages of a request, whichever key this API keeps them under.
fn messages_of(body: &serde_json::Value) -> Vec<serde_json::Value> {
    for key in ["messages", "input"] {
        if let Some(list) = body.get(key).and_then(|v| v.as_array()) {
            return list.clone();
        }
    }
    Vec::new()
}

/// The system prompt of a request, in whichever of the three shapes it was sent.
fn system_of(body: &serde_json::Value, messages: &[serde_json::Value]) -> Option<String> {
    // Responses API: a top-level `instructions` string.
    if let Some(text) = body.get("instructions").and_then(|v| v.as_str()) {
        return Some(text.to_string());
    }
    // Anthropic: `system` as a string or as content blocks.
    match body.get("system") {
        Some(serde_json::Value::String(text)) => return Some(text.clone()),
        Some(serde_json::Value::Array(blocks)) => {
            let joined: String = blocks
                .iter()
                .filter_map(|block| block.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("");
            if !joined.is_empty() {
                return Some(joined);
            }
        }
        _ => {}
    }
    // Chat completions: a leading `system` message.
    messages
        .iter()
        .find(|m| m.get("role").and_then(|r| r.as_str()) == Some("system"))
        .and_then(|m| m.get("content").and_then(|c| c.as_str()))
        .map(str::to_string)
}

/// Whether a tool definition is sent with Anthropic's `defer_loading`.
pub(super) fn is_deferred(tool: &serde_json::Value) -> bool {
    tool.get("defer_loading").and_then(serde_json::Value::as_bool) == Some(true)
}

/// `(name, description, whole-schema-json)` per tool, across all three wire shapes.
pub(super) fn entries(tools: &[serde_json::Value]) -> Vec<(String, String, String)> {
    tools
        .iter()
        .map(|tool| {
            // Chat completions nests under `function`; Responses and Anthropic are flat.
            let inner = tool.get("function").unwrap_or(tool);
            let name = inner.get("name").and_then(|v| v.as_str()).unwrap_or("?").to_string();
            let description =
                inner.get("description").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let json = serde_json::to_string(tool).unwrap_or_default();
            (name, description, json)
        })
        .collect()
}

/// Read the JSONL log, pairing each request with its response by call id.
pub(super) fn load(text: &str) -> Vec<Call> {
    let mut calls: Vec<Call> = Vec::new();
    let mut index: BTreeMap<u64, usize> = BTreeMap::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(record) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        let id = record.get("call").and_then(serde_json::Value::as_u64).unwrap_or(0);
        let kind = record.get("kind").and_then(|k| k.as_str()).unwrap_or("");
        match kind {
            "request" => {
                index.insert(id, calls.len());
                calls.push(Call::new(
                    calls.len() + 1,
                    record.get("provider").and_then(|p| p.as_str()).unwrap_or("?").to_string(),
                    record.get("url").and_then(|u| u.as_str()).unwrap_or("").to_string(),
                    record.get("body").cloned().unwrap_or(serde_json::Value::Null),
                    None,
                ));
            }
            "response" => {
                if let Some(position) = index.get(&id) {
                    if let Some(call) = calls.get_mut(*position) {
                        call.response = Some(record);
                    }
                }
            }
            _ => {}
        }
    }
    calls
}
