//! Wire — message expansion, SSE events and images.

use super::*;

#[test]
fn tool_message_expands_with_tool_call_id() {
    let msg = Message {
        role: Role::Tool,
        content: vec![
            ContentBlock::ToolResult {
                call_id: "call_1".into(),
                content: vec![ToolOutput::Text("result A".into())],
                is_error: false,
            },
            ContentBlock::ToolResult {
                call_id: "call_2".into(),
                content: vec![ToolOutput::Text("result B".into())],
                is_error: false,
            },
        ],
    };
    let mut out = Vec::new();
    push_wire_messages(&msg, &mut out, &HashMap::new(), false);
    // One tool message per result block, each carrying its own tool_call_id.
    assert_eq!(out.len(), 2);
    assert_eq!(out[0]["role"], "tool");
    assert_eq!(out[0]["tool_call_id"], "call_1");
    assert_eq!(out[0]["content"], "result A");
    assert_eq!(out[1]["tool_call_id"], "call_2");
    assert_eq!(out[1]["content"], "result B");
}

#[test]
fn reasoning_blocks_are_not_replayed_as_assistant_text() {
    let msg = Message {
        role: Role::Assistant,
        content: vec![
            ContentBlock::Thinking {
                text: "secret reasoning".into(),
                signature: None,
                id: None,
                encrypted: None,
                opaque: None,
            },
            ContentBlock::RedactedThinking { data: "ENC==".into() },
            ContentBlock::Text("the answer".into()),
        ],
    };
    let mut out = Vec::new();
    push_wire_messages(&msg, &mut out, &HashMap::new(), false);
    assert_eq!(out.len(), 1);
    let content = out[0]["content"].as_str().unwrap_or_default();
    assert_eq!(content, "the answer", "only the answer text is replayed");
    assert!(!content.contains("secret reasoning"), "reasoning is never inlined as text");
    assert!(!content.contains("thinking"), "no <thinking> wrapper leaks into the wire message");
}

#[test]
fn assistant_tool_call_only_sends_null_content() {
    let msg = Message {
        role: Role::Assistant,
        content: vec![ContentBlock::ToolCall {
            id: "call_1".into(),
            name: "echo".into(),
            arguments: serde_json::json!({"x": 1}),
        }],
    };
    let mut out = Vec::new();
    push_wire_messages(&msg, &mut out, &HashMap::new(), false);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["role"], "assistant");
    assert!(out[0]["content"].is_null(), "tool-call-only assistant must send null content");
    assert_eq!(out[0]["tool_calls"][0]["id"], "call_1");
    assert_eq!(out[0]["tool_calls"][0]["function"]["name"], "echo");
    // Arguments are serialized as a JSON string, per the OpenAI schema.
    assert!(out[0]["tool_calls"][0]["function"]["arguments"].is_string());
}

#[test]
fn assistant_text_with_tool_call_keeps_text() {
    let msg = Message {
        role: Role::Assistant,
        content: vec![
            ContentBlock::Text("let me check".into()),
            ContentBlock::ToolCall {
                id: "c1".into(),
                name: "echo".into(),
                arguments: serde_json::json!({}),
            },
        ],
    };
    let mut out = Vec::new();
    push_wire_messages(&msg, &mut out, &HashMap::new(), false);
    assert_eq!(out[0]["content"], "let me check");
    assert!(out[0]["tool_calls"].is_array());
}

#[test]
fn parse_sse_event_text_delta() {
    let sse_json = r#"{"choices":[{"index":0,"delta":{"role":"assistant","content":"Hello"},"finish_reason":null}],"usage":null}"#;
    let event: SseEvent = serde_json::from_str(sse_json).unwrap();

    let mut events = Vec::new();
    push_sse_events(&event, &mut events);

    assert_eq!(events.len(), 2); // TextStart + TextDelta
    assert!(matches!(&events[0], ProviderEvent::TextStart { index: 0 }));
    assert!(matches!(&events[1], ProviderEvent::TextDelta { index: 0, delta } if delta == "Hello"));
}

#[test]
fn parse_sse_event_tool_call() {
    let sse_json = r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"read","arguments":"{\"path\":\"test.rs\"}"}}]},"finish_reason":null}],"usage":null}"#;
    let event: SseEvent = serde_json::from_str(sse_json).unwrap();

    let mut events = Vec::new();
    push_sse_events(&event, &mut events);

    assert_eq!(events.len(), 2); // ToolCallStart + ToolCallDelta (no ToolCallEnd without finish_reason)
    assert!(matches!(&events[0], ProviderEvent::ToolCallStart { id, .. } if id == "call_1"));
    assert!(
        matches!(&events[1], ProviderEvent::ToolCallDelta { json_delta, .. } if json_delta.contains("test.rs"))
    );
}

#[test]
fn parse_sse_event_done() {
    let sse_json = r#"{"choices":[{"index":0,"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}"#;
    let event: SseEvent = serde_json::from_str(sse_json).unwrap();

    let mut events = Vec::new();
    push_sse_events(&event, &mut events);

    assert_eq!(events.len(), 3); // Usage + TextEnd + Done (usage precedes Done)
    assert!(
        matches!(&events[0], ProviderEvent::Usage(u) if u.prompt_tokens == 10 && u.completion_tokens == 5 && u.total_tokens == 15)
    );
    assert!(matches!(&events[1], ProviderEvent::TextEnd { index: 0 }));
    assert!(matches!(&events[2], ProviderEvent::Done { reason: StopReason::EndTurn }));
}

#[test]
fn parse_sse_usage_captures_cache_and_reasoning_details() {
    let sse_json = r#"{"choices":[],"usage":{"prompt_tokens":1000,"completion_tokens":200,"total_tokens":1200,"prompt_tokens_details":{"cached_tokens":768},"completion_tokens_details":{"reasoning_tokens":150}}}"#;
    let event: SseEvent = serde_json::from_str(sse_json).unwrap();
    let mut events = Vec::new();
    push_sse_events(&event, &mut events);
    let usage = events.iter().find_map(|e| match e {
        ProviderEvent::Usage(u) => Some(u),
        _ => None,
    });
    let usage = usage.expect("a Usage event");
    assert_eq!(usage.prompt_tokens, 1000); // cache-inclusive
    assert_eq!(usage.cache_read_tokens, 768);
    assert_eq!(usage.reasoning_tokens, 150);
}

#[test]
fn parse_sse_event_error() {
    let sse_json = r#"{"error":{"message":"Rate limit exceeded","type":"rate_limit_exceeded"}}"#;
    let event: SseEvent = serde_json::from_str(sse_json).unwrap();

    let mut events = Vec::new();
    push_sse_events(&event, &mut events);

    assert_eq!(events.len(), 1);
    assert!(
        matches!(&events[0], ProviderEvent::Error { message, .. } if message == "Rate limit exceeded")
    );
}

#[test]
fn parse_sse_event_empty_choices() {
    let sse_json = r#"{"choices":[]}"#;
    let event: SseEvent = serde_json::from_str(sse_json).unwrap();

    let mut events = Vec::new();
    push_sse_events(&event, &mut events);

    assert!(events.is_empty());
}

#[test]
fn parse_multiline_sse_fixture() {
    let events = decode_records(&[
        r#"data: {"choices":[{"index":0,"delta":{"role":"assistant"},"finish_reason":null}]}"#,
        r#"data: {"choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":null}]}"#,
        r#"data: {"choices":[{"index":0,"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}"#,
        "data: [DONE]",
    ]);

    assert_eq!(events.len(), 5, "{events:?}");
    assert!(matches!(&events[0], ProviderEvent::TextStart { index: 0 }));
    assert!(matches!(&events[1], ProviderEvent::TextDelta { index: 0, delta } if delta == "Hello"));
    assert!(matches!(&events[2], ProviderEvent::Usage(u) if u.total_tokens == 15));
    assert!(matches!(&events[3], ProviderEvent::TextEnd { index: 0 }));
    // One terminal `Done`, after the usage: the `[DONE]` sentinel releases the one the
    // finish_reason chunk produced instead of adding a second.
    assert!(matches!(&events[4], ProviderEvent::Done { reason: StopReason::EndTurn }));
}

/// A vision model that declares it takes images.
pub(super) fn seeing(id: &str) -> ModelInfo {
    ModelInfo {
        r#ref: ModelRef::new("lmstudio", id),
        input: vec![luxe_core::provider::Modality::Text, luxe_core::provider::Modality::Image],
        ..Default::default()
    }
}

/// One tool result carrying pixels, as `read` on an image or `screenshot` produces.
pub(super) fn tool_result_with_image(call_id: &str, text: &str) -> Message {
    Message {
        role: luxe_core::message::Role::Tool,
        content: vec![ContentBlock::ToolResult {
            call_id: call_id.into(),
            content: vec![
                luxe_core::ToolOutput::Text(text.into()),
                luxe_core::ToolOutput::Image {
                    data: "QUJD".into(),
                    mime: "image/png".into(),
                    name: Some("quad.png".into()),
                },
            ],
            is_error: false,
        }],
    }
}

pub(super) fn request(model: ModelInfo, messages: Vec<Message>) -> CompletionRequest {
    CompletionRequest {
        model,
        messages,
        system: None,
        tools: vec![],
        thinking: ThinkingLevel::Off,
        max_tokens: Some(4096),
        ..Default::default()
    }
}

/// The whole point of the run grouping: this schema has no image content on a `tool` message,
/// so the pixels ride on a following `user` message — and that message may never land between
/// two `tool` messages, which every provider rejects.
#[test]
fn a_tool_produced_image_reaches_a_vision_model_after_its_tool_message() {
    let body = provider(CompatFlags::default()).build_request_body(&request(
        seeing("qwen3.8-27b"),
        vec![Message::user("look at it"), tool_result_with_image("call-1", "quad.png: PNG")],
    ));
    let messages = body["messages"].as_array().expect("messages");

    let roles: Vec<&str> = messages.iter().filter_map(|m| m["role"].as_str()).collect();
    assert_eq!(roles, ["user", "tool", "user"], "the carrier follows the tool message");

    let carrier = messages.last().expect("carrier");
    let parts = carrier["content"].as_array().expect("the carrier is a content array");
    assert_eq!(
        parts[0]["type"], "text",
        "a text part leads: some servers reject an array without one"
    );
    assert_eq!(parts[1]["type"], "image_url");
    assert_eq!(parts[1]["image_url"]["url"], "data:image/png;base64,QUJD");
    // The tool message keeps its own text and never carries the bytes.
    assert_eq!(messages[1]["content"], "quad.png: PNG");
}

/// Sending base64 to a text-only endpoint is a large silent bill and, on a strict server, a
/// 400. The declared modality is the gate, and nothing else.
#[test]
fn a_text_only_model_is_never_sent_the_bytes() {
    let body = provider(CompatFlags::default()).build_request_body(&request(
        ModelInfo::default(),
        vec![Message::user("look at it"), tool_result_with_image("call-1", "quad.png: PNG")],
    ));
    let rendered = body.to_string();

    assert!(!rendered.contains("QUJD"), "no base64 anywhere in the body: {rendered}");
    assert!(!rendered.contains("image_url"), "and no carrier message: {rendered}");
    let roles: Vec<&str> =
        body["messages"].as_array().unwrap().iter().filter_map(|m| m["role"].as_str()).collect();
    assert_eq!(roles, ["user", "tool"]);
}

/// Luxe commits refusals and results as separate `Role::Tool` messages, and `tool_pairing` can
/// synthesize a third. One carrier for the whole run, after all of them — a carrier per message
/// would interleave a `user` turn between two `tool` turns and 400 the request.
#[test]
fn a_run_of_tool_messages_gets_exactly_one_carrier_after_all_of_them() {
    let body = provider(CompatFlags::default()).build_request_body(&request(
        seeing("qwen3.8-27b"),
        vec![
            Message::user("look"),
            tool_result_with_image("call-1", "first"),
            tool_result_with_image("call-2", "second"),
        ],
    ));
    let messages = body["messages"].as_array().expect("messages");
    let roles: Vec<&str> = messages.iter().filter_map(|m| m["role"].as_str()).collect();
    assert_eq!(roles, ["user", "tool", "tool", "user"], "never tool, user, tool");

    let parts = messages.last().unwrap()["content"].as_array().expect("array");
    let images = parts.iter().filter(|part| part["type"] == "image_url").count();
    assert_eq!(images, 2, "both results' pixels ride on the one carrier");
}

/// A result whose only output is an image would otherwise send empty content. It points at
/// what follows rather than shrugging — the model is about to receive it.
#[test]
fn an_image_only_result_points_at_what_is_attached() {
    let mut message = tool_result_with_image("call-1", "");
    if let ContentBlock::ToolResult { content, .. } = &mut message.content[0] {
        content.retain(|output| matches!(output, luxe_core::ToolOutput::Image { .. }));
    }
    let body = provider(CompatFlags::default())
        .build_request_body(&request(seeing("qwen3.8-27b"), vec![message.clone()]));
    assert_eq!(body["messages"][0]["content"], "(see attached image)");

    // With no way to send it, the marker stays a marker rather than a promise.
    let blind = provider(CompatFlags::default())
        .build_request_body(&request(ModelInfo::default(), vec![message]));
    assert_eq!(blind["messages"][0]["content"], "[image]");
}

/// One endpoint, two builds, two differently shaped requests — from one adapter.
///
/// This is the whole point of per-model compat, and it could not work before: adapters are built
/// once per provider, so a `thinking_format` written on a model reached nothing. The real setup
/// it is for is a single LM Studio port serving two Qwen builds whose chat templates disagree —
/// one handles a `developer` role, the other raises on it — where the provider-level format has
/// to be whatever the model you are actually using wants.
#[test]
fn two_models_on_one_adapter_are_shaped_by_their_own_compat() {
    // The endpoint's flags: OpenAI-style effort, and the newer role spelling.
    let endpoint = CompatFlags {
        supports_reasoning_effort: true,
        thinking_format: Some("openai".into()),
        supports_developer_role: true,
        ..Default::default()
    };
    let adapter = provider(endpoint.clone());
    let reasoning_model = |id: &str, compat: Option<CompatFlags>| ModelInfo {
        r#ref: ModelRef::new("lmstudio", id),
        reasoning: true,
        compat,
        ..Default::default()
    };
    let ask = |model: ModelInfo| {
        adapter.build_request_body(&CompletionRequest {
            model,
            messages: vec![Message::user("hi")],
            system: Some("be brief".into()),
            tools: vec![],
            thinking: ThinkingLevel::High,
            max_tokens: Some(64),
            ..Default::default()
        })
    };

    // The model with no opinion runs on the endpoint's flags, exactly as before.
    let plain = ask(reasoning_model("qwen3.8-27b", None));
    assert_eq!(plain["messages"][0]["role"], "developer");
    assert_eq!(plain["reasoning_effort"], "high");
    assert_eq!(plain["messages"][0]["content"], "be brief");

    // The build with its own flags is shaped differently on the same adapter: its template takes
    // thinking as a template argument and raises on a `developer` role.
    let fable = reasoning_model(
        "qwen3.6-27b-fable-fusion",
        Some(CompatFlags {
            thinking_format: Some("chat_template".into()),
            supports_developer_role: false,
            ..endpoint.clone()
        }),
    );
    let templated = ask(fable);
    assert_eq!(templated["messages"][0]["role"], "system");
    assert!(templated.get("reasoning_effort").is_none(), "{templated}");
    assert_eq!(templated["chat_template_kwargs"]["enable_thinking"], true);

    // And the adapter is unchanged by either request — the next model resolves for itself.
    let again = ask(reasoning_model("qwen3.8-27b", None));
    assert_eq!(again["messages"][0]["role"], "developer");
    assert_eq!(again["reasoning_effort"], "high");
}

/// The other two flags a model can carry through the same path, each with a wire consequence.
#[test]
fn a_models_compat_decides_the_usage_opt_in_and_the_max_tokens_field() {
    let strict = CompatFlags {
        supports_usage_in_streaming: false,
        max_tokens_field: "max_completion_tokens".into(),
        ..Default::default()
    };
    let adapter = provider(CompatFlags::default());
    let model = ModelInfo {
        r#ref: ModelRef::new("proxy", "strict"),
        compat: Some(strict),
        ..<_>::default()
    };

    let body = adapter.build_request_body(&thinking_request(model, ThinkingLevel::Off));
    assert!(body.get("stream_options").is_none(), "this model's server rejects the field");
    assert_eq!(body["max_completion_tokens"], 4096);
    assert!(body.get("max_tokens").is_none());

    // The adapter's own flags still answer for every other model on the endpoint.
    let sibling =
        adapter.build_request_body(&thinking_request(ModelInfo::default(), ThinkingLevel::Off));
    assert_eq!(sibling["stream_options"]["include_usage"], true);
    assert_eq!(sibling["max_tokens"], 4096);
}

/// A tool result carries the *name* of the tool where the endpoint requires it.
///
/// Not decoration: `openai/harmony` — the format gpt-oss speaks — renders a tool message from
/// its author's name and bails with `Tools should have a name!` when there is none, which a
/// server reports as a 400. OpenAI itself ignores the field, so it stays opt-in.
///
/// The name has to be *found*: a result carries the id of the call it answers and nothing else,
/// so it is read off the assistant turn that made the call, in the same request. That the call
/// is there is guaranteed rather than hoped for — a call and its result are one unit, and the
/// outgoing copy is repaired before it is built.
#[test]
fn a_tool_result_names_its_tool_where_the_endpoint_needs_it() {
    let call = |id: &str, name: &str| ContentBlock::ToolCall {
        id: id.to_string(),
        name: name.to_string(),
        arguments: serde_json::json!({}),
    };
    let result = |id: &str, text: &str| ContentBlock::ToolResult {
        call_id: id.to_string(),
        content: vec![luxe_core::ToolOutput::Text(text.to_string())],
        is_error: false,
    };
    let conversation = vec![
        Message::user("read both files"),
        Message {
            role: luxe_core::message::Role::Assistant,
            content: vec![call("call_1", "read"), call("call_2", "grep")],
        },
        Message {
            role: luxe_core::message::Role::Tool,
            // The third answers a call this request does not carry — a repaired half.
            content: vec![
                result("call_1", "fn main"),
                result("call_2", "3 matches"),
                result("call_orphan", "?"),
            ],
        },
    ];
    let request = |compat: Option<CompatFlags>| CompletionRequest {
        model: ModelInfo { compat, ..Default::default() },
        messages: conversation.clone(),
        system: None,
        tools: vec![],
        thinking: ThinkingLevel::Off,
        max_tokens: Some(64),
        ..Default::default()
    };
    let tool_messages = |body: &serde_json::Value| -> Vec<serde_json::Value> {
        body["messages"]
            .as_array()
            .expect("messages")
            .iter()
            .filter(|message| message["role"] == "tool")
            .cloned()
            .collect()
    };

    // Off by default: OpenAI has no such field, and a strict server rejects what it does not
    // model, so nothing is sent that was not asked for.
    let plain = provider(CompatFlags::default()).build_request_body(&request(None));
    for message in tool_messages(&plain) {
        assert!(message.get("name").is_none(), "{message}");
    }

    // On, from the endpoint's own flags.
    let flags = CompatFlags { requires_tool_result_name: true, ..Default::default() };
    let named = provider(flags.clone()).build_request_body(&request(None));
    let named = tool_messages(&named);
    assert_eq!(named[0]["name"], "read", "each result names the call it answers");
    assert_eq!(named[0]["tool_call_id"], "call_1");
    assert_eq!(named[1]["name"], "grep");
    assert_eq!(named[1]["tool_call_id"], "call_2");
    assert!(
        named[2].get("name").is_none(),
        "a result with no call in this request gets no name rather than a guessed one: {}",
        named[2]
    );

    // And on for one model on an endpoint whose other models do not want it — the same path
    // every other flag takes.
    let per_model = provider(CompatFlags::default()).build_request_body(&request(Some(flags)));
    assert_eq!(tool_messages(&per_model)[0]["name"], "read");
}

/// A compat flag that parses, resolves and displays but changes nothing is worse than no flag:
/// it reads back whatever you set, so the setting looks honored while the wire says otherwise.
#[test]
fn the_developer_role_flag_reaches_the_wire() {
    let request = CompletionRequest {
        model: ModelInfo::default(),
        messages: vec![Message::user("hi")],
        system: Some("be brief".into()),
        tools: vec![],
        thinking: ThinkingLevel::Off,
        max_tokens: Some(64),
        ..Default::default()
    };

    let default = provider(CompatFlags::default()).build_request_body(&request);
    assert_eq!(default["messages"][0]["role"], "system", "unset stays the classic spelling");

    let flags = CompatFlags { supports_developer_role: true, ..Default::default() };
    let opted_in = provider(flags).build_request_body(&request);
    assert_eq!(opted_in["messages"][0]["role"], "developer");
    assert_eq!(opted_in["messages"][0]["content"], "be brief", "and carries the same prompt");
}
