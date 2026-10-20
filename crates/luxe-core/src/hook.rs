//! Hook trait + registry — cross-cutting behavior injection points.
//!
//! Hooks wrap the agent loop at defined points (before/after tool use, session start/end,
//! etc.). They can observe, modify, or intercept behavior via `HookOutcome`.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Hook points in the agent loop lifecycle.
///
/// These names are a **published vocabulary**: they are the keys a user writes in `hooks.json`,
/// so renaming one silently unhooks everybody who was using it and no compiler catches that.
/// [`HookPoint::ALL`] and `the_hook_vocabulary_is_pinned` are what make such a rename fail loudly.
/// Adding a variant is free — add it to `ALL` so it becomes configurable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HookPoint {
    /// Session is starting (created or resumed).
    SessionStart,
    /// Session is ending (closed or replaced).
    SessionEnd,
    /// Before the agent starts processing a user prompt.
    BeforeAgentStart,
    /// User has submitted a prompt (before it reaches the agent).
    UserPromptSubmit,
    /// Agent is about to call the provider.
    AgentStart,
    /// Agent turn has completed (success or failure).
    AgentEnd,
    /// About to use a tool.
    BeforeToolUse,
    /// Tool has finished (success or failure).
    AfterToolUse,
    /// Assistant message is starting to stream.
    MessageStart,
    /// Assistant message has finished streaming.
    MessageEnd,
    /// About to compact the session.
    BeforeCompact,
    /// Compaction has finished.
    AfterCompact,
    /// A file has been changed by a tool.
    FileChanged,
    /// Notification event (e.g., from a skill or external source).
    Notification,
    /// An error has occurred.
    Error,
    /// Review gate: runs acceptance tests and returns PASS/FAIL/INVALID verdict.
    ReviewGate,
    /// Loop control directive received (progress/complete/blocked).
    LoopControl,
    /// A permission prompt is now on screen and the turn is blocked awaiting the user's decision.
    /// Observational: the decision is already gated by the permission engine, so a hook's outcome
    /// here does not change the prompt (unlike `BeforeToolUse`). Fires only when a front-end can
    /// actually prompt (interactive `ask`) — never under headless/loop auto-allow/deny.
    PermissionRequested,
    /// The permission prompt was answered (approved/rejected) or cancelled; the turn resumes.
    /// The observational counterpart to [`HookPoint::PermissionRequested`].
    PermissionResolved,
}

impl HookPoint {
    /// Every hook point a `hooks.json` may name, in lifecycle order.
    ///
    /// The source of truth for what is configurable. A variant missing from here still exists in
    /// the loop but cannot be reached from config, which is indistinguishable from a typo on the
    /// user's side — so a new variant belongs here in the same commit.
    pub const ALL: [HookPoint; 19] = [
        Self::SessionStart,
        Self::SessionEnd,
        Self::BeforeAgentStart,
        Self::UserPromptSubmit,
        Self::AgentStart,
        Self::AgentEnd,
        Self::BeforeToolUse,
        Self::AfterToolUse,
        Self::MessageStart,
        Self::MessageEnd,
        Self::BeforeCompact,
        Self::AfterCompact,
        Self::FileChanged,
        Self::Notification,
        Self::Error,
        Self::ReviewGate,
        Self::LoopControl,
        Self::PermissionRequested,
        Self::PermissionResolved,
    ];

    /// The one spelling luxe writes and documents, matching how the variant serializes.
    ///
    /// Exhaustive on purpose: a new variant does not compile until it has a name here.
    pub fn canonical_name(&self) -> &'static str {
        match self {
            Self::SessionStart => "SessionStart",
            Self::SessionEnd => "SessionEnd",
            Self::BeforeAgentStart => "BeforeAgentStart",
            Self::UserPromptSubmit => "UserPromptSubmit",
            Self::AgentStart => "AgentStart",
            Self::AgentEnd => "AgentEnd",
            Self::BeforeToolUse => "BeforeToolUse",
            Self::AfterToolUse => "AfterToolUse",
            Self::MessageStart => "MessageStart",
            Self::MessageEnd => "MessageEnd",
            Self::BeforeCompact => "BeforeCompact",
            Self::AfterCompact => "AfterCompact",
            Self::FileChanged => "FileChanged",
            Self::Notification => "Notification",
            Self::Error => "Error",
            Self::ReviewGate => "ReviewGate",
            Self::LoopControl => "LoopControl",
            Self::PermissionRequested => "PermissionRequested",
            Self::PermissionResolved => "PermissionResolved",
        }
    }

    /// Resolve a config key to a hook point, ignoring case and word separators.
    ///
    /// `BeforeToolUse`, `beforeToolUse`, `before_tool_use` and `before-tool-use` all land on the
    /// same point. Being strict here buys nothing: there is no second point a near-miss could
    /// plausibly mean, and the cost of refusing one is a hook that silently never runs.
    pub fn parse(name: &str) -> Option<Self> {
        let normalize = |s: &str| {
            s.chars()
                .filter(|c| c.is_alphanumeric())
                .flat_map(char::to_lowercase)
                .collect::<String>()
        };
        let wanted = normalize(name);
        Self::ALL.into_iter().find(|point| normalize(point.canonical_name()) == wanted)
    }

    /// The known name closest to `name`, for "did you mean" on an unrecognized config key.
    ///
    /// Deliberately crude — shared-prefix length, longest wins. A typo in a hook point is almost
    /// always a near-miss or a stale name from an older release, and both are caught by a prefix.
    pub fn closest(name: &str) -> Option<&'static str> {
        let lower = name.to_lowercase();
        let shared = |candidate: &str| {
            lower.chars().zip(candidate.to_lowercase().chars()).take_while(|(a, b)| a == b).count()
        };
        Self::ALL
            .into_iter()
            .map(|point| point.canonical_name())
            .filter(|candidate| shared(candidate) >= 3)
            .max_by_key(|candidate| shared(candidate))
    }
}

/// Outcome from a hook interception.
#[derive(Debug, Clone)]
pub enum HookOutcome {
    /// Continue with normal flow.
    Continue,
    /// Deny the operation (with reason).
    Deny { reason: String },
    /// Rewrite a tool call's arguments, and optionally escalate the rewritten call to a prompt.
    ///
    /// Args: `(args, ask)`. `args` is a shallow-merge *patch* over the call's arguments, not a
    /// replacement; `ask` carries the reason when the hook also wants the user to confirm.
    /// Honored at [`HookPoint::BeforeToolUse`] only — no other point has arguments to rewrite.
    ///
    /// The patch lands *before* the permission engine runs, so the rewritten arguments are what
    /// the gate judges and what the tool receives. A hook that turned `rm build` into `rm -rf /`
    /// would be denied by the catastrophic rules like any other call.
    Modify { args: serde_json::Value, ask: Option<String> },
    /// Escalate to a permission prompt before the operation proceeds.
    ///
    /// Args: `(reason)`. Honored at [`HookPoint::BeforeToolUse`]; elsewhere there is nothing to
    /// hold open while a human answers, so it reads as [`HookOutcome::Continue`].
    Ask(String),
    /// Add context the model reads.
    ///
    /// Args: `(text)`. At `AfterToolUse` it is appended to the tool result; at `UserPromptSubmit`
    /// and `BeforeAgentStart` it is committed with the turn's user message; at `ReviewGate` it is
    /// the gate's report. Never the system prompt, which must stay byte-identical between turns
    /// for the provider's cache to hold.
    InjectContext(String),
    /// Continue the loop with a user message (self-continuation).
    ContinueLoop { message: String },
    /// Stop the loop entirely (with reason).
    StopLoop { reason: String },
}

/// Shallow-merge `patch` over `base`: a key present in `patch` replaces the one in `base`, and
/// everything `patch` leaves out survives untouched.
///
/// Shallow on purpose. A hook that rewrites `bash`'s `command` should not have to echo back the
/// `timeout` it did not care about, and a deep merge has no defensible answer for arrays — is a
/// patched list appended, replaced, or merged by index? A nested object is therefore replaced
/// whole, which is the rule a hook author can predict without reading this function.
///
/// Anything that is not a JSON object replaces `base` outright; there is nothing to merge into.
pub(crate) fn merge_args(base: serde_json::Value, patch: serde_json::Value) -> serde_json::Value {
    match (base, patch) {
        (serde_json::Value::Object(mut base), serde_json::Value::Object(patch)) => {
            base.extend(patch);
            serde_json::Value::Object(base)
        }
        (_, patch) => patch,
    }
}

/// Join two optional reasons the way [`HookRegistry::merge`] joins everything else it cannot choose
/// between.
fn join_reasons(first: Option<String>, second: Option<String>) -> Option<String> {
    match (first, second) {
        (Some(first), Some(second)) => Some(format!("{first}\n\n{second}")),
        (only, None) | (None, only) => only,
    }
}

/// Loop control directives from the agent (via loop_control tool).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum LoopControl {
    /// Phase completed — advance to next phase (optionally specify which).
    Progress { phase: Option<u32> },
    /// All phases complete — trigger verification turn.
    Complete,
    /// Blocked on a decision or resource — pause for human.
    Blocked { reason: String },
}

/// Event data passed to hooks.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum HookEvent {
    /// Tool is about to be used.
    ToolUse { tool_name: String, args: serde_json::Value },
    /// Tool has finished.
    ToolResult { tool_name: String, result: String, is_error: bool },
    /// File has been changed.
    FileChanged { path: String, operation: String },
    /// Notification from an external source.
    Notification { source: String, message: String },
    /// Error occurred.
    Error { message: String },
    /// Generic event with arbitrary data.
    Other(serde_json::Value),
    /// Loop control directive (progress/complete/blocked).
    LoopControl(LoopControl),
}

/// Context passed to hooks at runtime.
#[derive(Debug, Clone)]
pub struct HookCtx {
    /// Session ID.
    pub session_id: String,
    /// Event sink for emitting progress updates.
    pub event_tx: tokio::sync::mpsc::Sender<crate::event::Event>,
    /// Hook registry for invoking other hooks (cloned reference).
    pub hook_registry: crate::hook::HookRegistry,
}

impl HookCtx {
    /// Say something a person watching this run should see.
    ///
    /// What a hook does is otherwise invisible outside the log: a phase advancing, a gate refusing,
    /// a run backing off. Those are the events a front-end renders, so they go out as
    /// [`crate::event::Event::Notice`] rather than staying in `tracing`.
    ///
    /// Never waits and never fails. A notice is a line on somebody's screen, and a loop that
    /// stalled to deliver one would have traded the thing being reported for the report.
    pub fn notify(&self, message: impl Into<String>) {
        let _ = self.event_tx.try_send(crate::event::Event::Notice(message.into()));
    }
}

/// The Hook trait — implements cross-cutting behavior.
#[async_trait]
pub trait Hook: Send + Sync {
    /// What this hook is called, in logs and anywhere a run is explained.
    ///
    /// Required rather than defaulted, because a default is what the registry had: every hook
    /// answered `"hook"`, so nothing the loop did could be attributed to whatever did it. Short and
    /// stable — `self-continuation`, `review-integrator` — since it lands in a log field and in the
    /// line `luxe loop start` prints about what it registered.
    fn name(&self) -> &'static str;

    /// Which hook points this hook is interested in.
    fn interest(&self) -> &[HookPoint];

    /// Handle a hook event.
    async fn on(&self, point: HookPoint, ev: &HookEvent, cx: &HookCtx) -> HookOutcome;
}

/// Registry managing hook instances and their invocations.
#[derive(Default, Clone)]
pub struct HookRegistry {
    hooks: Vec<Arc<dyn Hook>>,
}

impl std::fmt::Debug for HookRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HookRegistry").field("hook_count", &self.hooks.len()).finish()
    }
}

impl HookRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self { hooks: Vec::new() }
    }

    /// Register a hook instance.
    pub fn register(&mut self, hook: Arc<dyn Hook>) {
        self.hooks.push(hook);
    }

    /// Invoke all hooks interested in a point, in registration order.
    ///
    /// Collects outcomes from all hooks and picks the highest-priority one:
    /// StopLoop > ContinueLoop > Deny/Modify/Ask/InjectContext > Continue. Two hooks that both
    /// steer are joined rather than one being dropped.
    pub async fn invoke(&self, point: HookPoint, ev: &HookEvent, cx: &HookCtx) -> HookOutcome {
        let mut best = HookOutcome::Continue;
        for hook in &self.hooks {
            if hook.interest().contains(&point) {
                let outcome = hook.on(point, ev, cx).await;
                // Attribution, at the one place that has both the hook and what it decided. A
                // `Continue` is every hook's answer to nearly every point and would drown the log;
                // anything else changed the run, and the reader's next question is which hook did.
                if !matches!(outcome, HookOutcome::Continue) {
                    tracing::debug!(hook = hook.name(), ?point, ?outcome, "a hook acted");
                }
                best = merge_outcomes(best, outcome);
            }
        }
        best
    }

    /// Every registered hook, in registration order — which is also the order they run in.
    pub fn hook_names(&self) -> Vec<&'static str> {
        self.hooks.iter().map(|hook| hook.name()).collect()
    }
}

/// Fold two outcomes into one, keeping the higher-priority one and joining the pairs that are not
/// alternatives.
///
/// One owner for "which answer wins", shared by the registry and by the command-hook runner.
/// Two copies of a precedence table is two precedence tables.
pub(crate) fn merge_outcomes(a: HookOutcome, b: HookOutcome) -> HookOutcome {
    match (&a, &b) {
        // StopLoop always wins.
        (HookOutcome::StopLoop { .. }, _) | (_, HookOutcome::StopLoop { .. }) => {
            if let HookOutcome::StopLoop { .. } = &a {
                a
            } else {
                b
            }
        }
        // ContinueLoop beats Continue.
        (HookOutcome::ContinueLoop { .. }, HookOutcome::Continue) => a,
        (HookOutcome::Continue, HookOutcome::ContinueLoop { .. }) => b,
        // Two hooks both steering: join them, in registration order. Dropping one looks
        // harmless and is not — self-continuation steers on *every* iteration of a running
        // loop and is registered first, so keeping only the first silently threw away every
        // message context-aware termination ever produced, including "you are approaching
        // context limits" and the notice saying why a run had just ended.
        (
            HookOutcome::ContinueLoop { message: first },
            HookOutcome::ContinueLoop { message: second },
        ) => HookOutcome::ContinueLoop { message: format!("{first}\n\n{second}") },
        // Two hooks both feeding the model: joined, for the same reason. Only one outcome
        // reaches the caller, so keeping the first would silently discard the second — and
        // these are unrelated by construction (a linter's output and a plan reminder are not
        // alternatives), so there is nothing to choose between.
        (HookOutcome::InjectContext(first), HookOutcome::InjectContext(second)) => {
            HookOutcome::InjectContext(format!("{first}\n\n{second}"))
        }
        // A refusal outranks everything under it: once the call is not happening, a rewrite
        // and a prompt are both moot. This has to be explicit — the fallthrough below keeps
        // whichever outcome arrived first, so a hook that denied after one that injected
        // context would lose.
        (HookOutcome::Deny { .. }, _) => a,
        (_, HookOutcome::Deny { .. }) => b,
        // A rewrite and an escalation are not alternatives either, so they fold into one
        // outcome rather than one beating the other. Patches shallow-merge in registration
        // order, last writer winning a collided key; reasons join like every other pair here.
        (
            HookOutcome::Modify { args: first, ask: first_ask },
            HookOutcome::Modify { args: second, ask: second_ask },
        ) => HookOutcome::Modify {
            args: merge_args(first.clone(), second.clone()),
            ask: join_reasons(first_ask.clone(), second_ask.clone()),
        },
        (HookOutcome::Modify { args, ask }, HookOutcome::Ask(reason))
        | (HookOutcome::Ask(reason), HookOutcome::Modify { args, ask }) => HookOutcome::Modify {
            args: args.clone(),
            ask: join_reasons(ask.clone(), Some(reason.clone())),
        },
        (HookOutcome::Ask(first), HookOutcome::Ask(second)) => {
            HookOutcome::Ask(format!("{first}\n\n{second}"))
        }
        // An escalation outranks a bare context injection: one gates the call, the other only
        // annotates it, and only the gate can be lost by being dropped.
        (HookOutcome::Ask(_), HookOutcome::InjectContext(_))
        | (HookOutcome::Modify { .. }, HookOutcome::InjectContext(_)) => a,
        (HookOutcome::InjectContext(_), HookOutcome::Ask(_))
        | (HookOutcome::InjectContext(_), HookOutcome::Modify { .. }) => b,
        // Otherwise keep the first non-Continue.
        (HookOutcome::Continue, _) => b,
        (_, HookOutcome::Continue) => a,
        // Both are the same or similar — keep first.
        _ => a,
    }
}

#[cfg(test)]
mod tests;
