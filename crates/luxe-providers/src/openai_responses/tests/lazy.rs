//! Lazy tools — activation, replay and the refusal fallback.

use super::*;

pub(super) fn activation(id: &str, names: &[&str], is_error: bool) -> [Message; 2] {
    [
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolCall {
                id: id.to_string(),
                name: luxe_core::tool::ACTIVATE_TOOLS_NAME.to_string(),
                arguments: serde_json::json!({ "names": names }),
            }],
        },
        Message::tool_result(id, "Activated.", is_error),
    ]
}

/// The engine's request: core plus the activated ones in `tools`, every deferred tool beside.
pub(super) fn lazy_req(activated: &[&str], messages: Vec<Message>) -> CompletionRequest {
    let mut tools: Vec<ToolSchema> =
        ["activate_tools", "bash", "read"].iter().chain(activated).map(|n| schema(n)).collect();
    tools.sort_by(|a, b| a.name.cmp(&b.name));
    CompletionRequest {
        tools,
        deferred_tools: vec![schema("debug"), schema("test")],
        messages,
        ..make_req("gpt-5.4")
    }
}

pub(super) fn names(tools: &serde_json::Value) -> Vec<&str> {
    tools.as_array().unwrap().iter().map(|tool| tool["name"].as_str().unwrap()).collect()
}

/// Activating a tool leaves `tools` byte-identical and loads the tool right after the result
/// that asked for it, so the cached prefix runs on past the activation.
#[test]
fn an_activation_loads_in_place_and_the_tool_list_holds_still() {
    let p = make_provider();
    let before = p.build_request_body(&lazy_req(&[], vec![Message::user("go")]));
    let mut messages = vec![Message::user("go")];
    messages.extend(activation("call-1", &["test", "read", "nope"], false));
    let after = p.build_request_body(&lazy_req(&["test"], messages));

    assert_eq!(names(&before["tools"]), vec!["activate_tools", "bash", "read"]);
    assert_eq!(before["tools"], after["tools"], "the list holds still");
    let input = after["input"].as_array().unwrap();
    assert_eq!(input[0], before["input"][0], "an append");
    assert_eq!(input[2]["type"], "function_call_output");
    assert_eq!(input[3]["type"], "additional_tools");
    assert_eq!(input[3]["role"], "developer");
    assert_eq!(names(&input[3]["tools"]), vec!["test"], "only a deferred tool is loaded");
    assert_eq!(input[3]["tools"][0]["strict"], false, "shaped like any other tool");
}

/// A call to a tool loaded in place replays in that tool's namespace, which is where the API
/// put it (a live 400 without it: "Missing namespace for function_call 'module_report'").
#[test]
fn a_loaded_tool_s_call_replays_in_its_namespace() {
    let mut messages = vec![Message::user("go")];
    messages.extend(activation("call-1", &["test"], false));
    messages.push(Message {
        role: Role::Assistant,
        content: vec![ContentBlock::ToolCall {
            id: "call-2".into(),
            name: "test".into(),
            arguments: serde_json::json!({}),
        }],
    });
    messages.push(Message::tool_result("call-2", "1 passed", false));
    let body = make_provider().build_request_body(&lazy_req(&["test"], messages));
    let input = body["input"].as_array().unwrap();
    assert_eq!(input[1].get("namespace"), None, "a core call has none");
    assert_eq!(input[4]["type"], "function_call");
    assert_eq!(input[4]["namespace"], "test");
}

/// A deferred tool the history calls before anything loaded it (a session that ran with lazy
/// tools off) replays as it was sent, with no namespace: it was never loaded into one, and the
/// API takes a call to a tool it no longer has.
#[test]
fn a_call_from_before_any_activation_replays_as_sent() {
    let messages = vec![
        Message::user("go"),
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolCall {
                id: "call-1".into(),
                name: "test".into(),
                arguments: serde_json::json!({}),
            }],
        },
        Message::tool_result("call-1", "1 passed", false),
    ];
    let body = make_provider().build_request_body(&lazy_req(&[], messages));
    assert_eq!(names(&body["tools"]), vec!["activate_tools", "bash", "read"]);
    assert_eq!(body["input"][1]["name"], "test");
    assert_eq!(body["input"][1].get("namespace"), None);
}

/// An activation that failed loads nothing, a pass that withholds tools loads nothing, and
/// with the flag off the request is the list it always was.
#[test]
fn nothing_loads_in_place_without_a_successful_activation_and_a_tool_list() {
    let p = make_provider();
    let mut failed = vec![Message::user("go")];
    failed.extend(activation("call-1", &["test"], true));
    let body = p.build_request_body(&lazy_req(&[], failed));
    assert!(!body.to_string().contains("additional_tools"));

    let mut loaded = vec![Message::user("go")];
    loaded.extend(activation("call-1", &["test"], false));
    let wrap_up = CompletionRequest {
        tools: Vec::new(),
        deferred_tools: Vec::new(),
        ..lazy_req(&["test"], loaded.clone())
    };
    let body = p.build_request_body(&wrap_up);
    assert!(body.get("tools").is_none());
    assert!(!body.to_string().contains("additional_tools"));

    let listing = OpenaiResponsesProvider::new(ProviderConfig {
        compat: luxe_core::provider::CompatFlags {
            defer_tool_loading: false,
            ..Default::default()
        },
        ..Default::default()
    });
    let body = listing.build_request_body(&lazy_req(&["test"], loaded));
    assert_eq!(names(&body["tools"]), vec!["activate_tools", "bash", "read", "test"]);
    assert!(!body.to_string().contains("additional_tools"));
}

/// An endpoint that doesn't know `additional_tools` is told once: the adapter lists the
/// deferred tools instead, retries, and keeps listing them for that model.
#[tokio::test]
async fn a_refusal_of_additional_tools_falls_back_to_listing_and_is_remembered() {
    use futures::StreamExt;

    let rejection = r#"{"error":{"message":"Invalid value: 'additional_tools'. Supported values are: 'message', 'function_call', 'function_call_output'.","type":"invalid_request_error"}}"#;
    let turn = concat!(
        "event: response.output_text.delta\n",
        "data: {\"delta\":\"ok\"}\n\n",
        "event: response.completed\n",
        "data: {\"response\":{\"usage\":{\"input_tokens\":5,\"output_tokens\":1},",
        "\"output\":[{\"type\":\"message\"}]}}\n\n",
    );
    let server = serve_replies(vec![
        http_reply("400 Bad Request", "application/json", rejection),
        http_reply("200 OK", "text/event-stream", turn),
        http_reply("200 OK", "text/event-stream", turn),
    ]);
    let port = server.port();

    let provider = OpenaiResponsesProvider::new(ProviderConfig {
        api_key: Some("sk-test".into()),
        base_url: Some(format!("http://127.0.0.1:{port}/v1")),
        ..Default::default()
    });
    let mut messages = vec![Message::user("go")];
    messages.extend(activation("call-1", &["test"], false));
    for _ in 0..2 {
        let (_keep, cancel) = tokio::sync::watch::channel(false);
        let stream = provider
            .stream(lazy_req(&["test"], messages.clone()), cancel)
            .await
            .expect("the retry recovers");
        let events: Vec<ProviderEvent> = stream.collect().await;
        assert!(events.iter().any(|event| matches!(event, ProviderEvent::TextDelta { .. })));
    }

    let requests = server.requests();
    let bodies: Vec<&str> = requests.iter().map(|raw| request_body(raw)).collect();
    assert_eq!(bodies.len(), 3, "one refusal, its retry, then the next turn");
    assert!(bodies[0].contains("additional_tools"), "{}", bodies[0]);
    assert!(!bodies[1].contains("additional_tools"), "retried listed: {}", bodies[1]);
    assert!(bodies[1].contains("\"name\":\"test\""), "with the tool listed: {}", bodies[1]);
    assert!(!bodies[2].contains("additional_tools"), "and remembered: {}", bodies[2]);
}
