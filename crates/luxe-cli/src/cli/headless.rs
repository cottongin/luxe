//! Headless (non-interactive) execution — single-prompt runs with streamed text or a JSONL
//! event stream for scripts and benchmarks.

use super::args::OutputFormat;
use super::tui::shutdown_workspace_services;
use crate::output::{out, outln};

/// Run headless mode: drive a single prompt through the engine to completion, rendering the
/// event stream in the requested `format` (streamed text, or a JSONL event stream with usage).
pub(crate) async fn run_headless(
    prompt: &str,
    settings: crate::engine_setup::AgentSettings,
    format: OutputFormat,
) -> anyhow::Result<()> {
    use luxe_core::engine::Action;

    // A one-shot run has nobody to ask, so it says what is missing instead of starting an engine
    // that would refuse the turn a moment later. The interactive front-ends do the opposite: they
    // open, and choosing a model is one of the things you do there.
    settings.require_model()?;

    let crate::engine_setup::EngineHandle {
        actions: engine_tx, events: mut event_rx, closer, ..
    } = crate::engine_setup::build_engine(
        &settings,
        luxe_core::HookRegistry::new(),
        // Nobody to prompt, so every `ask` collapses to one answer — `--on-ask`, allow unless the
        // run asked to refuse. A `deny` rule is enforced either way.
        settings.unattended_ask(),
    )
    .await?;

    if let Some(level) = settings.initial_thinking {
        engine_tx
            .send(Action::SetThinkingLevel(level))
            .await
            .map_err(|e| anyhow::anyhow!("engine channel closed: {e}"))?;
    }

    engine_tx
        .send(Action::SubmitUserMessage {
            content: prompt.to_string(),
            images: vec![],
            remediation: None,
        })
        .await
        .map_err(|e| anyhow::anyhow!("engine channel closed: {e}"))?;

    let outcome = match format {
        OutputFormat::Text => stream_headless_text(&mut event_rx).await,
        OutputFormat::Json => stream_headless_json(&mut event_rx).await,
    };

    closer.close(&engine_tx).await;
    // A one-shot headless run must terminate promptly: gracefully shut down the warm LSP/DAP
    // servers (diagnostics-on-edit spawns rust-analyzer) so their child processes + reader tasks
    // don't keep the runtime alive after the turn (the TUI does the equivalent on quit).
    shutdown_workspace_services().await;
    finish(outcome)
}

/// How a headless turn ended, which is what the process's exit status says.
#[derive(Debug)]
pub(crate) enum TurnOutcome {
    /// The turn ended, however it ended, or the reader stopped listening
    Finished,
    /// The engine reported an error, already printed
    Failed(String),
    /// The engine went away before the turn ended
    Disconnected,
}

/// A failure the run has already printed, so the process exits 1 without saying it again.
#[derive(Debug)]
pub(crate) struct AlreadyReported(String);

impl std::fmt::Display for AlreadyReported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for AlreadyReported {}

/// The run's result for `outcome`: only a finished turn is a success.
fn finish(outcome: TurnOutcome) -> anyhow::Result<()> {
    match outcome {
        TurnOutcome::Finished => Ok(()),
        TurnOutcome::Failed(message) => Err(AlreadyReported(message).into()),
        TurnOutcome::Disconnected => anyhow::bail!("the engine stopped before the turn ended"),
    }
}

/// Human-friendly headless rendering: assistant text to stdout, tool activity to stderr.
pub(crate) async fn stream_headless_text(
    event_rx: &mut tokio::sync::mpsc::Receiver<luxe_core::event::Event>,
) -> TurnOutcome {
    use luxe_core::event::Event;
    while let Some(event) = event_rx.recv().await {
        // A reader that stopped — `luxe --headless -p ... | head -1` — ends the run here rather
        // than driving a whole turn into a closed pipe.
        if !crate::output::is_open() {
            return TurnOutcome::Finished;
        }
        match event {
            Event::TokenDelta { delta, .. } => out!("{delta}"),
            // Same rule the TUI follows: our own registry bookkeeping is not progress anyone
            // asked to watch. The `json` stream below is a machine's view and keeps every event.
            Event::ToolStarted { name, .. } if !luxe_core::activity::is_bookkeeping(&name) => {
                eprintln!("\n[tool: {name}]");
            }
            Event::ToolEnded { is_error, .. } if is_error => eprintln!("[tool failed]"),
            Event::Notice(msg) => eprintln!("\n[{msg}]"),
            Event::ProviderRetrying { attempt, attempts, reason } => {
                eprintln!("\n[retrying ({attempt}/{attempts}): {reason}]");
            }
            Event::Error(msg) => {
                eprintln!("\nerror: {msg}");
                return TurnOutcome::Failed(msg);
            }
            Event::TurnEnded { .. } => {
                outln!();
                return TurnOutcome::Finished;
            }
            _ => {}
        }
    }
    TurnOutcome::Disconnected
}

/// Machine-readable headless rendering: one JSON object per line (JSONL). Emits assistant
/// messages, tool start/end, per-provider-call `usage`, and a terminal `turn_end`/`error`.
/// Consumed by scripts, harness adapters, and the benchmark rig. `usage` carries what the provider
/// reported for one call (input, output, cache read and write, reasoning); sum the `usage` events
/// across a turn for the total.
pub(crate) async fn stream_headless_json(
    event_rx: &mut tokio::sync::mpsc::Receiver<luxe_core::event::Event>,
) -> TurnOutcome {
    use luxe_core::event::Event;

    while let Some(event) = event_rx.recv().await {
        if !crate::output::is_open() {
            return TurnOutcome::Finished;
        }
        if let Some(value) = headless_json_event(&event) {
            outln!("{value}");
        }
        // Error/TurnEnded are terminal: their line is out, and nothing after them is read
        match event {
            Event::Error(message) => return TurnOutcome::Failed(message),
            Event::TurnEnded { .. } => return TurnOutcome::Finished,
            _ => {}
        }
    }
    TurnOutcome::Disconnected
}

/// Map one engine `Event` to its headless-JSON line, or `None` for events we don't surface.
///
/// Pure (no IO) so the JSONL contract is unit-testable without a live engine. `usage` carries
/// exactly what the provider reported for one call; consumers sum them across a turn.
pub(crate) fn headless_json_event(event: &luxe_core::event::Event) -> Option<serde_json::Value> {
    use luxe_core::event::Event;
    use luxe_core::message::{ContentBlock, Role};

    match event {
        Event::MessageCommitted { message, .. } if message.role == Role::Assistant => {
            let text: String = message
                .content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text(t) => Some(t.as_str()),
                    _ => None,
                })
                .collect();
            let tool_calls: Vec<&str> = message
                .content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::ToolCall { name, .. } => Some(name.as_str()),
                    _ => None,
                })
                .collect();
            Some(serde_json::json!({
                "type": "message", "role": "assistant", "text": text, "tool_calls": tool_calls,
            }))
        }
        Event::ToolStarted { call_id, name } => {
            Some(serde_json::json!({"type": "tool_start", "call_id": call_id, "name": name}))
        }
        Event::ToolEnded { call_id, is_error } => {
            Some(serde_json::json!({"type": "tool_end", "call_id": call_id, "is_error": is_error}))
        }
        Event::TurnUsage {
            prompt_tokens,
            completion_tokens,
            cost_usd,
            cache_read_tokens,
            cache_write_tokens,
            reasoning_tokens,
            source,
        } => Some(serde_json::json!({
            "type": "usage",
            "input": prompt_tokens,
            "output": completion_tokens,
            "total": prompt_tokens + completion_tokens,
            "cost_usd": cost_usd,
            "cache_read": cache_read_tokens,
            "cache_write": cache_write_tokens,
            "reasoning": reasoning_tokens,
            "source": source.label(),
        })),
        // What the subscription has left, when the provider says. Scripts watching a plan need
        // this more than they need a dollar equivalent.
        Event::PlanUsage(plan) => Some(serde_json::json!({
            "type": "plan_usage",
            "plan": plan.plan,
            "summary": plan.summary(),
            "primary_used_percent": plan.primary.map(|window| window.used_percent),
            "primary_window_minutes": plan.primary.map(|window| window.window_minutes),
            "primary_resets_in_secs": plan.primary.and_then(|window| window.resets_in_secs),
            "secondary_used_percent": plan.secondary.map(|window| window.used_percent),
            "credits_balance": plan.credits_balance,
        })),
        // The running total, so a script does not have to re-derive one from the per-call lines
        Event::SessionUsage(usage) => Some(serde_json::json!({
            "type": "session_usage",
            "calls": usage.total.calls,
            "input": usage.total.prompt_tokens,
            "output": usage.total.completion_tokens,
            "cache_read": usage.total.cache_read_tokens,
            "cache_write": usage.total.cache_write_tokens,
            "reasoning": usage.total.reasoning_tokens,
            "cost_usd": usage.total.cost_usd,
            "unpriced_calls": usage.total.unpriced_calls,
            // Subscription usage is reported separately from spend: `cost_usd` is money charged,
            // `plan_usd` is what plan-billed tokens would have cost on the API.
            "plan_usd": usage.total.plan_usd,
            "plan_calls": usage.total.plan_calls,
            "by_source": usage
                .by_source
                .iter()
                .map(|entry| serde_json::json!({
                    "source": entry.source.label(),
                    "calls": entry.totals.calls,
                    "input": entry.totals.prompt_tokens,
                    "output": entry.totals.completion_tokens,
                    "cost_usd": entry.totals.cost_usd,
                    "plan_usd": entry.totals.plan_usd,
                }))
                .collect::<Vec<_>>(),
        })),
        Event::Notice(msg) => Some(serde_json::json!({"type": "notice", "message": msg})),
        Event::ProviderRetrying { attempt, attempts, reason } => Some(serde_json::json!({
            "type": "retrying",
            "attempt": attempt,
            "attempts": attempts,
            "reason": reason,
        })),
        Event::Error(msg) => Some(serde_json::json!({"type": "error", "message": msg})),
        Event::TurnEnded { reason } => {
            Some(serde_json::json!({"type": "turn_end", "reason": turn_end_reason(reason)}))
        }
        _ => None,
    }
}

/// The `reason` field of a `turn_end` line.
///
/// `EndTurn` and `ToolCall` keep the spelling scripts already parse; an `Other` reason is
/// unwrapped to the reason itself rather than rendered as `Other("cancelled")`, which is what
/// `{:?}` produces and no reader wants in a JSON field. A turn reports why it *actually* stopped.
fn turn_end_reason(reason: &luxe_core::event::StopReason) -> String {
    match reason {
        luxe_core::event::StopReason::Other(reason) => reason.clone(),
        named => format!("{named:?}"),
    }
}

#[cfg(test)]
mod tests;
