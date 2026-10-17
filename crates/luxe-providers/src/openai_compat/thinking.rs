//! Thinking — how each chat-completions dialect asks for reasoning.

use luxe_core::provider::{CompletionRequest, ThinkingLevel};

use crate::config::CompatFlags;

pub(super) fn apply_thinking_config(
    body: &mut serde_json::Value,
    req: &CompletionRequest,
    compat: &CompatFlags,
) {
    if !req.model.reasoning {
        return;
    }

    let level = req.model.effective_thinking_level(req.thinking);
    let enabled = level != ThinkingLevel::Off;
    let effort = req.model.thinking_level_value(level).unwrap_or_else(|| match level {
        ThinkingLevel::Off => "none",
        ThinkingLevel::Ultra => "max",
        _ => level.as_str(),
    });

    match compat.thinking_format.as_deref() {
        Some("qwen") => body["enable_thinking"] = enabled.into(),
        Some("qwen-chat-template" | "chat-template" | "chat_template") => {
            if let Some(kwargs) =
                crate::chat_template::kwargs(&compat.chat_template_kwargs, enabled, effort)
            {
                body["chat_template_kwargs"] = kwargs;
            }
        }
        Some("deepseek") => {
            body["thinking"] = serde_json::json!({
                "type": if enabled { "enabled" } else { "disabled" },
            });
            if enabled && compat.supports_reasoning_effort {
                body["reasoning_effort"] = effort.into();
            }
        }
        Some("openrouter") => {
            // A fifth of OpenRouter's catalog answers "Reasoning is mandatory for this endpoint"
            // to `effort: "none"` *and* to `enabled: false`, and off is the level a session starts
            // on — so those models would 400 on every turn. Omitting the key is the only spelling
            // that always works, and the catalog is trusted to say off is reachable before we ask
            // for it.
            if enabled || req.model.thinking_controls.can_disable {
                body["reasoning"] = serde_json::json!({"effort": effort});
            }
        }
        Some("together") => {
            body["reasoning"] = serde_json::json!({"enabled": enabled});
            if enabled && compat.supports_reasoning_effort {
                body["reasoning_effort"] = effort.into();
            }
        }
        Some("openai") => body["reasoning_effort"] = effort.into(),
        // `none` only where the model says it can stop thinking: Copilot answers it with a 400
        // for every model that cannot, and leaving the key out gets the model's own default
        None if compat.supports_reasoning_effort
            && (enabled || req.model.thinking_controls.can_disable) =>
        {
            body["reasoning_effort"] = effort.into();
        }
        _ => {}
    }
}
