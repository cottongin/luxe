//! Request — thinking, caching, sampling and templates in the body.

use super::*;

pub(super) fn thinking_request(model: ModelInfo, thinking: ThinkingLevel) -> CompletionRequest {
    CompletionRequest {
        model,
        messages: vec![Message::user("Hello")],
        system: None,
        tools: vec![],
        thinking,
        max_tokens: Some(4096),
        ..Default::default()
    }
}

pub(super) fn provider(compat: CompatFlags) -> OpenaiCompatProvider {
    OpenaiCompatProvider::new(ProviderConfig::default(), compat)
}

/// Copilot's chat completions carries reasoning in fields of its own. Its text is shown, its
/// opaque state is kept, and both go back with the message they came on — to Copilot only.
/// The records are copied from live Gemini and Kimi streams.
#[test]
fn copilots_reasoning_is_read_kept_and_replayed() {
    let events = decode_records(&[
        r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","reasoning_text":"Checking divisors"}}]}"#,
        r#"data: {"choices":[{"index":0,"delta":{"reasoning_opaque":"CpcBAb4+9vs"}}]}"#,
        r#"data: {"choices":[{"index":0,"delta":{"content":"No."},"finish_reason":"stop"}]}"#,
    ]);
    assert!(events.iter().any(|event| matches!(
        event,
        ProviderEvent::ThinkingDelta { delta, .. } if delta == "Checking divisors"
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        ProviderEvent::ReasoningOpaque { opaque, .. } if opaque == "CpcBAb4+9vs"
    )));

    let replayed = Message {
        role: luxe_core::message::Role::Assistant,
        content: vec![
            ContentBlock::Thinking {
                text: "Checking divisors".into(),
                signature: None,
                id: None,
                encrypted: None,
                opaque: Some("CpcBAb4+9vs".into()),
            },
            ContentBlock::ToolCall {
                id: "call_1".into(),
                name: "read".into(),
                arguments: serde_json::json!({}),
            },
        ],
    };
    let mut copilot = Vec::new();
    push_wire_messages(&replayed, &mut copilot, &HashMap::new(), true);
    assert_eq!(copilot[0]["reasoning_opaque"], "CpcBAb4+9vs");
    assert_eq!(copilot[0]["reasoning_text"], "Checking divisors");
    let mut elsewhere = Vec::new();
    push_wire_messages(&replayed, &mut elsewhere, &HashMap::new(), false);
    assert!(elsewhere[0].get("reasoning_opaque").is_none(), "{}", elsewhere[0]);
    assert!(elsewhere[0].get("reasoning_text").is_none());
}

/// Reasoning is output. Copilot reports it beside the counts, and for Gemini leaves it out of
/// `completion_tokens` while the total holds it; for Kimi it is inside. Both live usage blocks.
#[test]
fn copilots_reasoning_tokens_count_as_output_once() {
    let usage_of = |record: &str| {
        decode_records(&[record])
            .into_iter()
            .find_map(|event| match event {
                ProviderEvent::Usage(usage) => Some(usage),
                _ => None,
            })
            .expect("a usage event")
    };
    let gemini = usage_of(
        r#"data: {"choices":[],"usage":{"completion_tokens":1,"prompt_tokens":19,"prompt_tokens_details":{"cached_tokens":0},"total_tokens":1460,"reasoning_tokens":1440}}"#,
    );
    assert_eq!((gemini.completion_tokens, gemini.reasoning_tokens), (1441, 1440));
    let kimi = usage_of(
        r#"data: {"choices":[],"usage":{"completion_tokens":1500,"completion_tokens_details":{},"prompt_tokens":103,"prompt_tokens_details":{"cached_tokens":0},"total_tokens":1603,"reasoning_tokens":1500}}"#,
    );
    assert_eq!((kimi.completion_tokens, kimi.reasoning_tokens), (1500, 1500));
}

#[test]
fn stream_options_requests_usage_when_supported() {
    let req = thinking_request(ModelInfo::default(), ThinkingLevel::Off);
    // Default compat has usage-in-streaming on → the request opts into streamed usage.
    let on = provider(CompatFlags::default()).build_request_body(&req);
    assert_eq!(on["stream_options"]["include_usage"], true);
    // Explicit opt-out omits the field entirely (for strict servers).
    let off_flags = CompatFlags { supports_usage_in_streaming: false, ..Default::default() };
    let off = provider(off_flags).build_request_body(&req);
    assert!(off.get("stream_options").is_none());
}

#[test]
fn openai_effort_uses_model_specific_values_and_ultra_alias() {
    let model = ModelInfo {
        r#ref: ModelRef::new("openai", "gpt-5.6-sol"),
        reasoning: true,
        thinking_level_map: HashMap::from([
            ("minimal".to_string(), Some("low".to_string())),
            ("xhigh".to_string(), Some("xhigh".to_string())),
            ("max".to_string(), Some("max".to_string())),
        ]),
        ..Default::default()
    };
    let compat = CompatFlags {
        supports_reasoning_effort: true,
        thinking_format: Some("openai".to_string()),
        ..Default::default()
    };
    let provider = provider(compat);

    let minimal =
        provider.build_request_body(&thinking_request(model.clone(), ThinkingLevel::Minimal));
    let ultra = provider.build_request_body(&thinking_request(model.clone(), ThinkingLevel::Ultra));
    let off = provider.build_request_body(&thinking_request(model, ThinkingLevel::Off));

    assert_eq!(minimal["reasoning_effort"], "low");
    assert_eq!(ultra["reasoning_effort"], "max");
    assert_eq!(off["reasoning_effort"], "none");
}

/// A models.dev entry for OpenRouter, with the reasoning dials it declares.
pub(super) fn openrouter_model(
    id: &str,
    controls: luxe_core::provider::ThinkingControls,
) -> ModelInfo {
    ModelInfo {
        r#ref: ModelRef::new("openrouter", id),
        reasoning: true,
        thinking_controls: controls,
        thinking_level_map: HashMap::from([
            ("low".to_string(), Some("low".to_string())),
            ("medium".to_string(), Some("medium".to_string())),
            ("high".to_string(), Some("high".to_string())),
        ]),
        ..Default::default()
    }
}

/// The reported failure: OpenRouter answers 400 "Reasoning is mandatory for this endpoint and
/// cannot be disabled" to `effort: "none"` on roughly a fifth of its catalog — grok-4.x,
/// gemini-3.x, the thinking Qwens, deepseek-r1 — and off is the level a session starts on, so
/// every turn on those models failed. Asking for nothing is the only spelling they accept.
#[test]
fn openrouter_asks_a_mandatory_reasoning_endpoint_for_nothing_rather_than_for_none() {
    let provider = provider(CompatFlags {
        supports_reasoning_effort: true,
        thinking_format: Some("openrouter".to_string()),
        ..Default::default()
    });
    let toggleable = luxe_core::provider::ThinkingControls {
        effort: true,
        can_disable: true,
        ..Default::default()
    };
    let mandatory = luxe_core::provider::ThinkingControls { effort: true, ..Default::default() };

    let off_where_allowed = provider.build_request_body(&thinking_request(
        openrouter_model("anthropic/claude-sonnet-5", toggleable),
        ThinkingLevel::Off,
    ));
    let off_where_mandatory = provider.build_request_body(&thinking_request(
        openrouter_model("x-ai/grok-4.6", mandatory),
        ThinkingLevel::Off,
    ));
    let on_where_mandatory = provider.build_request_body(&thinking_request(
        openrouter_model("x-ai/grok-4.6", mandatory),
        ThinkingLevel::High,
    ));

    assert_eq!(off_where_allowed["reasoning"]["effort"], "none");
    assert!(
        off_where_mandatory.get("reasoning").is_none(),
        "a mandatory endpoint 400s on every way of asking it to stop: {off_where_mandatory}"
    );
    assert_eq!(on_where_mandatory["reasoning"]["effort"], "high");
}

/// OpenRouter does not cache Claude implicitly, so without this an agentic session re-reads its
/// whole transcript at full price every turn. One field, and only for the gateway that reads it.
#[test]
fn only_the_openrouter_format_asks_for_prompt_caching() {
    let req = thinking_request(ModelInfo::default(), ThinkingLevel::Off);
    let asked = provider(CompatFlags {
        cache_control_format: Some("openrouter".to_string()),
        ..Default::default()
    })
    .build_request_body(&req);
    let anthropics_own = provider(CompatFlags {
        cache_control_format: Some("anthropic".to_string()),
        ..Default::default()
    })
    .build_request_body(&req);
    let plain = provider(CompatFlags::default()).build_request_body(&req);

    assert_eq!(asked["cache_control"]["type"], "ephemeral");
    assert!(
        anthropics_own.get("cache_control").is_none(),
        "per-block markers belong to the Messages adapter, not this one"
    );
    assert!(plain.get("cache_control").is_none());

    // A prompt nothing will read back is not worth the write premium, whatever the gateway.
    let mut one_off = thinking_request(ModelInfo::default(), ThinkingLevel::Off);
    one_off.reuse = luxe_core::provider::PromptReuse::OneOff;
    let summarizing = provider(CompatFlags {
        cache_control_format: Some("openrouter".to_string()),
        ..Default::default()
    })
    .build_request_body(&one_off);
    assert!(summarizing.get("cache_control").is_none(), "{summarizing}");
}

/// The headers a request carries beyond `Content-Type` are opt-in per provider, and one
/// provider's must never ride along on another's.
#[test]
fn attribution_headers_go_only_where_they_were_asked_for() {
    let req = thinking_request(ModelInfo::default(), ThinkingLevel::Off);
    let named =
        request_headers(&CompatFlags { openrouter_headers: true, ..Default::default() }, &req);
    assert_eq!(named, vec![("HTTP-Referer", LUXE_HOME), ("X-Title", "luxe")]);
    assert!(request_headers(&CompatFlags::default(), &req).is_empty());
    assert!(
        !request_headers(&CompatFlags { copilot_headers: true, ..Default::default() }, &req)
            .iter()
            .any(|(name, _)| *name == "X-Title"),
        "Copilot has no use for OpenRouter's attribution"
    );
}

/// Cache writes are billed at a premium and counted inside `prompt_tokens`, so reporting them
/// as zero understates a turn's cost. OpenRouter sends them; this used to drop them.
#[test]
fn a_cache_write_is_read_off_the_usage_chunk_rather_than_assumed_absent() {
    let events = decode_records(&[
        r#"data: {"choices":[],"usage":{"prompt_tokens":9731,"completion_tokens":49,"total_tokens":9780,"prompt_tokens_details":{"cached_tokens":0,"cache_write_tokens":9729},"completion_tokens_details":{"reasoning_tokens":7}}}"#,
        "data: [DONE]",
    ]);
    let usage = events
        .iter()
        .find_map(|event| match event {
            ProviderEvent::Usage(usage) => Some(usage),
            _ => None,
        })
        .expect("usage decoded");
    assert_eq!(usage.cache_write_tokens, 9729);
    assert_eq!(usage.cache_read_tokens, 0);
    assert_eq!(usage.reasoning_tokens, 7);
    assert_eq!(usage.prompt_tokens, 9731, "writes are counted inside the prompt, not beside it");
}

#[test]
fn qwen_formats_use_boolean_thinking_after_level_clamping() {
    let model = ModelInfo {
        r#ref: ModelRef::new("dashscope", "qwen3.7-max-2026-06-08"),
        reasoning: true,
        thinking_level_map: HashMap::from([
            ("off".to_string(), Some("off".to_string())),
            ("minimal".to_string(), None),
            ("low".to_string(), None),
            ("medium".to_string(), None),
            ("high".to_string(), Some("on".to_string())),
            ("xhigh".to_string(), Some("on".to_string())),
        ]),
        ..Default::default()
    };
    let qwen =
        provider(CompatFlags { thinking_format: Some("qwen".to_string()), ..Default::default() });
    let local = provider(CompatFlags {
        thinking_format: Some("qwen-chat-template".to_string()),
        ..Default::default()
    });

    let clamped = qwen.build_request_body(&thinking_request(model.clone(), ThinkingLevel::Low));
    let off = qwen.build_request_body(&thinking_request(model.clone(), ThinkingLevel::Off));
    let local_high = local.build_request_body(&thinking_request(model, ThinkingLevel::High));

    assert_eq!(clamped["enable_thinking"], true);
    assert_eq!(off["enable_thinking"], false);
    assert_eq!(local_high["chat_template_kwargs"]["enable_thinking"], true);
    assert_eq!(local_high["chat_template_kwargs"]["preserve_thinking"], true);
}

/// A template's thinking vocabulary is the template's, not luxe's: Qwen3.8 takes a
/// `reasoning_effort` argument and can be told *not* to preserve thinking, neither of which
/// the fixed two-key object could express.
#[test]
fn a_configured_chat_template_describes_its_own_arguments() {
    let model = ModelInfo {
        r#ref: ModelRef::new("llama-server", "qwen3.8-27b"),
        reasoning: true,
        thinking_level_map: HashMap::from([
            ("off".to_string(), Some("none".to_string())),
            ("high".to_string(), Some("high".to_string())),
        ]),
        ..Default::default()
    };
    let configured = provider(CompatFlags {
        // Pi's spelling of the format, so an entry can be carried across unchanged.
        thinking_format: Some("chat-template".to_string()),
        chat_template_kwargs: match serde_json::json!({
            "enable_thinking": { "$var": "thinking.enabled" },
            "preserve_thinking": false,
            "reasoning_effort": { "$var": "thinking.level", "omit_when_off": true },
        }) {
            serde_json::Value::Object(map) => map,
            _ => unreachable!("fixture is an object"),
        },
        ..Default::default()
    });

    let high = configured.build_request_body(&thinking_request(model.clone(), ThinkingLevel::High));
    assert_eq!(high["chat_template_kwargs"]["enable_thinking"], true);
    assert_eq!(high["chat_template_kwargs"]["preserve_thinking"], false);
    assert_eq!(high["chat_template_kwargs"]["reasoning_effort"], "high");

    let off = configured.build_request_body(&thinking_request(model, ThinkingLevel::Off));
    assert_eq!(off["chat_template_kwargs"]["enable_thinking"], false);
    assert!(
        off["chat_template_kwargs"].get("reasoning_effort").is_none(),
        "an effort argument is meaningless with thinking off"
    );
}

/// The acceptance case from the spec: a model carrying Qwen's thinking-mode recipe puts all
/// seven keys on the wire, and each one beats the field the adapter would otherwise have sent.
#[test]
fn a_models_sampling_recipe_reaches_the_body_and_outranks_the_adapter() {
    let recipe = serde_json::json!({
        "temperature": 1.0, "top_p": 0.95, "top_k": 20, "min_p": 0.0,
        "presence_penalty": 0.0, "frequency_penalty": 0.0, "repeat_penalty": 1.0,
    });
    let model = ModelInfo {
        r#ref: ModelRef::new("lmstudio", "qwen3.8-27b"),
        reasoning: true,
        sampling_params: match recipe.clone() {
            serde_json::Value::Object(map) => map,
            _ => unreachable!("fixture is an object"),
        },
        thinking_level_map: HashMap::from([("off".to_string(), Some("none".to_string()))]),
        ..Default::default()
    };
    let compat = CompatFlags {
        supports_reasoning_effort: true,
        thinking_format: Some("openai".to_string()),
        ..Default::default()
    };
    let body = provider(compat).build_request_body(&thinking_request(model, ThinkingLevel::Off));

    for (key, value) in recipe.as_object().expect("fixture is an object") {
        assert_eq!(&body[key.as_str()], value, "{key} must reach the body verbatim");
    }
    // Nothing else about the request moved.
    assert_eq!(body["model"], "qwen3.8-27b");
    assert_eq!(body["stream"], true);
    assert_eq!(body["reasoning_effort"], "none");
    assert_eq!(body["max_tokens"], 4096);
}
