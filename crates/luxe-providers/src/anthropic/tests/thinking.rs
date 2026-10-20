//! Thinking — which thinking encoding each model gets, and how a rejection revises it.

use super::*;

#[test]
fn test_adaptive_thinking_mode() {
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

/// The reported failure: Claude Sonnet 5 / Opus 5 advertise effort levels and nothing else, and
/// reject `thinking.type: "enabled"` outright. The declared controls alone must pick adaptive —
/// no compat flag, no hand-written config.
#[test]
fn an_effort_only_model_is_steered_with_adaptive_thinking_and_effort() {
    let model = ModelInfo {
        reasoning: true,
        thinking_controls: ThinkingControls { effort: true, ..Default::default() },
        thinking_level_map: HashMap::from([
            ("minimal".to_string(), None),
            ("low".to_string(), Some("low".to_string())),
            ("medium".to_string(), Some("medium".to_string())),
            ("high".to_string(), Some("high".to_string())),
            ("xhigh".to_string(), Some("xhigh".to_string())),
            ("ultra".to_string(), Some("max".to_string())),
        ]),
        ..Default::default()
    };
    let compat = AnthropicCompat::default();

    let xhigh = build_request(&turn_request(&model, ThinkingLevel::XHigh), &model, &compat);
    assert_eq!(xhigh["thinking"]["type"], "adaptive");
    assert_eq!(xhigh["output_config"]["effort"], "xhigh");
    assert!(xhigh["thinking"].get("budget_tokens").is_none(), "extended thinking would 400");
    // luxe shows reasoning in the transcript, and these models omit the text by default.
    assert_eq!(xhigh["thinking"]["display"], "summarized");

    // A level the model doesn't offer clamps to one it does rather than inventing an effort.
    let minimal = build_request(&turn_request(&model, ThinkingLevel::Minimal), &model, &compat);
    assert_eq!(minimal["output_config"]["effort"], "low");

    // Off keeps its own switch and sends no effort, which also stays clear of the newer rule
    // rejecting a disabled block at xhigh/max effort.
    let off = build_request(&turn_request(&model, ThinkingLevel::Off), &model, &compat);
    assert_eq!(off["thinking"]["type"], "disabled");
    assert!(off.get("output_config").is_none());
}

/// A model that still advertises a token budget keeps extended thinking: the catalog can't tell
/// an extended-only model that happens to support effort (Opus 4.5, which rejects adaptive) from
/// one that takes both, so the mode that works for every such model wins.
#[test]
fn a_model_offering_a_token_budget_keeps_extended_thinking() {
    let model = ModelInfo {
        reasoning: true,
        thinking_controls: ThinkingControls {
            effort: true,
            budget_tokens: true,
            ..Default::default()
        },
        thinking_level_map: HashMap::from([("high".to_string(), Some("high".to_string()))]),
        ..Default::default()
    };
    let request =
        CompletionRequest { max_tokens: Some(64_000), ..turn_request(&model, ThinkingLevel::High) };
    let body = build_request(&request, &model, &<_>::default());
    assert_eq!(body["thinking"]["type"], "enabled");
    assert_eq!(body["thinking"]["budget_tokens"], 16_384);
    assert!(body.get("output_config").is_none());
}

/// Only a thinking-mode rejection is worth a retry, and each rejection teaches exactly one
/// thing: an always-on model loses `disabled`, an adaptive-only model loses `enabled`.
#[test]
fn a_revised_plan_answers_the_rejection_it_was_given() {
    let extended =
        ThinkingPlan { mode: ThinkingMode::Extended, can_disable: true, between_tools: false };
    let adaptive =
        ThinkingPlan { mode: ThinkingMode::Adaptive, can_disable: true, between_tools: false };

    let dropped_extended = "thinking.type.enabled is not supported for this model. Use \
             thinking.type.adaptive and output_config.effort to control thinking behavior";
    assert_eq!(
        extended.revised(dropped_extended),
        Some(ThinkingPlan {
            mode: ThinkingMode::Adaptive,
            can_disable: true,
            between_tools: false
        })
    );
    assert_eq!(
        adaptive.revised("thinking.type.adaptive is not supported for this model"),
        Some(ThinkingPlan {
            mode: ThinkingMode::Extended,
            can_disable: true,
            between_tools: false
        })
    );
    assert_eq!(
        adaptive.revised("thinking.type.disabled is not supported for this model"),
        Some(ThinkingPlan {
            mode: ThinkingMode::Adaptive,
            can_disable: false,
            between_tools: false
        }),
        "an always-on model keeps its mode and stops being told to stop thinking"
    );
    let named = r#"To turn thinking off on this model, send "thinking": {"type": "between_tools"} instead of {"type": "disabled"}."#;
    assert_eq!(
        adaptive.revised(named),
        Some(ThinkingPlan {
            mode: ThinkingMode::Adaptive,
            can_disable: false,
            between_tools: true
        }),
        "a model that names its own off switch gets it"
    );
    let learned = adaptive.revised(named).expect("learned");
    let model = ModelInfo { reasoning: true, ..Default::default() };
    let body = build_anthropic_request(
        &turn_request(&model, ThinkingLevel::Off),
        &model,
        &<_>::default(),
        learned,
        ToolLoading::InPlace,
    );
    assert_eq!(body["thinking"], serde_json::json!({"type": "between_tools"}));
    assert_eq!(learned.revised(named), None, "and keeps it");
    // Anything else is a real error the user needs to see.
    assert_eq!(extended.revised("max_tokens: must be greater than thinking.budget_tokens"), None);
    assert_eq!(
        extended.revised(dropped_extended).and_then(|plan| plan.revised(dropped_extended)),
        None,
        "one flip only"
    );
}

/// With `disabled` off the table, "thinking off" sends no thinking parameter at all — Anthropic's
/// own remedy for the always-on models.
#[test]
fn an_always_on_model_omits_the_thinking_parameter_instead_of_disabling() {
    let model = ModelInfo { reasoning: true, ..Default::default() };
    let plan =
        ThinkingPlan { mode: ThinkingMode::Adaptive, can_disable: false, between_tools: false };
    let body = build_anthropic_request(
        &turn_request(&model, ThinkingLevel::Off),
        &model,
        &<_>::default(),
        plan,
        ToolLoading::InPlace,
    );
    assert!(body.get("thinking").is_none(), "no thinking block at all: {body}");
    assert!(body.get("output_config").is_none());
}

#[test]
fn test_adaptive_ultra_and_unsupported_off_mapping() {
    let req = CompletionRequest {
        model: ModelInfo::default(),
        messages: vec![Message::user("Hello")],
        system: None,
        tools: vec![],
        max_tokens: Some(16_000),
        thinking: ThinkingLevel::Ultra,
        ..Default::default()
    };
    let mut model_info = ModelInfo {
        reasoning: true,
        thinking_level_map: std::collections::HashMap::from([
            ("off".to_string(), None),
            ("minimal".to_string(), None),
            ("low".to_string(), None),
            ("medium".to_string(), None),
            ("high".to_string(), Some("high".to_string())),
        ]),
        ..Default::default()
    };
    let compat = AnthropicCompat { force_adaptive_thinking: true, ..Default::default() };

    let ultra = build_request(&req, &model_info, &compat);
    assert_eq!(ultra["thinking"]["type"], "adaptive");
    assert_eq!(ultra["output_config"]["effort"], "max");

    model_info.thinking_level_map.insert("ultra".to_string(), None);
    let off_request = CompletionRequest { thinking: ThinkingLevel::Off, ..req };
    let clamped = build_request(&off_request, &model_info, &compat);
    assert_eq!(clamped["thinking"]["type"], "adaptive");
    assert_eq!(clamped["output_config"]["effort"], "high");
}

#[test]
fn test_serialize_thinking_block() {
    let msg = Message {
        role: luxe_core::message::Role::Assistant,
        content: vec![luxe_core::message::ContentBlock::Thinking {
            text: "Let me think...".to_string(),
            signature: Some("sig123".to_string()),
            id: None,
            encrypted: None,
            opaque: None,
        }],
    };
    let serialized = serialize_content_blocks(&msg.content);
    assert_eq!(serialized[0]["type"], "thinking");
    assert_eq!(serialized[0]["thinking"], "Let me think...");
    assert_eq!(serialized[0]["signature"], "sig123");
}

#[test]
fn thinking_without_signature_is_dropped_from_replay() {
    // A signature-less reasoning block can't be replayed safely (Anthropic 400s on an
    // invalid/empty signature), so it's omitted entirely — this also skips reasoning that
    // originated from another provider before a mid-session model switch.
    let content = vec![
        luxe_core::message::ContentBlock::Thinking {
            text: "unsigned".to_string(),
            signature: None,
            id: None,
            encrypted: None,
            opaque: None,
        },
        luxe_core::message::ContentBlock::Text("answer".to_string()),
    ];
    let serialized = serialize_content_blocks(&content);
    assert_eq!(serialized.len(), 1, "the unsigned thinking block is dropped");
    assert_eq!(serialized[0]["type"], "text");
}

/// The block this adapter drops can be the only one a message had. What is left is an empty
/// `content`, which the API rejects — and since the message is history, it rejects every
/// later request too, so the drop has to take the message with it.
#[test]
fn a_turn_left_with_no_blocks_is_dropped_rather_than_sent_empty() {
    let messages = vec![
        Message::user("go"),
        Message {
            role: luxe_core::message::Role::Assistant,
            content: vec![luxe_core::message::ContentBlock::Thinking {
                text: "reasoning from another provider".to_string(),
                signature: None,
                id: Some("rs_1".to_string()),
                encrypted: Some("opaque".to_string()),
                opaque: None,
            }],
        },
        Message::user("carry on"),
    ];

    let serialized = serialize_messages(&messages, false);

    assert_eq!(serialized.len(), 1, "the emptied turn is gone and the two users merge");
    let content = serialized[0]["content"].as_array().expect("content array");
    assert_eq!(content.len(), 2);
    assert!(content.iter().all(|block| block["text"].as_str().is_some_and(|t| !t.is_empty())));
}

#[test]
fn redacted_thinking_round_trips_verbatim() {
    let content = vec![luxe_core::message::ContentBlock::RedactedThinking {
        data: "ENCRYPTED==".to_string(),
    }];
    let serialized = serialize_content_blocks(&content);
    assert_eq!(serialized[0]["type"], "redacted_thinking");
    assert_eq!(serialized[0]["data"], "ENCRYPTED==");
}
