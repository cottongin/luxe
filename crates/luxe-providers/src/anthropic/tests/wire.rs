//! Wire — the error map, thinking levels, cache control and message serialization.

use super::*;

#[test]
fn test_map_error_to_provider_event() {
    let error = AnthropicError {
        message: None,
        error: Some(AnthropicErrorInner {
            message: Some("Authentication failed".into()),
            r#type: None,
        }),
    };
    match map_anthropic_event(AnthropicSseEvent::Error(error)) {
        Some(ProviderEvent::Error { message, .. }) => assert_eq!(message, "Authentication failed"),
        _ => panic!("Expected Error event"),
    }
}

#[test]
fn test_thinking_level_mapping() {
    use luxe_core::provider::ThinkingLevel;

    // Test Off level
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
    assert_eq!(body["thinking"]["type"], "disabled");

    // Test Minimal level
    let req = CompletionRequest {
        model: ModelInfo::default(),
        messages: vec![Message::user("Hello")],
        system: None,
        tools: vec![],
        max_tokens: Some(100),
        thinking: ThinkingLevel::Minimal,
        ..Default::default()
    };
    let body = build_request(&req, &model_info, &compat);
    assert!(body.get("thinking").is_some());
    assert_eq!(body["thinking"]["type"], "enabled");

    // Test Low level
    let req = CompletionRequest {
        model: ModelInfo::default(),
        messages: vec![Message::user("Hello")],
        system: None,
        tools: vec![],
        max_tokens: Some(100),
        thinking: ThinkingLevel::Low,
        ..Default::default()
    };
    let body = build_request(&req, &model_info, &compat);
    assert_eq!(body["thinking"]["type"], "enabled");

    // Test Medium level
    let req = CompletionRequest {
        model: ModelInfo::default(),
        messages: vec![Message::user("Hello")],
        system: None,
        tools: vec![],
        max_tokens: Some(100),
        thinking: ThinkingLevel::Medium,
        ..Default::default()
    };
    let body = build_request(&req, &model_info, &compat);
    assert_eq!(body["thinking"]["type"], "enabled");

    // Test High level
    let req = CompletionRequest {
        model: ModelInfo::default(),
        messages: vec![Message::user("Hello")],
        system: None,
        tools: vec![],
        max_tokens: Some(100),
        thinking: ThinkingLevel::High,
        ..Default::default()
    };
    let body = build_request(&req, &model_info, &compat);
    assert_eq!(body["thinking"]["type"], "enabled");

    // Top-end levels fall back to the highest standard budget unless a model maps them.
    for thinking in [ThinkingLevel::XHigh, ThinkingLevel::Ultra] {
        let req = CompletionRequest {
            model: ModelInfo::default(),
            messages: vec![Message::user("Hello")],
            system: None,
            tools: vec![],
            max_tokens: Some(64_000),
            thinking,
            ..Default::default()
        };
        let body = build_request(&req, &model_info, &compat);
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["thinking"]["budget_tokens"], 16_384);
    }
}

#[test]
fn test_system_prompt_cache_control() {
    let req = CompletionRequest {
        model: ModelInfo::default(),
        messages: vec![
            Message {
                role: luxe_core::message::Role::System,
                content: vec![luxe_core::message::ContentBlock::Text(
                    "You are a helpful assistant".to_string(),
                )],
            },
            Message::user("Hello"),
        ],
        system: None,
        tools: vec![],
        max_tokens: Some(100),
        thinking: ThinkingLevel::Off,
        ..Default::default()
    };
    let model_info = ModelInfo::default();
    let compat = AnthropicCompat::default();

    let body = build_request(&req, &model_info, &compat);

    // System prompt should be an array with cache_control marker
    assert!(body["system"].is_array(), "System should be an array, got: {:?}", body["system"]);
    assert_eq!(body["system"][0]["type"], "text");
    assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
}

#[test]
fn test_tool_cache_control() {
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

    // Tools should have cache_control on the last tool
    assert!(body["tools"].is_array());
    let tools = body["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 1);
    // Last tool should have cache_control
    assert_eq!(tools[0]["cache_control"]["type"], "ephemeral");
}

#[test]
fn test_empty_thinking_signature_allowed() {
    let req = CompletionRequest {
        model: ModelInfo::default(),
        messages: vec![Message::assistant("Let me think...")],
        system: None,
        tools: vec![],
        max_tokens: Some(100),
        thinking: ThinkingLevel::High,
        ..Default::default()
    };
    let model_info = ModelInfo::default();
    let compat = AnthropicCompat::default();

    let body = build_request(&req, &model_info, &compat);

    // Should succeed even with empty thinking signature
    assert!(body["thinking"].is_object());
}

#[test]
fn test_multiple_tool_calls() {
    let req = CompletionRequest {
        model: ModelInfo::default(),
        messages: vec![Message::assistant("I'll help with that")],
        system: None,
        tools: vec![
            luxe_core::provider::ToolSchema {
                name: "bash".to_string(),
                description: "Run a bash command".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "command": {"type": "string"}
                    }
                }),
            },
            luxe_core::provider::ToolSchema {
                name: "read_file".to_string(),
                description: "Read a file".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"}
                    }
                }),
            },
        ],
        max_tokens: Some(100),
        thinking: ThinkingLevel::Off,
        ..Default::default()
    };
    let model_info = ModelInfo::default();
    let compat = AnthropicCompat::default();

    let body = build_request(&req, &model_info, &compat);

    // Both tools should be present
    let tools = body["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 2);

    // Last tool should have cache_control
    assert_eq!(tools[1]["cache_control"]["type"], "ephemeral");

    // First tool should not have cache_control
    assert!(!tools[0].get("cache_control").is_some());
}

#[test]
fn test_tool_result_with_text() {
    let msg = Message::tool_result("call_123", "Command executed successfully", false);

    let serialized = serialize_messages(&[msg], false);

    assert_eq!(serialized.len(), 1);
    assert_eq!(serialized[0]["role"], "user");
    // Tool results are serialized as an array with tool_result object
    assert!(serialized[0]["content"].is_array());

    let content = serialized[0]["content"].as_array().unwrap();
    assert_eq!(content.len(), 1);
    assert_eq!(content[0]["type"], "tool_result");
    assert_eq!(content[0]["tool_use_id"], "call_123");
    assert_eq!(content[0]["content"], "Command executed successfully");
    assert_eq!(content[0]["is_error"], false);
}

#[test]
fn test_tool_result_with_error() {
    let msg = Message::tool_result("call_456", "Error: command failed", true);

    let serialized = serialize_messages(&[msg], false);

    assert_eq!(serialized.len(), 1);
    let content = serialized[0]["content"].as_array().unwrap();
    assert_eq!(content[0]["is_error"], true);
    assert_eq!(content[0]["content"], "Error: command failed");
}

#[test]
fn test_cache_control_with_tools_and_system() {
    let req = CompletionRequest {
        model: ModelInfo::default(),
        messages: vec![
            Message {
                role: luxe_core::message::Role::System,
                content: vec![luxe_core::message::ContentBlock::Text(
                    "You are a helpful assistant".to_string(),
                )],
            },
            Message::user("Hello"),
        ],
        system: None,
        tools: vec![luxe_core::provider::ToolSchema {
            name: "bash".to_string(),
            description: "Run a bash command".to_string(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        }],
        max_tokens: Some(100),
        thinking: ThinkingLevel::Off,
        ..Default::default()
    };
    let model_info = ModelInfo::default();
    let compat = AnthropicCompat::default();

    let body = build_request(&req, &model_info, &compat);

    // System should have cache_control
    assert!(body["system"].is_array());
    assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");

    // Last tool should have cache_control
    let tools = body["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["cache_control"]["type"], "ephemeral");
}

#[test]
fn test_thinking_block_with_signature() {
    let msg = Message {
        role: luxe_core::message::Role::Assistant,
        content: vec![luxe_core::message::ContentBlock::Thinking {
            text: "Let me think about this...".to_string(),
            signature: Some("sig123".to_string()),
            id: None,
            encrypted: None,
            opaque: None,
        }],
    };

    let serialized = serialize_messages(&[msg], false);

    assert_eq!(serialized.len(), 1);
    let content = serialized[0]["content"].as_array().unwrap();
    assert_eq!(content[0]["type"], "thinking");
    assert_eq!(content[0]["thinking"], "Let me think about this...");
    assert_eq!(content[0]["signature"], "sig123");
}

#[test]
fn test_thinking_block_without_signature_is_dropped() {
    // A signature-less reasoning block can't round-trip to Anthropic (it would 400 on an
    // invalid/empty signature), so it's dropped; the rest of the message survives.
    let msg = Message {
        role: luxe_core::message::Role::Assistant,
        content: vec![
            luxe_core::message::ContentBlock::Thinking {
                text: "Let me think about this...".to_string(),
                signature: None,
                id: None,
                encrypted: None,
                opaque: None,
            },
            luxe_core::message::ContentBlock::Text("the answer".to_string()),
        ],
    };

    let serialized = serialize_messages(&[msg], false);

    assert_eq!(serialized.len(), 1);
    let content = serialized[0]["content"].as_array().unwrap();
    assert_eq!(content.len(), 1, "the unsigned thinking block is omitted");
    assert_eq!(content[0]["type"], "text");
    assert_eq!(content[0]["text"], "the answer");
}

#[test]
fn test_tool_call_serialization() {
    let msg = Message {
        role: luxe_core::message::Role::Assistant,
        content: vec![luxe_core::message::ContentBlock::ToolCall {
            id: "call_abc".to_string(),
            name: "bash".to_string(),
            arguments: serde_json::json!({"command": "ls -la"}),
        }],
    };

    let serialized = serialize_messages(&[msg], false);

    assert_eq!(serialized.len(), 1);
    let content = serialized[0]["content"].as_array().unwrap();
    assert_eq!(content[0]["type"], "tool_use");
    assert_eq!(content[0]["id"], "call_abc");
    assert_eq!(content[0]["name"], "bash");
    assert_eq!(content[0]["input"]["command"], "ls -la");
}

#[test]
fn test_mixed_content_blocks() {
    let msg = Message {
        role: luxe_core::message::Role::Assistant,
        content: vec![
            luxe_core::message::ContentBlock::Thinking {
                text: "Let me think...".to_string(),
                signature: Some("sig1".to_string()),
                id: None,
                encrypted: None,
                opaque: None,
            },
            luxe_core::message::ContentBlock::Text("I'll help you.".to_string()),
        ],
    };

    let serialized = serialize_messages(&[msg], false);

    assert_eq!(serialized.len(), 1);
    let content = serialized[0]["content"].as_array().unwrap();
    assert_eq!(content.len(), 2);

    // First block should be thinking
    assert_eq!(content[0]["type"], "thinking");
    assert_eq!(content[0]["thinking"], "Let me think...");

    // Second block should be text
    assert_eq!(content[1]["type"], "text");
    assert_eq!(content[1]["text"], "I'll help you.");
}

#[test]
fn test_anthropic_compat_default() {
    let compat = AnthropicCompat::default();
    assert!(!compat.force_adaptive_thinking);
    assert!(compat.cache_control, "prompt caching is on unless a config turns it off");
}

/// A model's own flags decide how *its* requests are shaped, on the adapter every Claude shares.
///
/// The two this API has an opinion about: how thinking is asked for, and whether cache markers
/// go out. Both used to be fixed when the adapter was built, so a model whose generation
/// disagreed with its siblings could only be corrected by changing the provider — for all of
/// them. The projection is per request now, and reads through the same two-field conversion the
/// build-time path uses, so neither can drift.
#[test]
fn a_models_compat_shapes_its_own_request_on_a_shared_adapter() {
    let provider = AnthropicProvider::new(
        crate::config::ApiKey::default(),
        None,
        ModelInfo::default(),
        AnthropicCompat::default(),
    );
    let with_flags = |flags: luxe_core::provider::CompatFlags| ModelInfo {
        r#ref: luxe_core::provider::ModelRef::new("anthropic", "claude-x"),
        reasoning: true,
        compat: Some(flags),
        ..Default::default()
    };

    // Adaptive thinking forced for this model alone.
    let adaptive = with_flags(luxe_core::provider::CompatFlags {
        force_adaptive_thinking: true,
        cache_control_format: Some("anthropic".into()),
        ..Default::default()
    });
    let compat = provider.compat_for(&adaptive);
    assert!(compat.force_adaptive_thinking);
    assert!(compat.cache_control);
    let body = build_request(&turn_request(&adaptive, ThinkingLevel::High), &adaptive, &compat);
    assert_eq!(body["thinking"]["type"], "adaptive");
    assert_eq!(body["output_config"]["effort"], "high");
    assert!(body["messages"][0]["content"][0].get("cache_control").is_some());

    // Prompt caching refused for this one — a `null` format is how that is said.
    let uncached = with_flags(luxe_core::provider::CompatFlags::default());
    let compat = provider.compat_for(&uncached);
    assert!(!compat.cache_control);
    assert!(!compat.force_adaptive_thinking);
    let body = build_request(&turn_request(&uncached, ThinkingLevel::High), &uncached, &compat);
    assert!(body["messages"][0].get("cache_control").is_none());
    assert!(body["messages"][0]["content"][0].get("cache_control").is_none());

    // And a model with nothing of its own keeps the adapter's flags, caching included.
    let plain = ModelInfo { reasoning: true, ..Default::default() };
    assert_eq!(provider.compat_for(&plain), AnthropicCompat::default());
}

#[test]
fn test_multi_turn_conversation() {
    let messages = vec![
        Message::user("Hello"),
        Message::assistant("Hi there!"),
        Message::user("How are you?"),
    ];

    let serialized = serialize_messages(&messages, false);

    assert_eq!(serialized.len(), 3);
    assert_eq!(serialized[0]["role"], "user");
    assert_eq!(serialized[0]["content"][0]["text"], "Hello");

    assert_eq!(serialized[1]["role"], "assistant");
    assert_eq!(serialized[1]["content"][0]["text"], "Hi there!");

    assert_eq!(serialized[2]["role"], "user");
    assert_eq!(serialized[2]["content"][0]["text"], "How are you?");
}

#[test]
fn test_full_request_building() {
    let req = CompletionRequest {
        model: ModelInfo::default(),
        messages: vec![
            Message {
                role: luxe_core::message::Role::System,
                content: vec![luxe_core::message::ContentBlock::Text(
                    "You are helpful".to_string(),
                )],
            },
            Message::user("Hello"),
        ],
        system: None,
        tools: vec![luxe_core::provider::ToolSchema {
            name: "bash".to_string(),
            description: "Run bash".to_string(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        }],
        max_tokens: Some(2000),
        thinking: ThinkingLevel::High,
        ..Default::default()
    };
    let model_info = ModelInfo::default();
    let compat = AnthropicCompat::default();

    let body = build_request(&req, &model_info, &compat);

    // Should have all fields
    assert_eq!(body["max_tokens"], 2000);
    assert!(body["thinking"].is_object());
    assert!(body["tools"].is_array());
    assert!(body["system"].is_array());
    assert!(body["messages"].is_array());
}

#[test]
fn test_sse_parse_tool_call_events() {
    let sse_text = r#"event: content_block_start
data: {"type": "content_block_start", "content_block": {"type": "tool_use", "id": "toolu_123", "name": "bash"}}

event: content_block_delta
data: {"type": "content_block_delta", "delta": {"type": "input_json_delta", "partial_json": "{\"command\":"}}

event: content_block_delta
data: {"type": "content_block_delta", "delta": {"type": "input_json_delta", "partial_json": ": \"ls\"}"}}

event: content_block_stop
data: {}

event: message_stop
data: {}"#;

    let events = parse_anthropic_sse_text(sse_text);

    // Should have multiple events
    assert!(events.len() >= 4);

    // Verify we got tool call events
    let has_tool_start = events.iter().any(|e| matches!(e, ProviderEvent::ToolCallStart { .. }));
    let has_tool_delta = events.iter().any(|e| matches!(e, ProviderEvent::ToolCallDelta { .. }));
    let has_done = events.iter().any(|e| matches!(e, ProviderEvent::Done { .. }));

    assert!(has_tool_start, "Should have tool call start");
    assert!(has_tool_delta, "Should have tool call delta");
    assert!(has_done, "Should have done event");
}

#[test]
fn test_adaptive_thinking_effort() {
    let req = CompletionRequest {
        model: ModelInfo::default(),
        messages: vec![Message::user("Hello")],
        system: None,
        tools: vec![],
        max_tokens: Some(4000),
        thinking: ThinkingLevel::High,
        ..Default::default()
    };
    let model_info = ModelInfo::default();
    let mut compat = AnthropicCompat::default();
    compat.force_adaptive_thinking = true;

    let body = build_request(&req, &model_info, &compat);

    assert_eq!(body["thinking"]["type"], "adaptive");
    assert_eq!(body["output_config"]["effort"], "high");
    assert!(body["thinking"].get("budget_tokens").is_none());
}
