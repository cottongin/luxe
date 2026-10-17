// Loop control tool — direct the autonomous development loop (progress/complete/blocked).
//
// This tool is called by the agent to drive the unattended loop: signal phase completion,
// trigger verification on all phases done, or pause for human input when blocked.

use async_trait::async_trait;
use luxe_core::hook::{HookEvent, LoopControl};
use luxe_core::provider::ToolSchema;
use luxe_core::tool::{Tool, ToolContext, ToolResultData};
use serde_json::json;

/// The `loop_control` tool — direct the autonomous loop.
pub struct LoopControlTool;

impl Default for LoopControlTool {
    fn default() -> Self {
        Self::new()
    }
}

impl LoopControlTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for LoopControlTool {
    fn name(&self) -> &str {
        "loop_control"
    }

    /// No deadline of its own: this call *is* the phase gate, and the gate is already bounded.
    ///
    /// A claim runs the acceptance commands (each capped by `gate.timeout_secs`, ten minutes by
    /// default) and then a reviewer turn (24 tool calls, ten minutes). The default 120-second tool
    /// deadline is shorter than either, so on a real project the claim timed out while its own gate
    /// was still running — the phase advanced, the reviewer's verdict was written to the log, and
    /// the agent was handed a timeout error instead of the answer. A refusal that never reaches the
    /// model is a refusal that teaches it nothing.
    fn timeout(&self) -> Option<std::time::Duration> {
        None
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "loop_control".to_string(),
            description: "Direct the autonomous development loop: progress to next phase, complete all phases, or pause when blocked.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["progress", "complete", "blocked"],
                        "description": "Loop control action."
                    },
                    "phase": {
                        "type": "integer",
                        "description": "The phase you are claiming complete, for the record. The loop advances by exactly one when the gate agrees; it does not jump to a number you name (a human sets that with `luxe loop phase <n>`)."
                    },
                    "reason": {
                        "type": "string",
                        "description": "Reason for blocking (for blocked action)."
                    }
                },
                "required": ["action"]
            }),
        }
    }

    async fn run(&self, args: serde_json::Value, cx: &ToolContext) -> ToolResultData {
        let action = match args.get("action").and_then(|v| v.as_str()) {
            Some(a) => a,
            None => {
                return ToolResultData::error(
                    "Error: 'action' argument is required (progress/complete/blocked)".to_string(),
                );
            }
        };

        let loop_control = match action {
            "progress" => {
                let phase = args.get("phase").and_then(|v| v.as_u64()).map(|n| n as u32);
                LoopControl::Progress { phase }
            }
            "complete" => LoopControl::Complete,
            "blocked" => {
                let reason = args
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("no reason given")
                    .to_string();
                LoopControl::Blocked { reason }
            }
            other => {
                return ToolResultData::error(format!(
                    "Unknown action: {other}. Use progress/complete/blocked."
                ));
            }
        };

        // Emit the HookEvent to the hook registry (if available).
        let result_content = if let Some(hook_ctx) = &cx.hook_ctx {
            let event = HookEvent::LoopControl(loop_control);
            let outcome = hook_ctx
                .hook_registry
                .invoke(luxe_core::hook::HookPoint::LoopControl, &event, hook_ctx)
                .await;
            // Log outcome but don't fail — hooks may return Continue.
            tracing::debug!(?outcome, "loop_control hook outcome");
            // Propagate the hook's message/reason back to the agent.
            match outcome {
                luxe_core::hook::HookOutcome::ContinueLoop { message } => {
                    vec![luxe_core::ContentBlock::Text(message)]
                }
                luxe_core::hook::HookOutcome::StopLoop { reason } => {
                    vec![luxe_core::ContentBlock::Text(format!("Loop stopped: {reason}"))]
                }
                luxe_core::hook::HookOutcome::Continue => {
                    vec![luxe_core::ContentBlock::Text(format!("Loop control: {action}"))]
                }
                _ => {
                    vec![luxe_core::ContentBlock::Text(format!("Loop control: {action}"))]
                }
            }
        } else {
            vec![luxe_core::ContentBlock::Text(format!("Loop control: {action}"))]
        };

        ToolResultData { content: result_content, is_error: false }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A claim outlives the gate it triggers, or the gate's answer never gets back.
    ///
    /// The break this catches, measured on a live loop: `loop_control` inherited the 120-second
    /// default while its own gate ran a test suite and a reviewer turn, each bounded in minutes. The
    /// call was cancelled mid-gate — the phase still advanced, and the developer was told only that
    /// its tool had timed out.
    #[test]
    fn a_phase_claim_outlasts_the_gate_it_runs() {
        assert!(
            LoopControlTool::new().timeout().is_none(),
            "the gate's own bounds are the deadline here"
        );
    }
}
