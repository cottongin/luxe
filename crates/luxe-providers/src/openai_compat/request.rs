//! Chat-completions request building — core messages to `POST /chat/completions` wire JSON.

use std::collections::HashMap;

use luxe_core::message::ContentBlock;
use luxe_core::provider::CompletionRequest;

use super::thinking::apply_thinking_config;
use super::OpenaiCompatProvider;
use crate::config::CompatFlags;
use crate::wire_json::data_url;

impl OpenaiCompatProvider {
    /// The body a request would go out with, for tests elsewhere in the crate.
    #[cfg(test)]
    pub(crate) fn body_for_tests(&self, req: &CompletionRequest) -> serde_json::Value {
        self.build_request_body(req)
    }

    /// Build the JSON request body for `POST /chat/completions`.
    pub(super) fn build_request_body(&self, req: &CompletionRequest) -> serde_json::Value {
        let compat = self.compat_for(req);
        // Empty unless this endpoint needs the name, which is the only reason to walk the transcript
        // for it — an absent entry then simply means no name to attach.
        let tool_names = match compat.requires_tool_result_name {
            true => tool_names_by_call(&req.messages),
            false => HashMap::new(),
        };
        let mut messages: Vec<serde_json::Value> = Vec::with_capacity(req.messages.len());
        // A `tool` message has nowhere to put an image — this schema allows content parts only on
        // a `user` message — and a `user` message may not be interleaved between two `tool`
        // messages. luxe emits consecutive tool messages routinely (refusals, then results, plus a
        // synthesized repair), so the pixels are collected across the whole *run* and handed over
        // once, after every tool message in it.
        let accepts_images = req.model.input.contains(&luxe_core::provider::Modality::Image);
        let mut index = 0;
        while index < req.messages.len() {
            if req.messages[index].role != luxe_core::message::Role::Tool {
                push_wire_messages(
                    &req.messages[index],
                    &mut messages,
                    &tool_names,
                    compat.copilot_headers,
                );
                index += 1;
                continue;
            }
            let mut images: Vec<(String, String)> = Vec::new();
            while index < req.messages.len()
                && req.messages[index].role == luxe_core::message::Role::Tool
            {
                push_tool_messages(
                    &req.messages[index],
                    &mut messages,
                    accepts_images.then_some(&mut images),
                    &tool_names,
                );
                index += 1;
            }
            if let Some(carrier) = image_carrier_message(&images) {
                messages.push(carrier);
            }
        }

        if let Some(system) = &req.system {
            // `developer` is the newer OpenAI spelling of the system turn. The flag parsed,
            // resolved and displayed while this line said `system` regardless, which is a setting
            // that reads back what you set and does nothing.
            //
            // Honored rather than dropped because the local backends it defaults on for all take
            // it: Ollama forwards the role verbatim, llama.cpp accepts it and folds it into
            // `system` for ordinary templates while keeping the two distinct for GPT-OSS/Harmony,
            // where the difference is real, and LM Studio serves it fine.
            let role = if compat.supports_developer_role { "developer" } else { "system" };
            messages.insert(0, serde_json::json!({"role": role, "content": system}));
        }
        let mut body = serde_json::json!({
            "model": req.model.r#ref.id,
            "messages": serde_json::Value::Null,
            "stream": true,
        });
        body["messages"] = serde_json::Value::Array(messages); // moved in; `json!` would clone it

        // Ask for usage in the streamed response so token/cost/context accounting works. Most
        // OpenAI-compatible servers require this opt-in; without it the stream omits usage.
        if compat.supports_usage_in_streaming {
            body["stream_options"] = serde_json::json!({ "include_usage": true });
        }

        if !req.tools.is_empty() {
            body["tools"] = serde_json::Value::Array(
                req.tools
                    .iter()
                    .map(|t| {
                        serde_json::json!({
                            "type": "function",
                            "function": {
                                "name": t.name,
                                "description": t.description,
                                "parameters": t.parameters
                            }
                        })
                    })
                    .collect::<Vec<_>>(),
            );
        }

        if let Some(max_tokens) = req.max_tokens {
            let field = if compat.max_tokens_field == "max_completion_tokens" {
                "max_completion_tokens"
            } else {
                "max_tokens"
            };
            body[field] = max_tokens.into();
        }

        apply_thinking_config(&mut body, req, compat);
        apply_cache_control(&mut body, compat, req.reuse);
        // Last, so the model's own recipe outranks every field above it.
        crate::sampling::apply(&mut body, &req.model.sampling_params);
        body
    }
}

/// Ask a gateway that supports it to serve the stable prefix of this turn from cache.
///
/// Only OpenRouter's spelling lives here, and it is one field: the gateway places the breakpoint on
/// the last cacheable block itself and advances it as the conversation grows, which is exactly the
/// shape an agentic session wants and the reason not to hand-place markers the way the Anthropic
/// adapter must. Worth it by a wide margin — a Claude turn through OpenRouter costs ~10x less from
/// the second turn on — and the models that don't cache this way ignore the field rather than
/// refusing it, so it needs no per-model gate. Anthropic is the one that needs asking: OpenRouter
/// does *not* cache Claude implicitly, so without this every turn re-reads the whole transcript at
/// full price.
fn apply_cache_control(
    body: &mut serde_json::Value,
    compat: &CompatFlags,
    reuse: luxe_core::provider::PromptReuse,
) {
    // A one-off prompt is never read back, so a breakpoint on it only buys the write premium.
    if compat.cache_control_format.as_deref() == Some("openrouter") && reuse.worth_caching() {
        body["cache_control"] = serde_json::json!({"type": "ephemeral"});
    }
}

/// Convert one core [`Message`](luxe_core::message::Message) into one or more OpenAI wire
/// messages, appending them to `out`.
///
/// A `Tool` message fans out to one `{role:"tool", tool_call_id, content}` per result block
/// (OpenAI requires the id to pair a result with its call). Assistant tool calls become the
/// `tool_calls` array; when the assistant only called tools its `content` is `null`.
///
/// `copilot` replays the reasoning Copilot handed back with the message (`reasoning_text` and
/// `reasoning_opaque`), which no other chat-completions server knows.
pub(super) fn push_wire_messages(
    msg: &luxe_core::message::Message,
    out: &mut Vec<serde_json::Value>,
    tool_names: &HashMap<&str, &str>,
    copilot: bool,
) {
    use luxe_core::message::Role;

    if msg.role == Role::Tool {
        push_tool_messages(msg, out, None, tool_names);
        return;
    }

    let mut text_parts: Vec<String> = Vec::new();
    let mut tool_calls = Vec::new();
    let mut image_blocks: Vec<(String, String)> = Vec::new(); // (mime, base64_data)
    let mut reasoning: Option<(String, String)> = None; // (text, opaque)
    for block in &msg.content {
        match block {
            ContentBlock::Text(text) => {
                text_parts.push(text.clone());
            }
            // Copilot's own reasoning goes back in its own fields, the way it came, so the model
            // keeps the thread of a multi-step call (Gemini's thought signatures ride in
            // `reasoning_opaque`)
            ContentBlock::Thinking { text, opaque: Some(opaque), .. } if copilot => {
                let (texts, opaques) = reasoning.get_or_insert_with(Default::default);
                texts.push_str(text);
                opaques.push_str(opaque);
            }
            // Anything else is transient on the chat-completions path: replaying it as assistant
            // text wastes tokens, breaks prefix caching, and isn't the shape this API expects.
            // Reasoning-capable models regenerate their own reasoning server-side.
            ContentBlock::Thinking { .. } | ContentBlock::RedactedThinking { .. } => {}
            ContentBlock::ToolCall { id, name, arguments } => {
                tool_calls.push(serde_json::json!({
                    "id": id,
                    "type": "function",
                    "function": { "name": name, "arguments": arguments.to_string() }
                }));
            }
            ContentBlock::ToolResult { content, .. } => {
                // A tool result nested inside a non-tool message has no run to be carried
                // alongside, so its images stay a marker.
                let text = tool_result_text(content, false);
                if !text.is_empty() {
                    text_parts.push(text);
                }
            }
            ContentBlock::Image { data, mime, .. } => {
                image_blocks.push((mime.clone(), data.clone()));
            }
        }
    }

    let mut m = serde_json::json!({ "role": msg.role });
    if !image_blocks.is_empty() {
        // Image content: use array form with mixed text and image_url objects.
        let mut arr: Vec<serde_json::Value> = text_parts
            .into_iter()
            .map(|t| serde_json::json!({ "type": "text", "text": t }))
            .collect();
        for (mime, data) in &image_blocks {
            arr.push(serde_json::json!({
                "type": "image_url",
                "image_url": { "url": data_url(mime, data) }
            }));
        }
        m["content"] = serde_json::Value::Array(arr);
        if !tool_calls.is_empty() {
            m["tool_calls"] = serde_json::Value::Array(tool_calls);
        }
    } else if tool_calls.is_empty() {
        // No tool calls: use string content.
        m["content"] = serde_json::Value::String(text_parts.join("\n"));
    } else {
        // Tool calls present: use string content if there's text, null otherwise.
        let text = text_parts.join("\n");
        m["content"] =
            if text.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(text) };
        m["tool_calls"] = serde_json::Value::Array(tool_calls);
    }
    if let Some((text, opaque)) = reasoning {
        if !text.is_empty() {
            m["reasoning_text"] = serde_json::Value::String(text);
        }
        m["reasoning_opaque"] = serde_json::Value::String(opaque);
    }
    out.push(m);
}

/// The headers this endpoint wants beyond `Content-Type`, gathered in one place so the call site
/// stays one line however many providers grow one.
pub(super) fn request_headers(
    compat: &CompatFlags,
    req: &CompletionRequest,
) -> Vec<(&'static str, &'static str)> {
    let mut headers = copilot_request_headers(compat, req);
    if compat.openrouter_headers {
        headers.extend([("HTTP-Referer", LUXE_HOME), ("X-Title", "luxe")]);
    }
    headers
}

/// Where OpenRouter's attribution header points a reader who wants to know what called it.
pub(super) const LUXE_HOME: &str = "https://luxe.dev";

/// GitHub Copilot's per-request headers, empty for every other provider.
///
/// Whichever wire API Copilot is spoken in, so the Responses adapter sends them too; the headers
/// themselves are [`crate::copilot::turn_headers`].
pub(crate) fn copilot_request_headers(
    compat: &CompatFlags,
    req: &CompletionRequest,
) -> Vec<(&'static str, &'static str)> {
    match compat.copilot_headers {
        true => crate::copilot::turn_headers(req),
        false => Vec::new(),
    }
}

/// Which tool produced each result, read off the calls in the same request.
///
/// A result carries the id of the call it answers, never the tool's name — so for an endpoint that
/// wants `name` on a tool message, the name has to come from the assistant turn that made the call.
/// Safe to read from the outgoing transcript: a call and its result are one unit that is never
/// separated (`tool_pairing::repair` guarantees it on this copy), so the call is present whenever
/// its result is.
fn tool_names_by_call(messages: &[luxe_core::message::Message]) -> HashMap<&str, &str> {
    messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            ContentBlock::ToolCall { id, name, .. } => Some((id.as_str(), name.as_str())),
            _ => None,
        })
        .collect()
}

/// Emit one `tool` message per result block, draining any images into `images` when given.
///
/// `images` is `Some` only when the model declares it takes them; a text-only endpoint is billed
/// for every base64 byte it is sent and the strict ones answer 400, so a model that cannot see is
/// never handed pixels.
fn push_tool_messages(
    msg: &luxe_core::message::Message,
    out: &mut Vec<serde_json::Value>,
    mut images: Option<&mut Vec<(String, String)>>,
    tool_names: &HashMap<&str, &str>,
) {
    let mut emitted = false;
    for block in &msg.content {
        if let ContentBlock::ToolResult { call_id, content, .. } = block {
            if let Some(collected) = images.as_deref_mut() {
                collected.extend(
                    luxe_core::ToolOutput::images(content)
                        .map(|(mime, data)| (mime.to_string(), data.to_string())),
                );
            }
            let mut message = serde_json::json!({
                "role": "tool",
                "tool_call_id": call_id,
                "content": tool_result_text(content, images.is_some()),
            });
            // Only where the endpoint asks for it, and only when the call that produced this result
            // is in the same request — a name guessed from anywhere else would be worse than none.
            if let Some(name) = tool_names.get(call_id.as_str()) {
                message["name"] = serde_json::Value::String((*name).to_string());
            }
            out.push(message);
            emitted = true;
        }
    }
    // Defensive: a Tool message with no ToolResult blocks still needs a turn.
    if !emitted {
        out.push(serde_json::json!({ "role": "tool", "content": "" }));
    }
}

/// The one `user` message carrying every image a run of tool results produced.
///
/// It leads with a line of text on purpose: several OpenAI-compatible servers reject a content
/// array with no text part, and a bare wall of base64 tells the model nothing about where the
/// pixels came from.
fn image_carrier_message(images: &[(String, String)]) -> Option<serde_json::Value> {
    if images.is_empty() {
        return None;
    }
    let mut parts = vec![serde_json::json!({
        "type": "text",
        "text": "Attached image(s) from tool result:",
    })];
    parts.extend(images.iter().map(|(mime, data)| {
        let url = data_url(mime, data);
        serde_json::json!({ "type": "image_url", "image_url": { "url": url } })
    }));
    Some(serde_json::json!({ "role": "user", "content": parts }))
}

/// Join a tool result's outputs into a single text blob for the OpenAI `content` field.
///
/// `carried` says the images are being sent alongside, which changes what the text owes the model:
/// a pointer to something it is about to receive, rather than a marker standing in for something it
/// never will. A result with text of its own needs neither.
fn tool_result_text(content: &[luxe_core::ToolOutput], carried: bool) -> String {
    let text = luxe_core::ToolOutput::joined_text(content);
    if !text.is_empty() {
        return text;
    }
    let images = luxe_core::ToolOutput::images(content).next().is_some();
    match (images, carried) {
        (true, true) => "(see attached image)".to_string(),
        (true, false) => "[image]".to_string(),
        _ => text,
    }
}
