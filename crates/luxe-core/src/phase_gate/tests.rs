use super::*;

#[derive(Debug)]
struct Reviewer(PhaseReviewOutcome);

#[async_trait]
impl PhaseReviewer for Reviewer {
    async fn review(&self, _request: PhaseReviewRequest) -> PhaseReviewOutcome {
        self.0.clone()
    }
}

/// A reviewer that records how often it was consulted.
#[derive(Debug)]
struct Counting {
    asked: Arc<std::sync::atomic::AtomicUsize>,
    outcome: PhaseReviewOutcome,
}

#[async_trait]
impl PhaseReviewer for Counting {
    async fn review(&self, _request: PhaseReviewRequest) -> PhaseReviewOutcome {
        self.asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.outcome.clone()
    }
}

#[derive(Debug)]
struct Context(Result<String, String>);

#[async_trait]
impl PhaseReviewContext for Context {
    async fn diff(&self) -> Result<String, String> {
        self.0.clone()
    }

    async fn planning(&self, _phase: u32) -> String {
        "## Phase 0\nDo the thing.".into()
    }

    async fn files(&self, _diff: &str, _planning: &str) -> Vec<crate::phase_review::ReviewedFile> {
        Vec::new()
    }
}

fn gate_with(outcome: PhaseReviewOutcome, diff: Result<String, String>) -> PhaseGate {
    PhaseGate::new().with_reviewer(Arc::new(Reviewer(outcome)), Arc::new(Context(diff)))
}

/// The gate says which stage it is in, because the stages are minutes apart.
///
/// A claim runs a test suite and then a model taking a real turn with real tools. Between the
/// two, a run prints nothing for as long as they take — and silence is exactly what a hung
/// provider looks like. The lines are what tell the reader which half is slow, and that the
/// suite passed before the expensive half started.
#[tokio::test]
async fn the_gate_says_which_stage_is_running() {
    let said = std::sync::Mutex::new(Vec::new());
    let gate = gate_with(PhaseReviewOutcome::Continue, Ok("+ change".into()));

    gate.run(0, "this phase", &|line| said.lock().expect("not poisoned").push(line)).await;

    let said = said.lock().expect("not poisoned").join(" | ");
    assert!(said.contains("asking the reviewer"), "{said}");
    assert!(
        !said.contains("acceptance commands"),
        "there are none configured here, so there is nothing to say about them: {said}"
    );
}

/// A gate with nothing configured has no stages, and says nothing.
#[tokio::test]
async fn an_empty_gate_narrates_nothing() {
    let said = std::sync::Mutex::new(Vec::new());

    PhaseGate::new()
        .run(0, "this phase", &|line| said.lock().expect("not poisoned").push(line))
        .await;

    assert!(said.lock().expect("not poisoned").is_empty());
}

/// An empty gate accepts, so a loop with nothing configured still moves.
#[tokio::test]
async fn a_gate_with_nothing_configured_passes() {
    assert_eq!(PhaseGate::new().run(0, "this phase", &|_| {}).await, GateOutcome::Passed);
    assert!(!PhaseGate::new().has_reviewer());
}

/// An approving reviewer passes the claim.
#[tokio::test]
async fn an_approving_reviewer_passes() {
    let gate = gate_with(PhaseReviewOutcome::Continue, Ok("+ change".into()));
    assert_eq!(gate.run(0, "this phase", &|_| {}).await, GateOutcome::Passed);
    assert!(gate.has_reviewer());
}

/// A reviewer with findings refuses, and the refusal names them and what it judged.
#[tokio::test]
async fn findings_are_refused_and_named() {
    let gate =
        gate_with(PhaseReviewOutcome::Fix(vec!["src/a.rs:1 wrong".into()]), Ok("+ change".into()));
    let GateOutcome::Refused(message) = gate.run(2, "the work", &|_| {}).await else {
        panic!("findings must refuse")
    };
    assert!(message.contains("src/a.rs:1 wrong"), "{message}");
    assert!(message.contains("the work"), "the refusal answers what was claimed: {message}");
    assert!(message.contains("reviewer found work"), "{message}");
}

/// A reviewer that keeps refusing one phase stops being asked, and the phase waits for a person.
///
/// The break this catches, measured on a four-phase run: the developer implemented most of the
/// plan during phase 0, so phase 1 had no new source in its diff and the reviewer kept answering
/// "no changes to wordfreq.py, cannot verify rank() exists" — eleven times, each a paid call, with
/// no diff the agent could produce for work already done. An unattended loop must not be able to
/// wedge on an objection nothing can satisfy.
#[tokio::test]
async fn a_reviewer_that_will_never_be_satisfied_stops_being_asked() {
    let asked = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let gate = PhaseGate::new().with_reviewer(
        Arc::new(Counting {
            asked: asked.clone(),
            outcome: PhaseReviewOutcome::Fix(vec!["cannot verify anything".into()]),
        }),
        Arc::new(Context(Ok("+ pycache noise".into()))),
    );

    for attempt in 1..=MAX_REVIEWER_REFUSALS {
        assert!(
            matches!(gate.run(1, "this phase", &|_| {}).await, GateOutcome::Refused(_)),
            "attempt {attempt} must still be refused"
        );
    }
    let GateOutcome::Held(message) = gate.run(1, "this phase", &|_| {}).await else {
        panic!("past the cap the phase is held for a person, never waved through")
    };
    assert!(message.contains("luxe loop phase --set 2"), "says how to accept it: {message}");
    assert_eq!(
        asked.load(std::sync::atomic::Ordering::SeqCst),
        MAX_REVIEWER_REFUSALS as usize,
        "and the reviewer is not paid for the attempt past its cap"
    );
}

/// The count is per phase: a new phase gets the reviewer's full attention again.
#[tokio::test]
async fn the_refusal_count_resets_with_the_phase() {
    let asked = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let gate = PhaseGate::new().with_reviewer(
        Arc::new(Counting {
            asked: asked.clone(),
            outcome: PhaseReviewOutcome::Fix(vec!["not done".into()]),
        }),
        Arc::new(Context(Ok("+ change".into()))),
    );

    for _ in 0..MAX_REVIEWER_REFUSALS {
        gate.run(1, "this phase", &|_| {}).await;
    }
    assert!(
        matches!(gate.run(1, "this phase", &|_| {}).await, GateOutcome::Held(_)),
        "phase 1 is out of turns"
    );
    assert!(
        matches!(gate.run(2, "this phase", &|_| {}).await, GateOutcome::Refused(_)),
        "phase 2 is not"
    );
}

/// A reviewer that cannot answer holds the phase rather than approving it.
///
/// The break this catches: treating `Unavailable` as approval, so a provider outage waves every
/// phase through unreviewed. A refusal would be wrong too, since the agent cannot fix an outage.
#[tokio::test]
async fn an_unusable_review_holds_the_phase() {
    let gate = gate_with(PhaseReviewOutcome::Unavailable("provider down".into()), Ok("+ c".into()));
    let GateOutcome::Held(message) = gate.run(0, "this phase", &|_| {}).await else {
        panic!("an unanswered review is not approval")
    };
    assert!(message.contains("provider down"), "{message}");
}

/// A diff that cannot be read holds the phase too.
#[tokio::test]
async fn an_unreadable_diff_holds_the_phase() {
    let gate = gate_with(PhaseReviewOutcome::Continue, Err("not a repository".into()));
    assert!(matches!(gate.run(0, "this phase", &|_| {}).await, GateOutcome::Held(_)));
}

/// No change and nothing to read is no evidence, which is not an approval either.
#[tokio::test]
async fn a_claim_with_no_evidence_holds_the_phase() {
    let gate = gate_with(PhaseReviewOutcome::Continue, Ok("  \n".into()));
    assert!(matches!(gate.run(0, "this phase", &|_| {}).await, GateOutcome::Held(_)));
}
