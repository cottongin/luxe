//! Responses request building — core messages to `POST /responses` wire JSON.

use std::collections::{HashMap, HashSet};

use luxe_core::message::ContentBlock;
use luxe_core::provider::{CompletionRequest, ThinkingLevel};

use super::{OpenaiResponsesProvider, ResponsesFlavor};
use crate::wire_json::data_url;

impl OpenaiResponsesProvider {
    /// Build the JSON request body for `POST /v1/responses`, as the next request would.
    #[cfg(test)]
    pub(crate) fn build_request_body(&self, req: &CompletionRequest) -> serde_json::Value {
        self.body_for(req, self.loads_in_place(req))
    }

    /// Build the JSON request body for `POST /v1/responses`, with the deferred tools loaded in
    /// place when `in_place` is set and there are any.
    ///
    /// In place, the top-level `tools` is the engine's list without the deferred names, so it's the
    /// same bytes before and after an activation, and each successful `activate_tools` result is
    /// followed by an `additional_tools` item carrying what it loaded. A pass that withholds
    /// tools loads nothing either.
    pub(super) fn body_for(&self, req: &CompletionRequest, in_place: bool) -> serde_json::Value {
        let loadable: HashMap<&str, &luxe_core::provider::ToolSchema> =
            if in_place && !req.tools.is_empty() {
                req.deferred_tools.iter().map(|tool| (tool.name.as_str(), tool)).collect()
            } else {
                HashMap::new()
            };

        // --- input array ---
        let input = build_input_items_loading(
            &req.messages,
            req.model.input.contains(&luxe_core::provider::Modality::Image),
            &loadable,
        );

        let mut body = serde_json::json!({
            "model": req.model.r#ref.id,
            "stream": true,
            "input": input,
        });

        // System prompt becomes `instructions`.
        if let Some(system) = &req.system {
            body["instructions"] = serde_json::Value::String(system.clone());
        }

        // Max output tokens (omit when 0 — "no cap"). The Codex backend decides its own cap and
        // rejects the field.
        if self.flavor == ResponsesFlavor::Standard {
            if let Some(max) = req.max_tokens {
                if max > 0 {
                    body["max_output_tokens"] = max.into();
                }
            }
        }

        // The Codex backend never persists a response, and it wants the tool policy stated.
        if self.flavor == ResponsesFlavor::Codex {
            body["store"] = serde_json::Value::Bool(false);
            body["include"] = serde_json::json!(["reasoning.encrypted_content"]);
            body["tool_choice"] = serde_json::Value::String("auto".to_string());
            body["parallel_tool_calls"] = serde_json::Value::Bool(true);
            // Codex requires instructions; an empty turn would otherwise 400.
            if req.system.is_none() {
                body["instructions"] =
                    serde_json::Value::String("You are a helpful coding assistant.".to_string());
            }
        }

        // Reasoning — only for reasoning-capable models. Run statelessly (`store: false`) and ask
        // for encrypted reasoning items so they can be round-tripped across tool calls; the
        // Responses API 400s ("function_call ... without its required reasoning item") if a
        // reasoning model's tool call is replayed without its reasoning. `summary: "auto"` also
        // surfaces human-readable reasoning for display.
        if req.model.reasoning {
            body["store"] = serde_json::Value::Bool(false);
            body["include"] = serde_json::json!(["reasoning.encrypted_content"]);
            let level = req.model.effective_thinking_level(req.thinking);
            let mut reasoning = serde_json::json!({ "summary": "auto" });
            if level != ThinkingLevel::Off {
                let effort = req.model.thinking_level_value(level).unwrap_or(match level {
                    ThinkingLevel::Minimal | ThinkingLevel::Low => "low",
                    ThinkingLevel::High | ThinkingLevel::XHigh => "high",
                    ThinkingLevel::Ultra => "max",
                    _ => "medium",
                });
                reasoning["effort"] = serde_json::Value::String(effort.to_string());
            }
            body["reasoning"] = reasoning;
        }

        // Tools — flat array, NOT nested under "function".
        if !req.tools.is_empty() {
            let tools: Vec<serde_json::Value> = req
                .tools
                .iter()
                .filter(|tool| !loadable.contains_key(tool.name.as_str()))
                .map(function_tool)
                .collect();
            body["tools"] = serde_json::Value::Array(tools);
        }

        // Last, so the model's own recipe outranks every field above it.
        crate::sampling::apply(&mut body, &req.model.sampling_params);
        body
    }
}

/// One `function` tool definition, as `tools` and `additional_tools` both take it.
fn function_tool(t: &luxe_core::provider::ToolSchema) -> serde_json::Value {
    // NOTE: the Responses API defaults `strict` to true, unlike Chat Completions. Left out, every
    // call is decoded against its schema as a grammar, and that grammar allows whitespace between
    // tokens that a model can fall into writing until its output limit: replaying one captured
    // request, 3 of 18 tries ran away like that, and 0 of 36 with this set.
    serde_json::json!({
        "type": "function",
        "name": t.name,
        "description": t.description,
        "parameters": t.parameters,
        "strict": false,
    })
}

/// Whether `body` is a 400 about `additional_tools` itself, so listing the tools instead would get
/// the request through.
pub(super) fn refuses_in_place_loading(body: &str) -> bool {
    body.contains("additional_tools")
}

/// The calls in `messages` to a tool an earlier activation loaded in place, by call id.
///
/// A tool loaded by `additional_tools` lives in a namespace of its own name, and a call to it
/// replays only with that namespace attached (a live 400 without it: "Missing namespace for
/// function_call 'module_report'"). A call with no activation ahead of it comes from a session
/// that had the tool listed, and replays as it was sent; the API takes a call to a tool it no
/// longer has.
fn namespaced_calls<'a>(
    messages: &'a [luxe_core::message::Message],
    loadable: &HashMap<&str, &luxe_core::provider::ToolSchema>,
) -> HashSet<&'a str> {
    let mut namespaced = HashSet::new();
    if loadable.is_empty() {
        return namespaced;
    }
    let mut requested: HashMap<&str, Vec<String>> = HashMap::new();
    let mut loaded: HashSet<String> = HashSet::new();
    for block in messages.iter().flat_map(|message| &message.content) {
        match block {
            ContentBlock::ToolCall { id, name, arguments }
                if name == luxe_core::tool::ACTIVATE_TOOLS_NAME =>
            {
                requested.insert(id, luxe_core::tool_activation::requested_names(arguments));
            }
            ContentBlock::ToolCall { id, name, .. }
                if loadable.contains_key(name.as_str()) && loaded.contains(name) =>
            {
                namespaced.insert(id.as_str());
            }
            ContentBlock::ToolResult { call_id, is_error: false, .. } => {
                loaded.extend(requested.remove(call_id.as_str()).unwrap_or_default());
            }
            _ => {}
        }
    }
    namespaced
}

/// The deferred tools each `activate_tools` call loads in place, by call id.
fn activations<'a>(
    messages: &[luxe_core::message::Message],
    loadable: &HashMap<&str, &'a luxe_core::provider::ToolSchema>,
) -> HashMap<String, Vec<&'a luxe_core::provider::ToolSchema>> {
    if loadable.is_empty() {
        return HashMap::new();
    }
    messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            ContentBlock::ToolCall { id, name, arguments }
                if name == luxe_core::tool::ACTIVATE_TOOLS_NAME =>
            {
                let tools: Vec<_> = luxe_core::tool_activation::requested_names(arguments)
                    .iter()
                    .filter_map(|name| loadable.get(name.as_str()).copied())
                    .collect();
                (!tools.is_empty()).then(|| (id.clone(), tools))
            }
            _ => None,
        })
        .collect()
}

/// One `input_image` content part carrying bytes as a data URL.
fn input_image(mime: &str, data: &str) -> serde_json::Value {
    serde_json::json!({ "type": "input_image", "image_url": data_url(mime, data) })
}

/// A tool result's `output`: the plain string it has always been, or the content array this API
/// accepts when the result carries pixels.
///
/// Unlike chat-completions, nothing has to be restructured here — `function_call_output.output` is
/// `string | Array<input_text | input_image | input_file>`, so the image rides inside the result it
/// belongs to and no `user` message is interleaved. The string form is kept whenever there is no
/// image, because that is what every existing transcript and wire test reads.
fn function_call_output(
    content: &[luxe_core::ToolOutput],
    accepts_images: bool,
) -> serde_json::Value {
    let text = luxe_core::ToolOutput::joined_text(content);
    let images: Vec<serde_json::Value> = if accepts_images {
        luxe_core::ToolOutput::images(content).map(|(mime, data)| input_image(mime, data)).collect()
    } else {
        Vec::new()
    };

    if images.is_empty() {
        let carried = luxe_core::ToolOutput::images(content).next().is_some();
        // Nothing but an image, and no way to send it: say so rather than send an empty result.
        return serde_json::Value::String(if text.is_empty() && carried {
            "[image]".to_string()
        } else {
            text
        });
    }
    let mut parts = Vec::with_capacity(images.len() + 1);
    parts.push(serde_json::json!({
        "type": "input_text",
        "text": if text.is_empty() { "(see attached image)".to_string() } else { text },
    }));
    parts.extend(images);
    serde_json::Value::Array(parts)
}

/// [`build_input_items_loading`] with nothing loaded in place.
#[cfg(test)]
pub(super) fn build_input_items(
    messages: &[luxe_core::message::Message],
    accepts_images: bool,
) -> Vec<serde_json::Value> {
    build_input_items_loading(messages, accepts_images, &HashMap::new())
}

/// Convert the conversation history into the flat `input` array the Responses API expects.
///
/// A successful `activate_tools` result is followed by an `additional_tools` item carrying the
/// `loadable` tools it asked for, which makes them callable from that point on. It's derived from
/// the transcript alone, so it sits in the same place on every later request and the cache keeps
/// everything before it.
fn build_input_items_loading(
    messages: &[luxe_core::message::Message],
    accepts_images: bool,
    loadable: &HashMap<&str, &luxe_core::provider::ToolSchema>,
) -> Vec<serde_json::Value> {
    use luxe_core::message::Role;

    let activations = activations(messages, loadable);
    let namespaced = namespaced_calls(messages, loadable);
    let mut items: Vec<serde_json::Value> = Vec::new();

    for msg in messages {
        match msg.role {
            Role::User => items.push(user_item(msg, accepts_images)),
            Role::Assistant => assistant_items(msg, &namespaced, &mut items),
            Role::Tool => tool_items(msg, accepts_images, &activations, &mut items),
            // System role is handled via `instructions`; skip.
            Role::System => {}
        }
    }

    items
}

/// A user turn: its text, with its images beside it when the model takes them.
fn user_item(msg: &luxe_core::message::Message, accepts_images: bool) -> serde_json::Value {
    let text = msg.text();
    let images: Vec<serde_json::Value> = if accepts_images {
        msg.content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Image { data, mime, .. } => Some(input_image(mime, data)),
                _ => None,
            })
            .collect()
    } else {
        Vec::new()
    };
    if images.is_empty() {
        return serde_json::json!({ "role": "user", "content": text });
    }
    let mut parts = vec![serde_json::json!({ "type": "input_text", "text": text })];
    parts.extend(images);
    serde_json::json!({ "role": "user", "content": parts })
}

/// An assistant turn may contain text, tool calls, or both, each its own item in content order.
fn assistant_items(
    msg: &luxe_core::message::Message,
    namespaced: &HashSet<&str>,
    items: &mut Vec<serde_json::Value>,
) {
    for block in &msg.content {
        match block {
            ContentBlock::Text(text) => {
                items.push(serde_json::json!({ "role": "assistant", "content": text }));
            }
            ContentBlock::Thinking { text, id, encrypted, .. } => {
                items.extend(reasoning_item(text, id.as_deref(), encrypted.as_deref()));
            }
            // Anthropic's encrypted reasoning has no OpenAI equivalent; skip it.
            ContentBlock::RedactedThinking { .. } => {}
            ContentBlock::ToolCall { id, name, arguments } => {
                let mut item = serde_json::json!({
                    "type": "function_call",
                    "call_id": id,
                    "name": name,
                    "arguments": arguments.to_string(),
                });
                if namespaced.contains(id.as_str()) {
                    item["namespace"] = serde_json::Value::String(name.clone());
                }
                items.push(item);
            }
            // ToolResult inside an assistant block is unusual; skip.
            _ => {}
        }
    }
}

/// Replay OpenAI-origin reasoning verbatim, in content order, so each reasoning item still
/// precedes its function_call (required, or the API 400s).
///
/// Anthropic thinking (signature only, no id/encrypted) is skipped. Under `store: false` an item
/// without `encrypted_content` is dropped by the API, so only emit when we captured it.
fn reasoning_item(
    text: &str,
    id: Option<&str>,
    encrypted: Option<&str>,
) -> Option<serde_json::Value> {
    if id.is_none() && encrypted.is_none() {
        return None;
    }
    let mut item = serde_json::json!({ "type": "reasoning" });
    if let Some(id) = id {
        item["id"] = serde_json::Value::String(id.to_string());
    }
    if let Some(encrypted) = encrypted {
        item["encrypted_content"] = serde_json::Value::String(encrypted.to_string());
    }
    item["summary"] = if text.is_empty() {
        serde_json::json!([])
    } else {
        serde_json::json!([{ "type": "summary_text", "text": text }])
    };
    Some(item)
}

/// Each `ToolResult` block becomes a separate `function_call_output` item, followed by the
/// `additional_tools` item its activation loaded.
fn tool_items(
    msg: &luxe_core::message::Message,
    accepts_images: bool,
    activations: &HashMap<String, Vec<&luxe_core::provider::ToolSchema>>,
    items: &mut Vec<serde_json::Value>,
) {
    let mut emitted = false;
    for block in &msg.content {
        if let ContentBlock::ToolResult { call_id, content, is_error } = block {
            items.push(serde_json::json!({
                "type": "function_call_output",
                "call_id": call_id,
                "output": function_call_output(content, accepts_images),
            }));
            emitted = true;
            if let Some(tools) = activations.get(call_id).filter(|_| !is_error) {
                items.push(serde_json::json!({
                    "type": "additional_tools",
                    "role": "developer",
                    "tools": tools.iter().map(|tool| function_tool(tool)).collect::<Vec<_>>(),
                }));
            }
        }
    }
    if !emitted {
        // Defensive: Tool message with no ToolResult blocks.
        items.push(serde_json::json!({
            "type": "function_call_output",
            "call_id": "",
            "output": "",
        }));
    }
}
