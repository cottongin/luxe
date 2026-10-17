//! Support — the request builder and the canned turn the adapter tests share.

use super::*;

/// Builds a body the way the adapter does: the thinking encoding follows from the model's own
/// declared controls, so a test only has to describe the model.
pub(super) fn build_request(
    req: &CompletionRequest,
    model: &ModelInfo,
    compat: &AnthropicCompat,
) -> serde_json::Value {
    build_anthropic_request(
        req,
        model,
        compat,
        ThinkingPlan::for_model(model, compat),
        ToolLoading::InPlace,
    )
}

/// A minimal SSE turn that streams `text` and stops.
pub(super) fn sse_turn(text: &str) -> String {
    format!(
        "event: content_block_delta\n\
             data: {{\"type\": \"content_block_delta\", \"delta\": {{\"type\": \"text_delta\", \"text\": \"{text}\"}}}}\n\n\
             event: message_stop\n\
             data: {{\"type\": \"message_stop\"}}\n"
    )
}

/// A request against `model`, at `thinking`.
pub(super) fn turn_request(model: &ModelInfo, thinking: ThinkingLevel) -> CompletionRequest {
    CompletionRequest {
        model: model.clone(),
        messages: vec![Message::user("ping")],
        system: None,
        tools: Vec::new(),
        thinking,
        max_tokens: Some(20_000),
        ..Default::default()
    }
}
