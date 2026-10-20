use super::*;
use luxe_core::message::{ContentBlock, Message, Role, ToolOutput};
use luxe_core::provider::PromptReuse;

fn request_with(system: Option<&str>, messages: Vec<Message>) -> CompletionRequest {
    CompletionRequest {
        model: ModelInfo::default(),
        messages,
        system: system.map(str::to_string),
        tools: vec![],
        thinking: ThinkingLevel::Off,
        max_tokens: Some(1024),
        ..Default::default()
    }
}

/// The regression that made every Anthropic turn run with no instructions at all: the body
/// was built from a `System`-role message the engine never creates.
#[test]
fn the_system_prompt_reaches_the_body_from_the_request_field() {
    let req =
        request_with(Some("## Role\n\nYou are a senior engineer."), vec![Message::user("hi")]);
    let body = build_anthropic_request(
        &req,
        &ModelInfo::default(),
        &AnthropicCompat { cache_control: false, ..AnthropicCompat::default() },
        ThinkingPlan { mode: ThinkingMode::Extended, can_disable: true, between_tools: false },
        ToolLoading::InPlace,
    );

    assert_eq!(body["system"], "## Role\n\nYou are a senior engineer.");
}

#[test]
fn a_system_role_message_still_works_for_an_embedder() {
    let messages = vec![
        Message { role: Role::System, content: vec![ContentBlock::Text("from a message".into())] },
        Message::user("hi"),
    ];
    let body = build_anthropic_request(
        &request_with(None, messages),
        &ModelInfo::default(),
        &AnthropicCompat { cache_control: false, ..AnthropicCompat::default() },
        ThinkingPlan { mode: ThinkingMode::Extended, can_disable: true, between_tools: false },
        ToolLoading::InPlace,
    );

    assert_eq!(body["system"], "from a message");
    // The Messages API takes only `user` and `assistant` turns: a system one there is a 400
    let roles: Vec<&str> =
        body["messages"].as_array().unwrap().iter().filter_map(|m| m["role"].as_str()).collect();
    assert_eq!(roles, ["user"]);
}

#[test]
fn an_absent_system_prompt_omits_the_field_rather_than_sending_an_empty_one() {
    let body = build_anthropic_request(
        &request_with(None, vec![Message::user("hi")]),
        &ModelInfo::default(),
        &AnthropicCompat::default(),
        ThinkingPlan { mode: ThinkingMode::Extended, can_disable: true, between_tools: false },
        ToolLoading::InPlace,
    );

    assert!(body.get("system").is_none());
}

/// Caching only pays if the prompt is actually there to cache.
#[test]
fn the_system_prompt_carries_a_cache_breakpoint() {
    let body = build_anthropic_request(
        &request_with(Some("stable instructions"), vec![Message::user("hi")]),
        &ModelInfo::default(),
        &AnthropicCompat::default(),
        ThinkingPlan { mode: ThinkingMode::Extended, can_disable: true, between_tools: false },
        ToolLoading::InPlace,
    );

    assert_eq!(body["system"][0]["text"], "stable instructions");
    assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
}

/// The marker that pays for itself: without one at the previous turn's end, a batch of
/// parallel tool calls appends more than the twenty content blocks a lookup will walk back
/// through, and the whole conversation is re-billed as a cache write.
#[test]
fn both_the_last_turn_and_the_one_before_it_are_marked() {
    let messages = vec![
        Message::user("start"),
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolCall {
                id: "call-1".into(),
                name: "read".into(),
                arguments: serde_json::json!({}),
            }],
        },
        Message {
            role: Role::Tool,
            content: vec![ContentBlock::ToolResult {
                call_id: "call-1".into(),
                content: vec![ToolOutput::Text("file body".into())],
                is_error: false,
            }],
        },
    ];
    let body = build_anthropic_request(
        &request_with(Some("stable"), messages),
        &ModelInfo::default(),
        &AnthropicCompat::default(),
        ThinkingPlan { mode: ThinkingMode::Extended, can_disable: true, between_tools: false },
        ToolLoading::InPlace,
    );

    let marked: Vec<usize> = body["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .enumerate()
        .filter(|(_, message)| {
            message["content"]
                .as_array()
                .is_some_and(|blocks| blocks.iter().any(|b| b.get("cache_control").is_some()))
        })
        .map(|(at, _)| at)
        .collect();
    assert_eq!(marked, vec![0, 2], "this turn's end and the previous one's");
}

/// Anthropic rejects a marker on a reasoning block, so a turn ending in one takes it on the
/// last block that can hold it instead of losing the breakpoint.
#[test]
fn a_reply_that_ends_in_reasoning_still_gets_a_breakpoint() {
    let messages = vec![
        Message::user("start"),
        Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Text("working on it".into()),
                ContentBlock::Thinking {
                    text: "hmm".into(),
                    signature: Some("sig".into()),
                    id: None,
                    encrypted: None,
                    opaque: None,
                },
            ],
        },
    ];
    let body = build_anthropic_request(
        &request_with(Some("stable"), messages),
        &ModelInfo::default(),
        &AnthropicCompat::default(),
        ThinkingPlan { mode: ThinkingMode::Extended, can_disable: true, between_tools: false },
        ToolLoading::InPlace,
    );

    let blocks = body["messages"][1]["content"].as_array().expect("content");
    assert_eq!(blocks[0]["type"], "text");
    assert_eq!(blocks[0]["cache_control"]["type"], "ephemeral");
    assert!(blocks[1].get("cache_control").is_none(), "never on a thinking block");
}

/// The summarizer and the guide pass each build a prompt, send it once and throw it away.
/// Caching one costs a quarter more than sending it plainly and returns nothing, and the
/// summarizer's prompt is a whole context window by construction — so it is the expensive one
/// to get wrong.
#[test]
fn a_prompt_nothing_will_read_back_asks_for_no_cache_at_all() {
    let body = |reuse| {
        let mut request = request_with(Some("stable"), vec![Message::user("summarize this")]);
        request.reuse = reuse;
        build_anthropic_request(
            &request,
            &ModelInfo::default(),
            &AnthropicCompat::default(),
            ThinkingPlan { mode: ThinkingMode::Extended, can_disable: true, between_tools: false },
            ToolLoading::InPlace,
        )
    };

    let one_off = body(PromptReuse::OneOff);
    assert!(one_off["system"].is_string(), "not even normalized to blocks: {one_off}");
    assert_eq!(
        serde_json::to_string(&one_off).expect("serializes").matches("cache_control").count(),
        0,
    );

    // …and the conversation it was summarizing still asks for every breakpoint it can use.
    let turn = body(PromptReuse::Conversation);
    assert_eq!(turn["system"][0]["cache_control"]["type"], "ephemeral");
    assert_eq!(turn["messages"][0]["content"][0]["cache_control"]["type"], "ephemeral");
}

/// The lifetime is opt-in and applies to every breakpoint at once, so a session either keeps
/// its cache for an hour or it doesn't — a prefix cached under two lifetimes is two entries.
#[test]
fn the_hour_long_cache_is_asked_for_only_when_the_endpoint_says_it_takes_one() {
    let body = |long: bool| {
        build_anthropic_request(
            &request_with(Some("stable"), vec![Message::user("hi")]),
            &ModelInfo::default(),
            &AnthropicCompat { long_cache_retention: long, ..AnthropicCompat::default() },
            ThinkingPlan { mode: ThinkingMode::Extended, can_disable: true, between_tools: false },
            ToolLoading::InPlace,
        )
    };

    assert!(body(false)["system"][0]["cache_control"].get("ttl").is_none());
    let hour = body(true);
    assert_eq!(hour["system"][0]["cache_control"]["ttl"], "1h");
    assert_eq!(hour["messages"][0]["content"][0]["cache_control"]["ttl"], "1h");
}

/// A budget that tracked `max_tokens` exactly would move every turn as the prompt grows, and
/// Anthropic treats any change to it as a reason to drop the cached conversation.
#[test]
fn the_thinking_budget_holds_still_as_the_window_fills() {
    // Room to spare: the level's own budget, unchanged.
    assert_eq!(manual_thinking_budget(ThinkingLevel::High, 64_000), 16_384);
    // Closing in: reduced, but snapped so a few hundred tokens of drift changes nothing.
    let squeezed: Vec<u32> = [20_000, 19_800, 19_100, 18_500]
        .into_iter()
        .map(|max| manual_thinking_budget(ThinkingLevel::XHigh, max))
        .collect();
    assert_eq!(squeezed, vec![9_216, 9_216, 9_216, 9_216]);
}

/// Thinking and the reply share `max_tokens`, so a budget that doesn't fit must not take the
/// reply's share with it. Real caps: Copilot serves Sonnet 4.6 at 32k, Haiku 4.5 tops out at
/// 64k, and both still take a token budget.
#[test]
fn a_squeezed_thinking_budget_leaves_the_answer_room_to_finish() {
    for (level, max) in [
        (ThinkingLevel::XHigh, 32_000),
        (ThinkingLevel::Ultra, 32_000),
        (ThinkingLevel::Ultra, 64_000),
        (ThinkingLevel::High, 20_000),
        (ThinkingLevel::Medium, 8_000),
    ] {
        let budget = manual_thinking_budget(level, max);
        let answer = max - budget;
        assert!(
            answer >= (max / 2).min(16_384),
            "{level:?} at {max}: budget {budget} leaves the answer only {answer}"
        );
        assert!(budget >= 1_024, "still a budget Anthropic accepts");
    }
    // A level that fits is never touched.
    assert_eq!(manual_thinking_budget(ThinkingLevel::XHigh, 128_000), 32_768);
    assert_eq!(manual_thinking_budget(ThinkingLevel::Ultra, 128_000), 65_536);
}

#[test]
fn a_steer_after_tool_results_merges_into_one_user_turn() {
    // Tool results serialize as `user`, so a steering message right behind them would
    // otherwise send two consecutive user turns.
    let messages = vec![
        Message::user("start"),
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolCall {
                id: "call-1".into(),
                name: "read".into(),
                arguments: serde_json::json!({}),
            }],
        },
        Message {
            role: Role::Tool,
            content: vec![ContentBlock::ToolResult {
                call_id: "call-1".into(),
                content: vec![ToolOutput::Text("file body".into())],
                is_error: false,
            }],
        },
        Message::user("actually, check the tests too"),
    ];

    let serialized = serialize_messages(&messages, false);
    let roles: Vec<&str> = serialized.iter().filter_map(|m| m["role"].as_str()).collect::<Vec<_>>();
    assert_eq!(roles, vec!["user", "assistant", "user"], "roles must alternate");

    let merged = serialized.last().expect("the merged user turn");
    let blocks = merged["content"].as_array().expect("content array");
    assert_eq!(blocks.len(), 2, "tool result + steer text share one turn");
    assert_eq!(blocks[0]["type"], "tool_result");
    assert_eq!(blocks[1]["type"], "text");
    assert_eq!(blocks[1]["text"], "actually, check the tests too");
}

// === In-place tool loading ===

fn schema(name: &str) -> luxe_core::provider::ToolSchema {
    luxe_core::provider::ToolSchema {
        name: name.to_string(),
        description: format!("The {name} tool."),
        parameters: serde_json::json!({"type": "object", "properties": {}}),
    }
}

fn schemas(names: &[&str]) -> Vec<luxe_core::provider::ToolSchema> {
    names.iter().map(|name| schema(name)).collect()
}

fn activation(id: &str, names: &[&str], said: &str, is_error: bool) -> [Message; 2] {
    [
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolCall {
                id: id.to_string(),
                name: luxe_core::tool::ACTIVATE_TOOLS_NAME.to_string(),
                arguments: serde_json::json!({ "names": names }),
            }],
        },
        Message::tool_result(id, said, is_error),
    ]
}

/// A lazy-tools request the way the engine builds one: the core tier plus whatever has been
/// activated in `tools`, and every deferred tool in `deferred_tools`.
fn lazy_request(activated: &[&str], messages: Vec<Message>) -> CompletionRequest {
    let mut tools = schemas(&["activate_tools", "bash", "read"]);
    tools.extend(schemas(activated));
    tools.sort_by(|a, b| a.name.cmp(&b.name));
    CompletionRequest {
        tools,
        deferred_tools: schemas(&["debug", "test"]),
        ..request_with(Some("stable"), messages)
    }
}

fn body(req: &CompletionRequest, loading: ToolLoading) -> serde_json::Value {
    build_anthropic_request(
        req,
        &ModelInfo::default(),
        &AnthropicCompat::default(),
        ThinkingPlan { mode: ThinkingMode::Extended, can_disable: true, between_tools: false },
        loading,
    )
}

fn tool_names(body: &serde_json::Value) -> Vec<String> {
    body["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .map(|tool| tool["name"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The listed tools lead and the deferred ones follow with `defer_loading`. The cache marker
/// sits on the last listed tool, since Anthropic 400s on a deferred one that carries it.
#[test]
fn deferred_tools_follow_the_list_and_never_carry_the_marker() {
    let body = body(&lazy_request(&[], vec![Message::user("hi")]), ToolLoading::InPlace);
    assert_eq!(tool_names(&body), vec!["activate_tools", "bash", "read", "debug", "test"]);
    let tools = body["tools"].as_array().unwrap();
    for tool in &tools[..3] {
        assert!(tool.get("defer_loading").is_none(), "listed: {tool}");
    }
    for tool in &tools[3..] {
        assert_eq!(tool["defer_loading"], true, "deferred: {tool}");
        assert!(tool.get("cache_control").is_none(), "never marked: {tool}");
    }
    assert_eq!(tools[2]["cache_control"]["type"], "ephemeral", "the last listed tool is");
}

/// The point of the whole thing: activating a tool leaves the tool list, the system prompt and
/// the conversation before it byte-identical, so the next request reads all of it from cache.
/// Listing it instead rewrites the list, which sits ahead of everything else in the prefix.
#[test]
fn an_activation_leaves_the_cached_prefix_alone() {
    let opening = vec![Message::user("load the test runner")];
    let before = lazy_request(&[], opening.clone());
    let mut grown = opening;
    grown.extend(activation("call-1", &["test"], "Activated: test.", false));
    let after = lazy_request(&["test"], grown);

    let (was, now) = (body(&before, ToolLoading::InPlace), body(&after, ToolLoading::InPlace));
    assert_eq!(was["tools"], now["tools"], "the list holds still");
    assert_eq!(was["system"], now["system"]);
    let strip = |message: &serde_json::Value| {
        let mut message = message.clone();
        if let Some(blocks) = message["content"].as_array_mut() {
            for block in blocks {
                block.as_object_mut().map(|block| block.remove("cache_control"));
            }
        }
        message
    };
    assert_eq!(strip(&was["messages"][0]), strip(&now["messages"][0]), "an append");

    let listed = (body(&before, ToolLoading::Listed), body(&after, ToolLoading::Listed));
    assert_ne!(listed.0["tools"], listed.1["tools"], "what listing it costs");
}

/// A successful activation answers with references and nothing else: the API refuses a result
/// that mixes them with text, and text beside the result strips earlier reasoning and misses
/// the cache (measured: a 7,794-token rewrite on the next call). Only a deferred tool is
/// referenced, since a core one needs no loading and an unknown one is a 400 that would replay
/// every turn.
#[test]
fn an_activation_answers_with_references_alone() {
    let mut messages = vec![Message::user("go")];
    messages.extend(activation(
        "call-1",
        &["test", "read", "nope"],
        "Activated: test.\nNot activatable (already available or unknown): nope, read.",
        false,
    ));
    let body = body(&lazy_request(&["test"], messages), ToolLoading::InPlace);
    let turn = body["messages"][2]["content"].as_array().expect("the result turn");
    assert_eq!(turn.len(), 1, "the result and nothing beside it: {turn:?}");
    assert_eq!(turn[0]["type"], "tool_result");
    assert_eq!(turn[0]["tool_use_id"], "call-1");
    assert_eq!(
        turn[0]["content"],
        serde_json::json!([{ "type": "tool_reference", "tool_name": "test" }])
    );
}

/// An activation that loaded nothing stays what it was: a plain error result.
#[test]
fn a_failed_activation_is_left_as_text() {
    let mut messages = vec![Message::user("go")];
    messages.extend(activation("call-1", &["test"], "Error: no.", true));
    let body = body(&lazy_request(&[], messages), ToolLoading::InPlace);
    let turn = &body["messages"][2]["content"];
    assert_eq!(turn[0]["content"], "Error: no.");
    assert_eq!(turn[0]["is_error"], true);
}

/// Listing is the request luxe always sent: the engine's list as is, and no reference anywhere.
/// The same holds for a pass that withholds tools, where a reference would point at nothing.
#[test]
fn listing_and_a_toolless_pass_send_no_deferred_tools() {
    let mut messages = vec![Message::user("go")];
    messages.extend(activation("call-1", &["test"], "Activated: test.", false));
    let lazy = lazy_request(&["test"], messages.clone());

    let listed = body(&lazy, ToolLoading::Listed);
    assert_eq!(tool_names(&listed), vec!["activate_tools", "bash", "read", "test"]);
    let plain = CompletionRequest { deferred_tools: Vec::new(), ..lazy.clone() };
    assert_eq!(listed, body(&plain, ToolLoading::InPlace), "identical to no lazy tools at all");

    let wrap_up = CompletionRequest { tools: Vec::new(), deferred_tools: Vec::new(), ..lazy };
    let body = body(&wrap_up, ToolLoading::InPlace);
    assert!(body.get("tools").is_none());
    assert!(!body.to_string().contains("tool_reference"), "{body}");
}

#[test]
fn a_refusal_of_in_place_loading_is_recognized() {
    for body in [
        r#"{"error":{"message":"Tool reference 'nope' not found in available tools"}}"#,
        r#"{"error":{"message":"tools.3.defer_loading: Extra inputs are not permitted"}}"#,
        r#"{"error":{"message":"Tool definitions/code execution functions cannot be mixed with other content"}}"#,
        r#"{"error":{"message":"messages.2.content.0.content.0.type: unknown variant tool_reference"}}"#,
    ] {
        assert!(refuses_in_place_loading(body), "{body}");
    }
    assert!(!refuses_in_place_loading(r#"{"error":{"message":"max_tokens: too large"}}"#));
}
