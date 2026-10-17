//! Command hooks — run user-configured external commands at agent-loop hook points.
//!
//! On a hook point the runner serializes the event to the child's stdin, reads a JSON decision
//! from stdout, enforces a timeout, and applies a fail-safe on timeout/crash. At `BeforeToolUse`
//! a hook can `deny` the call, `ask` the user to confirm it, or rewrite its arguments — so hooks
//! are a genuine policy extension point, not just an audit log.
//!
//! # Where a hook sits in the guard chain
//!
//! The runner hands its answer to `engine::dispatch`, which applies it *before* the permission
//! engine sees the call. That ordering is the whole design: a rewritten argument is what the gate
//! judges and what the tool receives, so a hook cannot launder a call past the rules by editing it
//! after the check — and an `ask` needs no prompt of its own, because it rides in as one more
//! permission check and inherits the resolution rules every other check already has.
//!
//! **Trust:** because a hook runs an arbitrary command, specs are loaded from the *global* config
//! (`~/.config/luxe/hooks.json`) only; a project's `.luxe/hooks.json` is honored solely under
//! `--trust-project`, so cloning a repo can't execute code on the next tool call (the CLI enforces
//! this — see `engine_setup`).

mod decision;
mod file;
mod serialization;
use crate::hook::{Hook, HookCtx, HookEvent, HookOutcome, HookPoint};
pub use decision::HookDecisionKind;
pub use file::{CommandHookDef, HooksFile};
use serde::{Deserialize, Serialize};
pub use serialization::run_command_hook;
use std::collections::HashMap;
use std::time::Duration;
use tokio::process::Command;
use tracing::{debug, error, warn};

/// What a command hook does when its command times out, crashes, or returns junk.
///
/// `Ask` escalates to a permission prompt at `BeforeToolUse` and reads as `Allow` everywhere else,
/// since no other point holds a call open long enough for a human to answer. It is the honest
/// setting for a guard that is advisory rather than authoritative: a classifier that fails to
/// answer should put the question to the reader, not silently wave the call through or block work
/// because a helper script is broken.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HookFailureAction {
    #[default]
    Allow,
    Deny,
    Ask,
}

/// Which tool calls a hook applies to.
///
/// The point of a matcher is that a non-matching call costs **nothing** — no process, no pipe, no
/// deadline. Without one, a `BeforeToolUse` hook that only cares about `bash` still pays a spawn
/// on every `read` and `grep`, which in a coding session is most of the traffic.
///
/// Patterns are the ones `permissions.json` already uses, deliberately: `bash` gets
/// command-aware matching (`"rm *"` finds the `rm` buried in `cd x && rm -rf y`), everything
/// else is a glob over the same string a rule would match on.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum HookMatcher {
    /// A glob over the tool name: `"bash"`, `"web_*"`, `"*"`.
    Tool(String),
    /// Tool-name glob to an argument pattern: `{"bash": "rm *", "edit": "src/**"}`.
    ///
    /// Any entry matching is enough. The argument half needs the call's arguments, which only
    /// `BeforeToolUse` carries — elsewhere it degrades to the tool-name half rather than
    /// silently never matching.
    Keyed(HashMap<String, String>),
}

impl HookMatcher {
    /// Does this matcher apply to `event`?
    ///
    /// An event with no tool call to speak of is never filtered: a matcher there is a config
    /// mistake ([`HooksFile::into_specs`] says so at load), and running the hook is the cheaper
    /// way to be wrong than silently disabling it.
    fn matches(&self, event: &HookEvent) -> bool {
        let (tool, args) = match event {
            HookEvent::ToolUse { tool_name, args } => (tool_name, Some(args)),
            HookEvent::ToolResult { tool_name, .. } => (tool_name, None),
            _ => return true,
        };
        match self {
            Self::Tool(glob) => crate::permission::wildcard::matches(tool, glob),
            Self::Keyed(map) => map.iter().any(|(tool_glob, pattern)| {
                crate::permission::wildcard::matches(tool, tool_glob)
                    && match args {
                        Some(args) => {
                            let key = crate::permission::permission_key(tool);
                            let input = crate::permission::primary_input(tool, args);
                            crate::permission::input_matches(key, &input, pattern)
                        }
                        None => true,
                    }
            }),
        }
    }

    /// Whether the argument half of this matcher can mean anything at `point`.
    fn needs_arguments(&self) -> bool {
        matches!(self, Self::Keyed(map) if map.values().any(|pattern| pattern != "*"))
    }
}

/// A command hook specification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandHookSpec {
    /// The hook point to run on.
    pub point: HookPoint,
    /// Which tool calls it applies to; `None` matches every one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matcher: Option<HookMatcher>,
    /// The command to execute (first element is the program, rest are args).
    pub command: Vec<String>,
    /// Timeout in milliseconds.
    pub timeout_ms: u64,
    /// What to do on timeout/crash.
    pub on_timeout: HookFailureAction,
}

/// Whether a command hook's stdout can change anything at `point`.
///
/// Everywhere else the engine ignores the outcome, so parsing the output there only manufactures
/// errors: a notifier that prints `sent` is doing its job, not returning a malformed decision.
fn reads_decision(point: HookPoint) -> bool {
    matches!(
        point,
        HookPoint::BeforeToolUse
            | HookPoint::AfterToolUse
            | HookPoint::UserPromptSubmit
            // the loop reads a `context` here as the gate's report
            | HookPoint::ReviewGate
    )
}

/// A [`Hook`] that runs configured external commands at their hook points.
///
/// Registered once (from trusted config) into the engine's hook registry; at each configured
/// point it runs that point's commands and returns their combined outcome (a `deny` wins).
pub struct CommandHook {
    specs: HashMap<HookPoint, Vec<CommandHookSpec>>,
    interest: Vec<HookPoint>,
}

impl CommandHook {
    /// Build from a point-keyed spec map.
    pub fn new(specs: HashMap<HookPoint, Vec<CommandHookSpec>>) -> Self {
        let interest = specs.keys().copied().collect();
        Self { specs, interest }
    }

    /// Whether any hooks are configured (skip registration when empty).
    pub fn is_empty(&self) -> bool {
        self.specs.is_empty()
    }

    /// The longest the hooks at `point` can take between them: every deadline, end to end.
    ///
    /// They run one after another, so this is the time a caller must be prepared to wait for the
    /// point to finish — what an exiting front-end gives `SessionEnd` before it stops waiting.
    pub fn budget(&self, point: HookPoint) -> Duration {
        let millis = self.specs.get(&point).into_iter().flatten().map(|spec| spec.timeout_ms).sum();
        Duration::from_millis(millis)
    }
}

#[async_trait::async_trait]
impl Hook for CommandHook {
    fn name(&self) -> &'static str {
        "command"
    }

    fn interest(&self) -> &[HookPoint] {
        &self.interest
    }

    async fn on(&self, point: HookPoint, ev: &HookEvent, _cx: &HookCtx) -> HookOutcome {
        run_command_hooks_for_point(&self.specs, &point, ev).await
    }
}

/// Decision returned by a command hook, as JSON on its stdout.
///
/// Every field is optional except `decision`, and an empty `{"decision": "allow"}` is the way to
/// say "no opinion" — the call carries on into the permission engine exactly as if no hook were
/// installed. `allow` is deliberately *not* an approval: a hook cannot pre-approve its way past a
/// rule the reader wrote, which is the difference between an extension point and a back door.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookDecision {
    /// `allow` (no opinion), `deny`/`block` (refuse the call), or `ask` (make the user confirm).
    pub decision: HookDecisionKind,
    /// Why. Shown to the user on an `ask` and returned to the model on a `deny`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// A shallow-merge patch over the tool call's arguments, honored at `BeforeToolUse`.
    ///
    /// Keys present replace the call's; keys left out survive. Combines with any `decision`
    /// except a refusal, which discards the patch along with the call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<serde_json::Value>,
    /// Text to feed back to the model, honored at `AfterToolUse` and `UserPromptSubmit`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

impl HookDecision {
    /// Lower a parsed decision into the outcome the engine acts on.
    ///
    /// `point` decides which fields mean anything: only `BeforeToolUse` has arguments to rewrite
    /// or a call to hold open for a prompt, so `args` and `ask` are dropped elsewhere rather than
    /// returned as an outcome the caller would only warn about.
    fn into_outcome(self, point: HookPoint) -> HookOutcome {
        // A reason is written by a guard describing the call it objected to, so it quotes the
        // command — and from here it travels to the transcript on a deny and to the approval card
        // on an `ask`. Its sibling `context` has always been scrubbed on the way out; this is the
        // same trip with the same hazard, so it gets the same treatment at the same choke point.
        let reason = self.reason.map(|reason| crate::redact::process_secrets().redact(&reason));
        if matches!(self.decision, HookDecisionKind::Deny | HookDecisionKind::Block) {
            return HookOutcome::Deny {
                reason: reason.unwrap_or_else(|| "denied by command hook".to_string()),
            };
        }
        if point == HookPoint::BeforeToolUse {
            let ask = (self.decision == HookDecisionKind::Ask).then(|| {
                reason.clone().unwrap_or_else(|| "a command hook asked you to confirm".into())
            });
            match (self.args, ask) {
                (Some(args), ask) => return HookOutcome::Modify { args, ask },
                (None, Some(reason)) => return HookOutcome::Ask(reason),
                (None, None) => {}
            }
        }
        match self.context.filter(|c| !c.trim().is_empty()) {
            Some(context) => HookOutcome::InjectContext(context),
            None => HookOutcome::Continue,
        }
    }
}

async fn run_serialized_hook(spec: &CommandHookSpec, event_json: String) -> HookOutcome {
    debug!(point = ?spec.point, command = ?spec.command, "running command hook");

    let timeout = Duration::from_millis(spec.timeout_ms);
    let mut child = match spawn_hook(spec) {
        Ok(child) => child,
        Err(outcome) => return outcome,
    };

    let child_id = child.id().unwrap_or(0);
    let deadline = tokio::time::Instant::now() + timeout;
    let stdin = child.stdin.take();
    // Draining output while feeding input prevents an echoing hook from filling both pipes
    let output = tokio::time::timeout_at(deadline, async {
        let wait = async { child.wait_with_output().await.map_err(HookIoError::Wait) };
        let (_, output) = tokio::try_join!(feed_stdin(stdin, event_json.as_bytes()), wait)?;
        Ok::<_, HookIoError>(output)
    })
    .await;

    let decision = match output {
        Ok(Ok(output)) => interpret_exit(spec, output),
        Ok(Err(e)) => {
            error!(error = %e, "command hook I/O failed");
            crate::program::kill_tree(child_id);
            timeout_outcome(spec, e.to_string())
        }
        Err(_) => {
            // Timeout — kill the whole tree (kill_on_drop only reaps the direct child).
            warn!(timeout_ms = spec.timeout_ms, pid = child_id, "command hook timed out");
            crate::program::kill_tree(child_id);
            timeout_outcome(spec, format!("command hook timed out after {}ms", spec.timeout_ms))
        }
    };

    decision
}

fn spawn_hook(spec: &CommandHookSpec) -> Result<tokio::process::Child, HookOutcome> {
    let Some((program, args)) = spec.command.split_first() else {
        return Err(timeout_outcome(spec, "command hook has an empty command".to_string()));
    };
    // Spawn as its own process group with kill-on-drop, so a timeout can reap the whole tree
    // (not just the direct child) instead of orphaning descendants.
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    crate::program::detach_from_terminal(&mut command);
    match command.spawn() {
        Ok(child) => Ok(child),
        Err(e) => {
            error!(command = ?spec.command, error = %e, "failed to spawn command hook");
            Err(timeout_outcome(spec, format!("command hook failed to spawn: {e}")))
        }
    }
}

fn interpret_exit(spec: &CommandHookSpec, output: std::process::Output) -> HookOutcome {
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        error!(stderr = %stderr, "command hook exited with error");
        timeout_outcome(spec, format!("command hook exited with error: {stderr}"))
    } else if !reads_decision(spec.point) || stdout.trim().is_empty() {
        // Nothing to act on, which is "no opinion" — the same as `allow`, not a failure
        debug!(point = ?spec.point, "command hook finished without a decision");
        HookOutcome::Continue
    } else {
        match serde_json::from_str::<HookDecision>(&stdout) {
            Ok(decision) => {
                debug!(decision = ?decision.decision, reason = ?decision.reason, "command hook decision");
                decision.resolve(spec)
            }
            Err(e) => {
                error!(error = %e, stdout = %stdout, "failed to parse command hook decision");
                timeout_outcome(spec, format!("command hook returned invalid JSON: {e}"))
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum HookIoError {
    #[error("command hook stdin write failed: {0}")]
    Stdin(std::io::Error),
    #[error("command hook wait failed: {0}")]
    Wait(std::io::Error),
}

async fn feed_stdin(
    stdin: Option<tokio::process::ChildStdin>,
    event: &[u8],
) -> Result<(), HookIoError> {
    let Some(mut stdin) = stdin else { return Ok(()) };
    match tokio::io::AsyncWriteExt::write_all(&mut stdin, event).await {
        Ok(()) => Ok(()),
        // A hook may ignore stdin and still emit a valid decision before exiting
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => {
            debug!("command hook did not read stdin (broken pipe); reading its decision anyway");
            Ok(())
        }
        Err(error) => Err(HookIoError::Stdin(error)),
    }
}

/// The configured fail-safe, as an outcome.
///
/// Every way a hook can fail to produce a usable decision — no spawn, a crash, junk on stdout, a
/// blown deadline — lands here, so "what happens when the guard is broken" is one answer in one
/// place rather than five copies that drift apart.
fn timeout_outcome(spec: &CommandHookSpec, reason: String) -> HookOutcome {
    match spec.on_timeout {
        HookFailureAction::Deny => HookOutcome::Deny { reason },
        HookFailureAction::Ask if spec.point == HookPoint::BeforeToolUse => {
            HookOutcome::Ask(reason)
        }
        HookFailureAction::Allow | HookFailureAction::Ask => HookOutcome::Continue,
    }
}

/// Run all command hooks registered for a given hook point.
pub async fn run_command_hooks_for_point(
    hooks: &HashMap<HookPoint, Vec<CommandHookSpec>>,
    point: &HookPoint,
    event: &HookEvent,
) -> HookOutcome {
    let Some(specs) = hooks.get(point) else {
        return HookOutcome::Continue;
    };
    let mut folded = HookOutcome::Continue;
    for spec in specs {
        // Filtered out *before* the spawn, which is the entire point of a matcher.
        if !spec.matcher.as_ref().is_none_or(|m| m.matches(event)) {
            continue;
        }
        let outcome = run_command_hook(spec, event).await;
        // A refusal short-circuits: the remaining hooks can only weigh in on a call that is no
        // longer going to happen, and each one costs a process spawn on the turn's critical path.
        if matches!(outcome, HookOutcome::Deny { .. }) {
            return outcome;
        }
        folded = crate::hook::merge_outcomes(folded, outcome);
    }
    folded
}

#[cfg(test)]
mod tests;
