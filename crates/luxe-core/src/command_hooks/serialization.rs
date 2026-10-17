//! Hook serialization — encode the event before starting its command.

use serde::Serialize;

use super::{run_serialized_hook, timeout_outcome, CommandHookSpec};
use crate::hook::{HookEvent, HookOutcome};

/// Execute a command hook and return the outcome.
pub async fn run_command_hook(spec: &CommandHookSpec, event: &HookEvent) -> HookOutcome {
    run_event_hook(spec, event).await
}

async fn run_event_hook(spec: &CommandHookSpec, event: &impl Serialize) -> HookOutcome {
    let event_json = match serde_json::to_string(event) {
        Ok(event_json) => event_json,
        Err(error) => {
            tracing::error!(%error, "failed to serialize the command hook event");
            return timeout_outcome(
                spec,
                format!("command hook event serialization failed: {error}"),
            );
        }
    };
    run_serialized_hook(spec, event_json).await
}

#[cfg(test)]
mod tests;
