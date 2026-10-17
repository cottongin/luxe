//! The phase gate — what a claim has to clear, wherever the claim comes from.
//!
//! Two stages, in order. The **acceptance commands** say whether the code runs; the **reviewer**
//! says whether it does what was asked. A red suite never reaches the reviewer, because a review
//! that contradicts a failing test is worth nothing and the reviewer is the expensive half.
//!
//! One type rather than logic inside a hook, because two different claims have to go through it:
//! `loop_control action="progress"` closes a phase, and `action="complete"` ends the whole run. The
//! gate judges both, or a reviewed exit and an unreviewed one would depend on which word the agent
//! chose.

use crate::hook::{Hook, HookCtx, HookEvent, HookOutcome, HookPoint};
use crate::hooks::review_gate::ReviewGateHook;
use crate::phase_review::{PhaseReviewOutcome, PhaseReviewRequest, PhaseReviewer};
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, warn};

/// How many times the reviewer may refuse one phase before the gate stops asking and holds it.
///
/// A reviewer can refuse for a reason the agent cannot act on, such as a phase whose work already
/// landed in an earlier one, and every refusal is a paid call. Past this many, the next claim is
/// held for a person rather than re-asked forever or waved through.
const MAX_REVIEWER_REFUSALS: u32 = 3;

/// Where a phase review's evidence comes from.
///
/// A trait because gathering it means running `git` in the project and reading its planning files,
/// and a gate that did those directly could not be tested without a repository on disk.
#[async_trait]
pub trait PhaseReviewContext: Send + Sync + std::fmt::Debug {
    /// The change under review, as a unified diff.
    async fn diff(&self) -> Result<String, String>;

    /// What `phase` was for, from the project's planning documents.
    ///
    /// Scoped to the phase where the plan says which is which: a reviewer handed all four phases
    /// judges phase one against a document describing four, which is not the question it was asked.
    async fn planning(&self, phase: u32) -> String;

    /// The files the requirement has to be true of, as they now stand.
    ///
    /// Given both the diff and the phase's planning text, because either can name a file the other
    /// does not: the diff knows what moved, and the plan knows what was asked for — which is the only
    /// source left when the work landed in an earlier phase.
    async fn files(&self, diff: &str, planning: &str) -> Vec<crate::phase_review::ReviewedFile>;
}

/// What the gate decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateOutcome {
    /// Nothing objected.
    Passed,
    /// Something objected, and this is what the agent has to be told.
    Refused(String),
    /// A configured reviewer could not approve and asking again will not help, so a person decides.
    ///
    /// The reviewer was unavailable, its evidence could not be read, there was none, or it has
    /// refused this phase as often as it may. None of those is approval: the loop pauses with this
    /// explanation instead of advancing on the commands' word alone.
    Held(String),
}

impl GateOutcome {
    /// Whether the claim may proceed.
    pub fn passed(&self) -> bool {
        matches!(self, Self::Passed)
    }
}

/// The gate a claimed phase clears: acceptance commands, then the reviewer.
#[derive(Default)]
pub struct PhaseGate {
    commands: Option<Arc<ReviewGateHook>>,
    reviewer: Option<Arc<dyn PhaseReviewer>>,
    context: Option<Arc<dyn PhaseReviewContext>>,
    /// Which phase the reviewer has been refusing, and how many times.
    refusals: Mutex<(u32, u32)>,
}

impl std::fmt::Debug for PhaseGate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PhaseGate")
            .field("commands", &self.commands.is_some())
            .field("reviewer", &self.reviewer.is_some())
            .finish()
    }
}

impl PhaseGate {
    /// A gate that accepts everything, which is what a loop with nothing configured has.
    pub fn new() -> Self {
        Self::default()
    }

    /// The acceptance commands stage.
    pub fn with_commands(mut self, hook: Arc<ReviewGateHook>) -> Self {
        self.commands = Some(hook);
        self
    }

    /// The reviewer stage, and where its evidence comes from.
    ///
    /// Both or neither: a reviewer with no diff to read would be asked to judge nothing.
    pub fn with_reviewer(
        mut self,
        reviewer: Arc<dyn PhaseReviewer>,
        context: Arc<dyn PhaseReviewContext>,
    ) -> Self {
        self.reviewer = Some(reviewer);
        self.context = Some(context);
        self
    }

    /// Whether a reviewer model will be consulted.
    pub fn has_reviewer(&self) -> bool {
        self.reviewer.is_some()
    }

    /// Run the gate for `phase`.
    ///
    /// `what` names the claim being judged ("this phase", "the work") so the refusal reads as an
    /// answer to what was actually asked. `say` reports which stage is running: the gate is the
    /// slowest thing in a loop iteration by a wide margin — a test suite, then a reviewer taking a
    /// real turn — and a run that goes silent for four minutes is indistinguishable from one that
    /// has died.
    pub async fn run(
        &self,
        phase: u32,
        what: &str,
        say: &(dyn Fn(String) + Send + Sync),
    ) -> GateOutcome {
        if self.commands.is_some() {
            say("running the acceptance commands".into());
        }
        let commands = match self.run_commands().await {
            Ok(output) if output.is_empty() => output,
            Ok(fix_list) => {
                info!(phase, "the acceptance commands refused the claim");
                return GateOutcome::Refused(format!(
                    "The acceptance gate did not pass, so {what} is not complete. Fix these, then \
                     claim it again:\n\n{fix_list}"
                ));
            }
            Err(error) => {
                // A gate that could not run is not a gate that passed.
                warn!(%error, "the acceptance gate could not run");
                return GateOutcome::Refused(format!(
                    "The acceptance gate could not run, so {what} has not been accepted: {error}"
                ));
            }
        };

        if self.reviewer.is_some() {
            say(match self.commands.is_some() {
                true => "the acceptance commands passed — asking the reviewer".into(),
                false => "asking the reviewer".to_string(),
            });
        }
        match self.run_reviewer(phase, &commands).await {
            Review::Approved => GateOutcome::Passed,
            Review::Findings(findings) => {
                let listed =
                    findings.iter().map(|f| format!("- {f}")).collect::<Vec<_>>().join("\n");
                GateOutcome::Refused(format!(
                    "The acceptance gate passed, but the reviewer found work that has to land \
                     before {what} is complete. Address these, then claim it again:\n\n{listed}"
                ))
            }
            Review::Undecided(why) => GateOutcome::Held(format!(
                "The reviewer could not approve {what} ({why}), so the loop is paused for a person. \
                 Fix what it needs and run `luxe loop start` again, or accept phase {phase} with \
                 `luxe loop phase --set {}`.",
                phase + 1
            )),
        }
    }

    /// Run the acceptance commands, returning their fix list (empty when they passed).
    async fn run_commands(&self) -> Result<String, String> {
        let Some(gate) = self.commands.as_ref() else {
            return Ok(String::new());
        };
        // The gate is a hook because `HookPoint::ReviewGate` is its published entry point; calling
        // it directly here is how the loop reaches it without going through a registry that would
        // also fire everything else listening there.
        let ctx = HookCtx {
            session_id: "phase-gate".into(),
            event_tx: tokio::sync::mpsc::channel(1).0,
            hook_registry: crate::hook::HookRegistry::new(),
        };
        match gate.on(HookPoint::ReviewGate, &HookEvent::Other(serde_json::Value::Null), &ctx).await
        {
            // "CONTINUE" heads a passing verdict; anything else is the fix list.
            HookOutcome::InjectContext(text) if text.starts_with("CONTINUE") => Ok(String::new()),
            HookOutcome::InjectContext(text) => Ok(text),
            HookOutcome::Continue => Ok(String::new()),
            other => Err(format!("unexpected gate outcome: {other:?}")),
        }
    }

    /// Whether the reviewer has already refused this phase as often as it may.
    ///
    /// Counted per phase and reset when the phase changes, so a phase that legitimately needs three
    /// attempts gets them and a new phase starts with a clean slate.
    async fn reviewer_is_out_of_turns(&self, phase: u32) -> bool {
        let mut refusals = self.refusals.lock().await;
        if refusals.0 != phase {
            *refusals = (phase, 0);
        }
        refusals.1 >= MAX_REVIEWER_REFUSALS
    }

    /// Record that the reviewer refused `phase`.
    async fn note_refusal(&self, phase: u32) -> u32 {
        let mut refusals = self.refusals.lock().await;
        if refusals.0 != phase {
            *refusals = (phase, 0);
        }
        refusals.1 += 1;
        refusals.1
    }

    /// Run the reviewer: approval, findings the agent can act on, or no decision at all.
    ///
    /// Only an explicit approval passes. With no reviewer configured there is nothing to ask, which
    /// counts as approval; every other way of not answering is undecided.
    async fn run_reviewer(&self, phase: u32, commands: &str) -> Review {
        let (Some(reviewer), Some(context)) = (self.reviewer.as_ref(), self.context.as_ref())
        else {
            return Review::Approved;
        };
        if self.reviewer_is_out_of_turns(phase).await {
            // not asked again, so the refusal is not repeated and the call is not paid for
            warn!(
                phase,
                refusals = MAX_REVIEWER_REFUSALS,
                "the reviewer keeps refusing this phase"
            );
            return Review::Undecided(format!(
                "it refused this phase {MAX_REVIEWER_REFUSALS} times"
            ));
        }
        let diff = match context.diff().await {
            Ok(diff) => diff,
            Err(error) => {
                warn!(%error, "phase review impossible: could not read the change");
                return Review::Undecided(format!("the change could not be read: {error}"));
            }
        };
        let planning = context.planning(phase).await;
        let files = context.files(&diff, &planning).await;
        if diff.trim().is_empty() && files.is_empty() {
            // reviewing no evidence at all invites invented findings
            warn!(phase, "phase review impossible: no diff and no files to read");
            return Review::Undecided("there was no change and no file to read".into());
        }
        let request = PhaseReviewRequest {
            reason: crate::phase_review::ReviewReason::PhaseClaim,
            phase,
            diff,
            planning,
            gate: commands.to_string(),
            files,
        };
        match reviewer.review(request).await {
            PhaseReviewOutcome::Continue => {
                info!(phase, "phase review passed");
                Review::Approved
            }
            PhaseReviewOutcome::Fix(findings) => {
                let refusals = self.note_refusal(phase).await;
                info!(phase, findings = findings.len(), refusals, "phase review found work");
                Review::Findings(findings)
            }
            PhaseReviewOutcome::Unavailable(reason) => {
                warn!(%reason, phase, "phase review unusable");
                Review::Undecided(format!("it could not answer: {reason}"))
            }
        }
    }
}

/// What the reviewer stage concluded.
enum Review {
    Approved,
    Findings(Vec<String>),
    /// Args: `(why)`, a clause naming what went wrong
    Undecided(String),
}

#[cfg(test)]
mod tests;
