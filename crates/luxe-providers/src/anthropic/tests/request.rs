//! Request — the body the adapter builds, tools, images and cache markers.

use super::*;

#[test]
fn test_build_anthropic_request_basic() {
    let req = CompletionRequest {
        model: ModelInfo::default(),
        messages: vec![Message::user("Hello")],
        system: None,
        tools: vec![],
        max_tokens: Some(100),
        thinking: ThinkingLevel::Off,
        ..Default::default()
    };
    let model_info = ModelInfo::default();
    let compat = AnthropicCompat::default();

    let body = build_request(&req, &model_info, &compat);
    assert_eq!(body["max_tokens"], 100);
    assert_eq!(body["messages"][0]["role"], "user");
    assert_eq!(body["messages"][0]["content"][0]["type"], "text");
}

#[test]
fn test_build_anthropic_request_with_thinking() {
    let req = CompletionRequest {
        model: ModelInfo::default(),
        messages: vec![Message::user("Hello")],
        system: None,
        tools: vec![],
        max_tokens: Some(64_000),
        thinking: ThinkingLevel::High,
        ..Default::default()
    };
    let model_info = ModelInfo::default();
    let compat = AnthropicCompat::default();

    let body = build_request(&req, &model_info, &compat);
    assert_eq!(body["thinking"]["type"], "enabled");
    assert_eq!(body["thinking"]["budget_tokens"], 16_384);
}

#[test]
fn test_build_anthropic_request_with_tools() {
    let req = CompletionRequest {
        model: ModelInfo::default(),
        messages: vec![Message::user("Hello")],
        system: None,
        tools: vec![luxe_core::provider::ToolSchema {
            name: "bash".to_string(),
            description: "Run a bash command".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string"}
                }
            }),
        }],
        max_tokens: Some(100),
        thinking: ThinkingLevel::Off,
        ..Default::default()
    };
    let model_info = ModelInfo::default();
    let compat = AnthropicCompat::default();

    let body = build_request(&req, &model_info, &compat);
    assert_eq!(body["tools"].as_array().unwrap().len(), 1);
    assert_eq!(body["tools"][0]["name"], "bash");
}

/// Only `write` streams its arguments: unvalidated, Claude garbles `edit`'s.
#[test]
fn only_write_asks_for_its_arguments_as_they_are_written() {
    let schema = |name: &str| luxe_core::provider::ToolSchema {
        name: name.into(),
        description: String::new(),
        parameters: serde_json::json!({"type": "object"}),
    };
    let req = CompletionRequest {
        messages: vec![Message::user("hi")],
        tools: vec![schema("write"), schema("edit"), schema("bash")],
        max_tokens: Some(100),
        thinking: ThinkingLevel::Off,
        ..Default::default()
    };
    let eager = |compat: &AnthropicCompat| {
        let body = build_request(&req, &ModelInfo::default(), compat);
        body["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool.get("eager_input_streaming").is_some())
            .collect::<Vec<_>>()
    };
    assert_eq!(eager(&AnthropicCompat::default()), [true, false, false]);
    let off = AnthropicCompat { stream_tool_arguments: false, ..AnthropicCompat::default() };
    assert_eq!(eager(&off), [false, false, false]);
}

#[test]
fn test_serialize_messages_tool_result() {
    let msg = Message::tool_result("call_123", "output", false);
    let serialized = serialize_messages(&[msg], false);
    assert_eq!(serialized[0]["role"], "user");
    assert_eq!(serialized[0]["content"][0]["type"], "tool_result");
    assert_eq!(serialized[0]["content"][0]["tool_use_id"], "call_123");
}

/// A tool result carrying pixels, as `read` on an image or `screenshot` produces.
fn tool_result_with_image(text: &str) -> Message {
    Message {
        role: luxe_core::message::Role::Tool,
        content: vec![luxe_core::message::ContentBlock::ToolResult {
            call_id: "call_1".into(),
            content: vec![
                luxe_core::message::ToolOutput::Text(text.into()),
                luxe_core::message::ToolOutput::Image {
                    data: "QUJD".into(),
                    mime: "image/png".into(),
                    name: Some("quad.png".into()),
                },
            ],
            is_error: false,
        }],
    }
}

/// This API takes image blocks inside a `tool_result`, so the pixels ride in the result they
/// belong to — no carrier message, no restructuring.
#[test]
fn a_tool_result_image_becomes_a_content_block_for_a_vision_model() {
    let serialized = serialize_messages(&[tool_result_with_image("quad.png: PNG")], true);
    let result = &serialized[0]["content"][0];
    assert_eq!(result["type"], "tool_result");

    let parts = result["content"].as_array().expect("the block array form");
    assert_eq!(parts[0]["type"], "text");
    assert_eq!(parts[0]["text"], "quad.png: PNG");
    assert_eq!(parts[1]["type"], "image");
    assert_eq!(parts[1]["source"]["type"], "base64");
    assert_eq!(parts[1]["source"]["media_type"], "image/png");
    assert_eq!(parts[1]["source"]["data"], "QUJD");
}

/// The common shape is unchanged: a text-only result stays a plain string, which is what every
/// existing transcript and wire test reads. Churning it buys nothing.
#[test]
fn a_text_only_result_keeps_the_plain_string_form() {
    let serialized = serialize_messages(&[Message::tool_result("call_1", "done", false)], true);
    assert_eq!(serialized[0]["content"][0]["content"], "done");

    // And a model with no declared vision never receives the bytes, even when they exist.
    let blind = serialize_messages(&[tool_result_with_image("quad.png: PNG")], false);
    assert_eq!(blind[0]["content"][0]["content"], "quad.png: PNG");
    let rendered = serde_json::Value::Array(blind).to_string();
    assert!(!rendered.contains("QUJD"), "no base64 reaches a text-only model: {rendered}");
}

/// The default breakpoint marker, for the tests that are about placement rather than lifetime.
fn five_minute() -> serde_json::Value {
    serde_json::json!({"type": "ephemeral"})
}

/// The breakpoint ends where every session's prompt stops being the same, so a session in
/// another directory reads the harness rather than writing it, and the rest follows uncached.
#[test]
fn the_system_breakpoint_ends_the_shared_part() {
    let system = "## Role\n\nshared\n\n## Environment\n\n**cwd**: `/x`";
    let shared = system.find("## Environment").unwrap();
    let mut body = serde_json::json!({"system": system, "messages": []});
    add_cache_control_markers(&mut body, five_minute(), shared);
    assert_eq!(body["system"][0]["text"], "## Role\n\nshared\n\n");
    assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
    assert_eq!(body["system"][1]["text"], "## Environment\n\n**cwd**: `/x`");
    assert!(body["system"][1].get("cache_control").is_none());

    // Nothing shared, or nothing after it, is one block as before
    for shared in [0, system.len(), system.len() + 5] {
        let mut body = serde_json::json!({"system": system, "messages": []});
        add_cache_control_markers(&mut body, five_minute(), shared);
        assert_eq!(body["system"].as_array().unwrap().len(), 1, "{shared}");
        assert_eq!(body["system"][0]["text"], system);
    }
}

#[test]
fn test_cache_control_markers_system() {
    let mut body = serde_json::json!({
        "system": "You are a helpful assistant.",
        "messages": [],
    });
    add_cache_control_markers(&mut body, five_minute(), 0);

    // System should be converted to array with cache_control
    assert!(body["system"].is_array());
    assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
}

#[test]
fn test_cache_control_markers_tools() {
    let mut body = serde_json::json!({
        "tools": [
            {"name": "bash", "description": "Run bash"},
            {"name": "edit", "description": "Edit files"},
        ],
        "messages": [],
    });
    add_cache_control_markers(&mut body, five_minute(), 0);

    // Last tool should have cache_control
    assert_eq!(body["tools"][1]["cache_control"]["type"], "ephemeral");
}

#[test]
fn test_cache_control_marks_last_conversation_message() {
    // The key incremental-caching breakpoint: the last message is marked so the growing
    // conversation is cached, not re-sent uncached each turn. String content is normalized
    // to a block; array content marks its last block.
    let mut body = serde_json::json!({
        "system": "sys",
        "tools": [{"name": "bash", "description": "Run bash"}],
        "messages": [
            {"role": "user", "content": "do the thing"},
            {"role": "assistant", "content": [{"type": "text", "text": "ok"}]},
            {"role": "user", "content": [
                {"type": "tool_result", "content": "result A"},
                {"type": "tool_result", "content": "result B"},
            ]},
        ],
    });
    add_cache_control_markers(&mut body, five_minute(), 0);

    // The last message's last block is marked, which caches everything before it — and so is
    // the previous user turn, which is where the last request stopped.
    let last = &body["messages"][2]["content"];
    assert!(last[0].get("cache_control").is_none());
    assert_eq!(last[1]["cache_control"]["type"], "ephemeral");
    assert_eq!(body["messages"][0]["content"][0]["cache_control"]["type"], "ephemeral");
    // The assistant turn between them is not a breakpoint.
    assert!(body["messages"][1]["content"][0].get("cache_control").is_none());
}

#[test]
fn test_cache_control_normalizes_string_message_content() {
    let mut body = serde_json::json!({
        "messages": [{"role": "user", "content": "hello world"}],
    });
    add_cache_control_markers(&mut body, five_minute(), 0);
    let content = &body["messages"][0]["content"];
    assert!(content.is_array());
    assert_eq!(content[0]["text"], "hello world");
    assert_eq!(content[0]["cache_control"]["type"], "ephemeral");
}
