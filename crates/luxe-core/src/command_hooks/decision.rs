//! Hook decisions — typed policy answers and the fail-safe for unknown values.

use serde::{Deserialize, Serialize};

use super::{timeout_outcome, CommandHookSpec, HookDecision};
use crate::hook::HookOutcome;

/// Policy actions a command hook can return in its `decision` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HookDecisionKind {
    /// Continue into the permission rules without approving the call.
    Allow,
    /// Refuse the call or message.
    Deny,
    /// Alternate spelling for a refusal.
    Block,
    /// Ask for confirmation at BeforeToolUse.
    Ask,
    /// An unsupported answer that must use the hook's configured fail-safe.
    #[serde(other)]
    Unknown,
}

impl HookDecision {
    pub(super) fn resolve(self, spec: &CommandHookSpec) -> HookOutcome {
        if self.decision == HookDecisionKind::Unknown {
            tracing::warn!(command = ?spec.command, "command hook returned an unrecognized decision");
            return timeout_outcome(spec, "command hook returned an unrecognized decision".into());
        }
        self.into_outcome(spec.point)
    }
}
