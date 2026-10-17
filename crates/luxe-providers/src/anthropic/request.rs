//! Anthropic Messages request building — core messages → Anthropic wire JSON.

use super::*;
use std::collections::HashSet;

mod cache;
mod messages;

pub(crate) use cache::add_cache_control_markers;
use cache::cache_marker;
pub(crate) use messages::{find_system_message, serialize_messages_loading};
#[cfg(test)]
pub(crate) use messages::{serialize_content_blocks, serialize_messages};

/// How a request expresses a non-off thinking level on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ThinkingMode {
    /// `thinking: {type: "adaptive"}` + `output_config.effort` — Claude 4.7 and later.
    Adaptive,
    /// `thinking: {type: "enabled", budget_tokens: N}` — Claude 4.5 and earlier.
    Extended,
}

/// The thinking encoding one request uses: how a level is expressed, and whether thinking may be
/// switched off at all.
///
/// Derived from the model's declared controls, and revised when Anthropic rejects it (see
/// [`ThinkingPlan::revised`]) so a model the catalog doesn't know yet still lands on a working
/// request after one retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ThinkingPlan {
    /// Encoding for a level above `off`
    pub(crate) mode: ThinkingMode,
    /// Whether `thinking: {type: "disabled"}` is accepted; the newest models are always-on
    pub(crate) can_disable: bool,
    /// Whether "off" is `thinking: {type: "between_tools"}`: the newest models think unless told
    /// not to, refuse `disabled`, and name this as the way to stop
    pub(crate) between_tools: bool,
}

impl ThinkingPlan {
    /// The encoding a model's declared reasoning controls call for.
    ///
    /// Effort-only models (the adaptive-only generation) reject extended thinking outright;
    /// anything that still advertises a token budget keeps it, because that's the only mode the
    /// older models accept and the catalog can't tell an extended-only model that happens to
    /// support effort from one that also took adaptive.
    pub(crate) fn for_model(model: &ModelInfo, compat: &AnthropicCompat) -> Self {
        let adaptive = compat.force_adaptive_thinking || model.thinking_controls.effort_only();
        let mode = if adaptive { ThinkingMode::Adaptive } else { ThinkingMode::Extended };
        Self { mode, can_disable: true, between_tools: false }
    }

    /// The plan to retry with when `body` is Anthropic rejecting this one, or `None` when the 400
    /// is about something else and there is nothing to learn.
    pub(crate) fn revised(self, body: &str) -> Option<Self> {
        // "To turn thinking off on this model, send {type: between_tools} instead of {type:
        // disabled}": the remedy is named, and omitting the block would leave the model thinking
        if self.can_disable && body.contains("between_tools") && body.contains("disabled") {
            return Some(Self { can_disable: false, between_tools: true, ..self });
        }
        if self.can_disable && rejects(body, "disabled") {
            return Some(Self { can_disable: false, ..self });
        }
        match self.mode {
            ThinkingMode::Extended if rejects(body, "enabled") => {
                Some(Self { mode: ThinkingMode::Adaptive, ..self })
            }
            ThinkingMode::Adaptive if rejects(body, "adaptive") => {
                Some(Self { mode: ThinkingMode::Extended, ..self })
            }
            _ => None,
        }
    }
}

/// Whether `body` is Anthropic naming `thinking.type.<what>` as the value it rejected.
///
/// The complaint follows the rejected value and the *suggested* replacement follows the complaint
/// ("thinking.type.enabled is not supported … Use thinking.type.adaptive …"), so the match has to
/// stay local to each mention — scanning the whole body would read the remedy as a second rejection
/// and flip the encoding straight back.
fn rejects(body: &str, what: &str) -> bool {
    const COMPLAINT_WINDOW: usize = 40;
    let needle = format!("thinking.type.{what}");
    body.match_indices(&needle).any(|(at, _)| {
        body[at + needle.len()..]
            .chars()
            .take(COMPLAINT_WINDOW)
            .collect::<String>()
            .contains("not supported")
    })
}

/// How a deferred tool reaches the model once `activate_tools` asks for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolLoading {
    /// In `tools`, which the engine has already grown by the activated ones
    Listed,
    /// Sent with `defer_loading` and expanded where the activation happened, by `tool_reference`
    InPlace,
}

/// Whether `body` is a 400 about in-place tool loading itself, so listing the tools instead would
/// get the request through.
///
/// The phrases are the ones Anthropic uses (`defer_loading`, "Tool reference 'x' not found",
/// "Tool definitions … cannot be mixed with other content") plus the block's own type name, which
/// is what a proxy that doesn't know it tends to quote back.
pub(crate) fn refuses_in_place_loading(body: &str) -> bool {
    ["defer_loading", "tool_reference", "Tool reference", "Tool definitions"]
        .iter()
        .any(|phrase| body.contains(phrase))
}

/// The deferred tools one request loads in place, by name; empty when it lists them instead.
///
/// In place only when there's something to load and something left in the list: Anthropic
/// requires at least one tool that isn't deferred, and a pass that withholds tools sends no
/// deferred ones either, which also keeps a `tool_reference` from pointing at nothing.
fn in_place_names(req: &CompletionRequest, loading: ToolLoading) -> HashSet<&str> {
    if loading != ToolLoading::InPlace {
        return HashSet::new();
    }
    let deferred: HashSet<&str> = req.deferred_tools.iter().map(|t| t.name.as_str()).collect();
    let listed_any = req.tools.iter().any(|tool| !deferred.contains(tool.name.as_str()));
    if listed_any {
        deferred
    } else {
        HashSet::new()
    }
}

/// Build an Anthropic Messages API request body.
pub(crate) fn build_anthropic_request(
    req: &CompletionRequest,
    model_info: &ModelInfo,
    compat: &AnthropicCompat,
    plan: ThinkingPlan,
    loading: ToolLoading,
) -> serde_json::Value {
    let in_place = in_place_names(req, loading);
    let mut body = serde_json::json!({
        "model": model_info.r#ref.id.clone(),
        "max_tokens": req.max_tokens.unwrap_or(4096),
        "messages": serialize_messages_loading(
            &req.messages,
            req.model.input.contains(&luxe_core::provider::Modality::Image),
            &in_place,
        ),
    });

    // The engine puts the system prompt in `CompletionRequest.system`; a `System`-role *message*
    // is only how an embedder might hand one over. Reading the message alone silently shipped
    // every Anthropic request with no system prompt at all.
    if let Some(system) =
        req.system.clone().or_else(|| find_system_message(&req.messages)).filter(|s| !s.is_empty())
    {
        body["system"] = serde_json::Value::String(system);
    }

    if !req.tools.is_empty() {
        let tool_json = |t: &luxe_core::provider::ToolSchema| {
            let mut tool = serde_json::json!({
                "name": t.name,
                "description": t.description,
                "input_schema": t.parameters,
            });
            // Per tool, not request-wide: unvalidated streaming garbles `edit`
            if compat.stream_tool_arguments && luxe_core::activity::streams_arguments(&t.name) {
                tool["eager_input_streaming"] = serde_json::Value::Bool(true);
            }
            tool
        };
        // The listed tools first and the deferred ones after, all in the order the engine sorted
        // them. A deferred definition is outside the cached prefix, so the list is the same bytes
        // before and after an activation and the prefix holds.
        let listed =
            req.tools.iter().filter(|t| !in_place.contains(t.name.as_str())).map(tool_json);
        let deferred =
            req.deferred_tools.iter().filter(|_| !in_place.is_empty()).map(tool_json).map(
                |mut tool| {
                    tool["defer_loading"] = serde_json::Value::Bool(true);
                    tool
                },
            );
        body["tools"] = serde_json::json!(listed.chain(deferred).collect::<Vec<_>>());
    }

    let (level, effort) = resolve_anthropic_thinking(model_info, req.thinking, compat);
    match (level, plan.mode) {
        // Off carries no `output_config`, which also keeps us clear of the newer rule that rejects
        // a disabled block combined with xhigh/max effort.
        (ThinkingLevel::Off, _) if plan.can_disable => {
            body["thinking"] = serde_json::json!({"type": "disabled"});
        }
        // The newest models stop thinking only when told `between_tools`
        (ThinkingLevel::Off, _) if plan.between_tools => {
            body["thinking"] = serde_json::json!({"type": "between_tools"});
        }
        // Always-on models reject `disabled`; omitting the parameter is Anthropic's own remedy.
        (ThinkingLevel::Off, _) => {}
        (_, ThinkingMode::Adaptive) => {
            // `display: "summarized"` because the newest models omit thinking text by default and
            // Luxe shows reasoning in the transcript.
            body["thinking"] = serde_json::json!({"type": "adaptive", "display": "summarized"});
            body["output_config"] = serde_json::json!({"effort": effort});
        }
        (_, ThinkingMode::Extended) => {
            body["thinking"] = serde_json::json!({
                "type": "enabled",
                "budget_tokens": manual_thinking_budget(level, req.max_tokens.unwrap_or(4096)),
            });
        }
    }

    // A one-off prompt is never read back, so a breakpoint on it only buys the write premium.
    if compat.cache_control && req.reuse.worth_caching() {
        add_cache_control_markers(&mut body, cache_marker(compat), req.shared_system_len);
    }

    body
}

/// Whether a request at `requested` goes out with thinking on, once the model's own limits apply.
pub(crate) fn thinking_is_on(
    model: &ModelInfo,
    requested: ThinkingLevel,
    compat: &AnthropicCompat,
) -> bool {
    resolve_anthropic_thinking(model, requested, compat).0 != ThinkingLevel::Off
}

/// The level this model will actually honor, plus the effort value that names it.
///
/// Anthropic models always reason, so a missing `reasoning` flag never silences thinking here.
fn resolve_anthropic_thinking(
    model: &ModelInfo,
    requested: ThinkingLevel,
    compat: &AnthropicCompat,
) -> (ThinkingLevel, String) {
    let mut capabilities = model.clone();
    capabilities.reasoning = true;
    // Forcing adaptive by hand is how an endpoint the catalog knows nothing about says "this is a
    // current model", so grant it the top level the mode implies.
    if compat.force_adaptive_thinking
        && !capabilities.thinking_level_map.contains_key("ultra")
        && !capabilities.thinking_level_map.contains_key("max")
    {
        capabilities.thinking_level_map.insert("ultra".to_string(), Some("max".to_string()));
    }

    let level = capabilities.effective_thinking_level(requested);
    let value = capabilities
        .thinking_level_value(level)
        .unwrap_or(match level {
            // Anthropic has no `minimal` effort; the floor is `low`.
            ThinkingLevel::Off | ThinkingLevel::Minimal | ThinkingLevel::Low => "low",
            ThinkingLevel::Medium => "medium",
            ThinkingLevel::High => "high",
            ThinkingLevel::XHigh => "xhigh",
            ThinkingLevel::Ultra => "max",
        })
        .to_string();
    (level, value)
}

/// The token budget a level asks for, reduced to fit under `max_tokens` when the cap is too small
/// to honor it.
///
/// A reduced budget leaves the reply a real share of the cap — half of it, up to 16k — because
/// thinking and answer draw on the same `max_tokens`. Shrinking only to the 1,024-token floor gave
/// XHigh on a 32k cap a 30,720 budget and the answer about 1,200 tokens, so a model that used its
/// budget was cut off mid-reply or mid-tool-call, and the error told the user to raise a cap that
/// was already the model's maximum.
///
/// The reduction is snapped to a coarse step for a reason that has nothing to do with thinking:
/// Anthropic invalidates the cached *messages* prefix whenever `budget_tokens` changes, and
/// `max_tokens` is itself clamped against a prompt that grows every turn. A budget read straight
/// off it would therefore drift by a few hundred tokens per call and re-bill the whole
/// conversation as a cache write each time.
fn manual_thinking_budget(level: ThinkingLevel, max_tokens: u32) -> u32 {
    /// Anthropic's own floor for a thinking budget, and the granularity the reduction moves in.
    const FLOOR: u32 = 1_024;
    /// The most a squeezed budget holds back for the answer.
    const ANSWER_RESERVE: u32 = 16_384;

    let requested = match level {
        ThinkingLevel::Off => 0,
        ThinkingLevel::Minimal => FLOOR,
        ThinkingLevel::Low => 2_048,
        ThinkingLevel::Medium => 8_192,
        ThinkingLevel::High => 16_384,
        ThinkingLevel::XHigh => 32_768,
        ThinkingLevel::Ultra => 65_536,
    };
    let reserve = (max_tokens / 2).clamp(FLOOR, ANSWER_RESERVE);
    let room = max_tokens.saturating_sub(reserve).max(1);
    if requested <= room {
        return requested;
    }
    (room / FLOOR * FLOOR).clamp(1, room)
}

#[cfg(test)]
mod tests;
