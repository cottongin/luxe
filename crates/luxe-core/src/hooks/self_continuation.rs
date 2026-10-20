//! Self-continuation hook — enqueues the next iteration's user message on AgentEnd.
//!
//! When the continuous loop is running, this hook fires on `HookPoint::AgentEnd` and
//! returns `HookOutcome::ContinueLoop` with a "continue building" steer message. The
//! engine handles this by sending `Action::SubmitUserMessage`.

use crate::hook::{Hook, HookCtx, HookEvent, HookOutcome, HookPoint};
use crate::loop_controller::LoopController;
use async_trait::async_trait;
use std::sync::Arc;
use tracing::{debug, info};

/// Characters of the phase's plan section carried into the steer that opens it.
///
/// A steer is a user message and therefore permanent history, so the slice is bounded and sent
/// **once per phase** — every later iteration of the same phase gets the heading alone.
const MAX_PHASE_SECTION_CHARS: usize = 4_000;

/// Default steer message for self-continuation.
///
/// The journal path is built from the constants the loop actually writes to, so no iteration opens
/// by telling the agent to read a file that is not there.
///
/// It also has to say how a phase *ends*. Without the last sentence nothing ever calls
/// `loop_control`, so the phase gate never runs, the phase never advances, and the loop grinds
/// through its turn budget on one phase forever. Measured: four turns of real work, no claim, no
/// gate.
fn default_continue_steer() -> String {
    format!(
        "Continue building. Read {dir}/{journal} to recall state, then take ONE small concrete \
         increment toward the current phase, validate it, record what you did, and END YOUR TURN. \
         When the current phase is genuinely finished and its acceptance checks pass, call \
         `loop_control` with action=\"progress\" to claim it — the phase only advances if the gate \
         agrees, and you will be told what is missing if it does not.",
        dir = crate::loop_hooks::journal_dir_display(),
        journal = crate::loop_hooks::JOURNAL_FILE,
    )
}

/// What the loop is working on, as the steer says it.
///
/// `section` is `Some` only on the first iteration of a phase: the plan's own words are what stop a
/// developer building the whole plan at once, and repeating them every turn would pay for them
/// again in every request that follows.
fn phase_steer(phase: u32, heading: Option<&str>, section: Option<&str>) -> String {
    let named = match heading {
        // A heading that already names its own number is left alone, or the steer reads
        // "phase 1 — Phase 1 — Ranking".
        Some(heading) if heading.to_ascii_lowercase().starts_with(&format!("phase {phase}")) => {
            heading.to_string()
        }
        Some(heading) => format!("phase {phase} — {heading}"),
        None => format!("phase {phase}"),
    };
    let mut steer = format!(
        "Continue building. You are on {named}. Read {dir}/{journal} to recall state, then take ONE \
         small concrete increment toward THIS phase — not the ones after it — validate it, record \
         what you did, and END YOUR TURN. When this phase is genuinely finished and its acceptance \
         checks pass, call `loop_control` with action=\"progress\" to claim it — the phase only \
         advances if the gate agrees, and you will be told what is missing if it does not.",
        dir = crate::loop_hooks::journal_dir_display(),
        journal = crate::loop_hooks::JOURNAL_FILE,
    );
    if let Some(section) = section {
        steer.push_str(&format!("\n\n## What phase {phase} is, from `PLAN.md`\n\n{section}"));
    }
    steer
}

/// The steer for a phase number the plan does not have.
///
/// The counter can outrun the plan — the gate advances it on every accepted claim, and nothing in
/// the plan says how many phases there are. Left alone the loop keeps steering "work on the current
/// phase" at a phase that does not exist, and a finished project spends the rest of its turn
/// budget. Saying so, and naming both ways out, is the difference between a loop that ends and
/// one that idles.
///
/// What the work is verified against is `PLAN.md`, and `PROJECT.md`'s Definition of Done only where
/// the project has one: naming a file that is not there sends the loop to `blocked`, asking a
/// person for a document no plan-mode plan comes with.
fn past_the_plan_steer(phase: u32, has_project_brief: bool) -> String {
    let against = match has_project_brief {
        true => "what `PLAN.md` asked for, and the Definition of Done in `PROJECT.md`,",
        false => "what each phase of `PLAN.md` asked for",
    };
    format!(
        "`PLAN.md` has no phase {phase}, so its phases are done. Do not invent another one. Verify \
         {against} against what is actually in the tree, record the result in {dir}/{journal}, and \
         then either call `loop_control` with action=\"complete\" if every item holds, or \
         action=\"blocked\" with the specific question if something needs a human decision.",
        dir = crate::loop_hooks::journal_dir_display(),
        journal = crate::loop_hooks::JOURNAL_FILE,
    )
}

/// The steer for `phase`, as the loop would send it.
///
/// `brief` carries the phase's own section of `PLAN.md`; without it the steer names the phase and
/// nothing more. Public because the loop's *kickoff* has to say the same thing its iterations do,
/// or a developer sees nothing about which phase just started.
pub async fn steer_for(repo_root: &std::path::Path, phase: u32, brief: bool) -> String {
    let Ok(plan) = tokio::fs::read_to_string(repo_root.join("PLAN.md")).await else {
        return default_continue_steer();
    };
    let Some(section) = crate::plan::phase_section(&plan, phase) else {
        // A plan with no phase headings at all is not a plan that ran out; it is a plan written
        // another way, and the generic steer is right for it.
        return match crate::plan::phase_section(&plan, 0).is_some() {
            true => past_the_plan_steer(phase, repo_root.join("PROJECT.md").is_file()),
            false => default_continue_steer(),
        };
    };
    let heading = crate::plan::phase_heading(&plan, phase);
    phase_steer(
        phase,
        heading.as_deref(),
        brief.then(|| crate::hooks::head(&section, MAX_PHASE_SECTION_CHARS)).as_deref(),
    )
}

/// A hook that enables self-continuation for the native continuous loop.
///
/// On `HookPoint::AgentEnd`, if the loop is running, returns `HookOutcome::ContinueLoop`
/// with a steer message to enqueue the next iteration.
pub struct SelfContinuationHook {
    /// Loop controller for checking running state.
    loop_controller: Arc<LoopController>,
    /// Custom steer message (if None, use default).
    continue_steer: Option<String>,
    /// Cooldown in milliseconds between iterations (0 = no cooldown).
    cooldown_ms: u64,
    /// Where the pause after a failed turn comes from.
    pacing: Option<Arc<dyn crate::hooks::runaway_guards::IterationPacing>>,
    /// The project whose `PLAN.md` says what each phase is. `None` steers without one.
    repo_root: Option<std::path::PathBuf>,
    /// The phase whose plan section has already been sent, so it is sent once rather than per turn.
    briefed_phase: std::sync::Mutex<Option<u32>>,
}

impl std::fmt::Debug for SelfContinuationHook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SelfContinuationHook").field("cooldown_ms", &self.cooldown_ms).finish()
    }
}

impl SelfContinuationHook {
    /// Create a new self-continuation hook.
    pub fn new(loop_controller: Arc<LoopController>) -> Self {
        Self {
            loop_controller,
            continue_steer: None,
            cooldown_ms: 0,
            pacing: None,
            repo_root: None,
            briefed_phase: std::sync::Mutex::new(None),
        }
    }

    /// The project the loop is building, whose `PLAN.md` scopes each iteration to one phase.
    pub fn in_project(mut self, repo_root: impl Into<std::path::PathBuf>) -> Self {
        self.repo_root = Some(repo_root.into());
        self
    }

    /// Treat `phase` as already briefed, because the loop's kickoff message carried its plan.
    pub fn briefed_on(self, phase: u32) -> Self {
        if let Ok(mut briefed) = self.briefed_phase.lock() {
            *briefed = Some(phase);
        }
        self
    }

    /// Pace iterations by how the last ones went, so a failing provider is backed off.
    pub fn with_pacing(
        mut self,
        pacing: Arc<dyn crate::hooks::runaway_guards::IterationPacing>,
    ) -> Self {
        self.pacing = Some(pacing);
        self
    }

    /// Wait before the next iteration: the fixed cooldown, then any backoff a failure earned.
    ///
    /// Slept in short slices and abandoned the moment the loop stops, because `luxe loop stop` has
    /// to take effect while a backoff is in progress — a run that ignores it for half a minute reads
    /// as a hung process.
    async fn wait_before_continuing(&self) {
        let mut remaining = std::time::Duration::from_millis(self.cooldown_ms);
        if let Some(pacing) = self.pacing.as_ref() {
            let backoff = pacing.pause_before_next_iteration().await;
            if !backoff.is_zero() {
                debug!(?backoff, "backing off after a failed turn");
                remaining = remaining.saturating_add(backoff);
            }
        }
        const SLICE: std::time::Duration = std::time::Duration::from_millis(250);
        while !remaining.is_zero() {
            if !self.loop_controller.is_running().await {
                debug!("loop stopped during the pause");
                return;
            }
            let slice = remaining.min(SLICE);
            tokio::time::sleep(slice).await;
            remaining = remaining.saturating_sub(slice);
        }
    }

    /// Set a custom steer message for self-continuation.
    pub fn with_steer(mut self, steer: String) -> Self {
        self.continue_steer = Some(steer);
        self
    }

    /// Set the cooldown between iterations in milliseconds.
    pub fn with_cooldown(mut self, ms: u64) -> Self {
        self.cooldown_ms = ms;
        self
    }

    /// The steer for this iteration: a custom one, else the current phase's, else the generic one.
    ///
    /// The phase matters because nothing else tells the developer which one it is on. Without it a
    /// model handed a three-phase plan builds all three in the first turn, and then claims phases
    /// there is no work left in — which is exactly what a run with a 27B local developer did.
    async fn steer_message(&self) -> String {
        if let Some(custom) = self.continue_steer.clone() {
            return custom;
        }
        let Some(repo_root) = self.repo_root.as_ref() else { return default_continue_steer() };
        let phase = self.loop_controller.get_state().await.current_phase;

        // Scoped, and never held across the read below: a phase is briefed once, and the lock only
        // decides whether this is that once.
        let first_iteration_of_this_phase = match self.briefed_phase.lock() {
            Ok(mut briefed) => {
                let first = *briefed != Some(phase);
                *briefed = Some(phase);
                first
            }
            Err(_) => true,
        };

        steer_for(repo_root, phase, first_iteration_of_this_phase).await
    }
}

#[async_trait]
impl Hook for SelfContinuationHook {
    fn name(&self) -> &'static str {
        "self-continuation"
    }

    fn interest(&self) -> &[HookPoint] {
        &[HookPoint::AgentEnd]
    }

    async fn on(&self, _point: HookPoint, _ev: &HookEvent, cx: &HookCtx) -> HookOutcome {
        if !self.loop_controller.is_running().await {
            debug!("loop not running, skipping");
            return HookOutcome::Continue;
        }

        // Increment iteration for runaway guards.
        self.loop_controller.increment_iteration().await;

        self.wait_before_continuing().await;

        let steer = self.steer_message().await;
        // Routine, and the steer is long and identical every time: the iteration number is the part
        // worth reading, and the text itself is a `debug!` for when it is genuinely in question.
        let state = self.loop_controller.get_state().await;
        let iteration = state.iteration;
        info!(iteration, "continuing the loop");
        debug!(%steer, "self-continuation steer");
        // The one marker that makes a run readable from outside: where it is, each time it moves.
        cx.notify(iteration_notice(iteration, state.current_phase));

        HookOutcome::ContinueLoop { message: steer }
    }
}

/// The notice that opens each iteration: `iteration 4 · phase 1`.
pub fn iteration_notice(iteration: u64, phase: u32) -> String {
    format!("iteration {iteration} · phase {phase}")
}

/// The iteration and phase an [`iteration_notice`] names, or `None` for any other notice. Beside
/// the writer so a front-end drawing the run's structure never guesses at the wording.
pub fn parse_iteration_notice(notice: &str) -> Option<(u64, u32)> {
    let (iteration, phase) = notice.strip_prefix("iteration ")?.split_once(" · phase ")?;
    Some((iteration.parse().ok()?, phase.parse().ok()?))
}

#[cfg(test)]
mod tests;
