//! Request — the body and the input items the adapter builds.

use super::*;

#[test]
fn request_body_system_becomes_instructions() {
    let p = make_provider();
    let mut req = make_req("gpt-5");
    req.system = Some("You are a helpful assistant.".into());
    let body = p.build_request_body(&req);

    assert_eq!(body["instructions"], "You are a helpful assistant.");
}

#[test]
fn request_body_max_output_tokens_zero_omitted() {
    let p = make_provider();
    let mut req = make_req("gpt-5");
    req.max_tokens = Some(0);
    let body = p.build_request_body(&req);
    assert!(body.get("max_output_tokens").is_none());
}

#[test]
fn request_body_max_output_tokens_nonzero_included() {
    let p = make_provider();
    let mut req = make_req("gpt-5");
    req.max_tokens = Some(8192);
    let body = p.build_request_body(&req);
    assert_eq!(body["max_output_tokens"], 8192);
}

/// The Responses API is OpenAI-shaped too, so a model's recipe applies here as well — and it
/// is applied last, over `max_output_tokens` and everything else the adapter chose.
#[test]
fn a_models_sampling_recipe_reaches_the_responses_body_last() {
    let p = make_provider();
    let mut req = make_req("gpt-5");
    req.max_tokens = Some(8192);
    req.model.sampling_params = match serde_json::json!({
        "temperature": 1.0, "top_p": 0.95, "max_output_tokens": 4096, "stream": false,
    }) {
        serde_json::Value::Object(map) => map,
        _ => unreachable!("fixture is an object"),
    };
    let body = p.build_request_body(&req);

    assert_eq!(body["temperature"], 1.0);
    assert_eq!(body["top_p"], 0.95);
    assert_eq!(body["max_output_tokens"], 4096, "a named field the recipe sets is overridden");
    assert_eq!(body["stream"], true, "but not the ones that decide what request this is");
}

#[test]
fn request_body_tools_flat_not_nested() {
    let p = make_provider();
    let mut req = make_req("gpt-5");
    req.tools = vec![ToolSchema {
        name: "search".into(),
        description: "Search the web".into(),
        parameters: serde_json::json!({ "type": "object", "properties": {} }),
    }];
    let body = p.build_request_body(&req);

    let tools = body["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 1);
    let tool = &tools[0];
    // Flat layout: name/description at top level, NOT under "function".
    assert_eq!(tool["type"], "function");
    assert_eq!(tool["name"], "search");
    assert_eq!(tool["description"], "Search the web");
    assert!(tool.get("function").is_none(), "tools must NOT be nested under 'function'");
    assert_eq!(tool["strict"], false, "left out, the Responses API means true");
}

#[test]
fn request_body_reasoning_effort_for_reasoning_model() {
    use std::collections::HashMap;
    let p = make_provider();
    let mut req = make_req("gpt-5");
    req.model.reasoning = true;
    req.model.thinking_level_map = HashMap::from([
        ("low".to_string(), Some("low".to_string())),
        ("medium".to_string(), Some("medium".to_string())),
        ("high".to_string(), Some("high".to_string())),
    ]);
    req.thinking = ThinkingLevel::Medium;
    let body = p.build_request_body(&req);
    assert_eq!(body["reasoning"]["effort"], "medium");
    assert_eq!(body["reasoning"]["summary"], "auto");
    // Stateless reasoning replay: encrypted items, no server-side storage.
    assert_eq!(body["store"], false);
    assert_eq!(body["include"], serde_json::json!(["reasoning.encrypted_content"]));
}

#[test]
fn request_body_reasoning_model_off_still_enables_encrypted_replay() {
    // Even at thinking=off, a reasoning model can emit reasoning we must be able to round-trip,
    // so encrypted items + summary are requested (just without an explicit effort).
    let p = make_provider();
    let mut req = make_req("gpt-5");
    req.model.reasoning = true;
    req.thinking = ThinkingLevel::Off;
    let body = p.build_request_body(&req);
    assert_eq!(body["store"], false);
    assert_eq!(body["include"], serde_json::json!(["reasoning.encrypted_content"]));
    assert_eq!(body["reasoning"]["summary"], "auto");
    assert!(body["reasoning"].get("effort").is_none(), "no effort at thinking=off");
}

#[test]
fn request_body_non_reasoning_model_no_reasoning() {
    let p = make_provider();
    let mut req = make_req("gpt-5");
    req.model.reasoning = false;
    req.thinking = ThinkingLevel::High;
    let body = p.build_request_body(&req);
    assert!(body.get("reasoning").is_none());
}

// -----------------------------------------------------------------------
// Input item building
// -----------------------------------------------------------------------

#[test]
fn input_user_message() {
    let msgs = vec![Message::user("Hello")];
    let items = build_input_items(&msgs, false);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["role"], "user");
    assert_eq!(items[0]["content"], "Hello");
}

/// Unlike chat-completions, `function_call_output.output` is `string | Array<input_text |
/// input_image | input_file>`, so the image rides inside the result and nothing is interleaved.
#[test]
fn a_tool_result_image_rides_inside_its_own_output_for_a_vision_model() {
    let tool = Message {
        role: Role::Tool,
        content: vec![ContentBlock::ToolResult {
            call_id: "call_1".into(),
            content: vec![
                ToolOutput::Text("quad.png: PNG".into()),
                ToolOutput::Image {
                    data: "QUJD".into(),
                    mime: "image/png".into(),
                    name: Some("quad.png".into()),
                },
            ],
            is_error: false,
        }],
    };

    let items = build_input_items(std::slice::from_ref(&tool), true);
    assert_eq!(items.len(), 1, "no carrier item: this schema needs none");
    assert_eq!(items[0]["type"], "function_call_output");
    let parts = items[0]["output"].as_array().expect("the content-array form");
    assert_eq!(parts[0]["type"], "input_text");
    assert_eq!(parts[0]["text"], "quad.png: PNG");
    assert_eq!(parts[1]["type"], "input_image");
    assert_eq!(parts[1]["image_url"], "data:image/png;base64,QUJD");

    // Text-only model: the plain string, and not one byte of base64.
    let blind = build_input_items(&[tool], false);
    assert_eq!(blind[0]["output"], "quad.png: PNG");
    assert!(!blind[0].to_string().contains("QUJD"));
}

/// This path dropped user-attached images too — its `Role::User` arm only ever collected text,
/// so a `@screenshot.png` mention reached a Codex/ChatGPT session as nothing at all.
#[test]
fn a_user_attached_image_reaches_a_vision_model() {
    let msg = Message {
        role: Role::User,
        content: vec![
            ContentBlock::Text("what is this?".into()),
            ContentBlock::Image {
                data: "QUJD".into(),
                mime: "image/png".into(),
                name: Some("shot.png".into()),
            },
        ],
    };

    let items = build_input_items(std::slice::from_ref(&msg), true);
    let parts = items[0]["content"].as_array().expect("content parts");
    assert_eq!(parts[0]["type"], "input_text");
    assert_eq!(parts[0]["text"], "what is this?");
    assert_eq!(parts[1]["type"], "input_image");
    assert_eq!(parts[1]["image_url"], "data:image/png;base64,QUJD");

    // Text-only: the string form it has always used, with the text intact.
    let blind = build_input_items(&[msg], false);
    assert_eq!(blind[0]["content"], "what is this?");
}

#[test]
fn input_assistant_text_message() {
    let msg =
        Message { role: Role::Assistant, content: vec![ContentBlock::Text("Hi there".into())] };
    let items = build_input_items(&[msg], false);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["role"], "assistant");
    assert_eq!(items[0]["content"], "Hi there");
}

#[test]
fn input_replays_reasoning_item_before_its_function_call() {
    // A reasoning turn: reasoning (summary + id + encrypted) then a tool call, then its result.
    let assistant = Message {
        role: Role::Assistant,
        content: vec![
            ContentBlock::Thinking {
                text: "let me look".into(),
                signature: None,
                id: Some("rs_1".into()),
                encrypted: Some("ENC==".into()),
                opaque: None,
            },
            ContentBlock::ToolCall {
                id: "call_1".into(),
                name: "read".into(),
                arguments: serde_json::json!({"path": "x"}),
            },
        ],
    };
    let tool = Message {
        role: Role::Tool,
        content: vec![ContentBlock::ToolResult {
            call_id: "call_1".into(),
            content: vec![ToolOutput::Text("ok".into())],
            is_error: false,
        }],
    };
    let items = build_input_items(&[assistant, tool], false);
    // Order preserved: reasoning → function_call → function_call_output.
    assert_eq!(items[0]["type"], "reasoning");
    assert_eq!(items[0]["id"], "rs_1");
    assert_eq!(items[0]["encrypted_content"], "ENC==");
    assert_eq!(items[0]["summary"][0]["type"], "summary_text");
    assert_eq!(items[0]["summary"][0]["text"], "let me look");
    assert_eq!(items[1]["type"], "function_call");
    assert_eq!(items[1]["call_id"], "call_1");
    assert_eq!(items[2]["type"], "function_call_output");
    assert_eq!(items[2]["call_id"], "call_1");
}

#[test]
fn input_reasoning_without_summary_uses_empty_summary_array() {
    let msg = Message {
        role: Role::Assistant,
        content: vec![ContentBlock::Thinking {
            text: String::new(),
            signature: None,
            id: Some("rs_2".into()),
            encrypted: Some("ENC2==".into()),
            opaque: None,
        }],
    };
    let items = build_input_items(&[msg], false);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["type"], "reasoning");
    assert_eq!(items[0]["summary"], serde_json::json!([]));
    assert_eq!(items[0]["encrypted_content"], "ENC2==");
}

#[test]
fn input_skips_anthropic_thinking_on_responses_path() {
    // Anthropic thinking (signature only, no id/encrypted) is not an OpenAI reasoning item.
    let msg = Message {
        role: Role::Assistant,
        content: vec![
            ContentBlock::Thinking {
                text: "anthropic reasoning".into(),
                signature: Some("SIG".into()),
                id: None,
                encrypted: None,
                opaque: None,
            },
            ContentBlock::Text("answer".into()),
        ],
    };
    let items = build_input_items(&[msg], false);
    assert!(
        items.iter().all(|i| i["type"] != "reasoning"),
        "cross-provider thinking is not replayed as an OpenAI reasoning item: {items:?}"
    );
    assert_eq!(items[0]["role"], "assistant");
    assert_eq!(items[0]["content"], "answer");
}

#[test]
fn sse_captures_reasoning_summary_and_item() {
    let text = concat!(
        "event: response.reasoning_summary_text.delta\n",
        "data: {\"output_index\":0,\"delta\":\"thinking...\"}\n\n",
        "event: response.output_item.done\n",
        "data: {\"output_index\":0,\"item\":{\"type\":\"reasoning\",\"id\":\"rs_9\",\"encrypted_content\":\"ENC==\",\"summary\":[]}}\n\n",
    );
    let events = parse_responses_sse(text);
    assert!(
        events.iter().any(|e| matches!(e, ProviderEvent::ThinkingDelta { index: 0, delta } if delta == "thinking...")),
        "summary delta maps to ThinkingDelta at its output index: {events:?}"
    );
    assert!(
        events.iter().any(|e| matches!(e, ProviderEvent::ReasoningItem { index: 0, id: Some(id), encrypted: Some(enc) } if id == "rs_9" && enc == "ENC==")),
        "output_item.done reasoning maps to ReasoningItem with id + encrypted: {events:?}"
    );
}

#[test]
fn input_assistant_tool_call() {
    let msg = Message {
        role: Role::Assistant,
        content: vec![ContentBlock::ToolCall {
            id: "call_abc".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({"path": "foo.rs"}),
        }],
    };
    let items = build_input_items(&[msg], false);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["type"], "function_call");
    assert_eq!(items[0]["call_id"], "call_abc");
    assert_eq!(items[0]["name"], "read_file");
    // Arguments are a JSON string.
    assert!(items[0]["arguments"].is_string());
    let args: serde_json::Value =
        serde_json::from_str(items[0]["arguments"].as_str().unwrap()).unwrap();
    assert_eq!(args["path"], "foo.rs");
}

#[test]
fn input_tool_result() {
    let msg = Message {
        role: Role::Tool,
        content: vec![ContentBlock::ToolResult {
            call_id: "call_abc".into(),
            content: vec![ToolOutput::Text("file contents".into())],
            is_error: false,
        }],
    };
    let items = build_input_items(&[msg], false);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["type"], "function_call_output");
    assert_eq!(items[0]["call_id"], "call_abc");
    assert_eq!(items[0]["output"], "file contents");
}

#[test]
fn input_multiple_tool_results_expand() {
    let msg = Message {
        role: Role::Tool,
        content: vec![
            ContentBlock::ToolResult {
                call_id: "c1".into(),
                content: vec![ToolOutput::Text("result A".into())],
                is_error: false,
            },
            ContentBlock::ToolResult {
                call_id: "c2".into(),
                content: vec![ToolOutput::Text("result B".into())],
                is_error: false,
            },
        ],
    };
    let items = build_input_items(&[msg], false);
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["call_id"], "c1");
    assert_eq!(items[1]["call_id"], "c2");
}

// -----------------------------------------------------------------------
// SSE parsing
// -----------------------------------------------------------------------

#[test]
fn sse_output_text_delta() {
    let sse = "event: response.output_text.delta\ndata: {\"delta\":\"Hello, world!\"}\n\n";
    let events = parse_responses_sse(sse);
    assert_eq!(events.len(), 1);
    assert!(
        matches!(&events[0], ProviderEvent::TextDelta { index: 0, delta } if delta == "Hello, world!")
    );
}
