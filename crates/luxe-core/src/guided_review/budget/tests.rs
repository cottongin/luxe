use super::*;
use crate::guided_review::AttentionKind;
use crate::guided_review::{
    parse_unified_diff, CommandRun, FileInsight, RelevantFile, ReviewContextDraft, ReviewId,
    ReviewInsight, ReviewRevisionId, ReviewSource, ReviewStats, SymbolContext, TurnProvenance,
};

/// A one-hunk change to `login`, whose hunk carries the symbol name the graph uses.
fn review() -> GuidedReview {
    let diff = "diff --git a/src/auth.rs b/src/auth.rs\n--- a/src/auth.rs\n+++ b/src/auth.rs\n@@ -1,1 +1,1 @@ fn login\n-let a = 1;\n+let a = 2;\n";
    let mut files = parse_unified_diff(diff).expect("fixture diff parses").files;
    files[0].hunks[0].symbol =
        Some(SymbolContext { kind: "function".into(), name: Some("login".into()) });
    let revision_id = ReviewRevisionId::from_canonical_bytes(diff.as_bytes());
    GuidedReview {
        conversation: None,
        id: ReviewId::new(),
        revision_id: revision_id.clone(),
        target: crate::guided_review::ReviewTarget::WorkingTree,
        source: ReviewSource {
            base: crate::guided_review::ReviewBase::EmptyTree,
            content_digest: revision_id,
            includes_staged: false,
            includes_unstaged: true,
            included_untracked_files: 0,
            captured_at_ms: 0,
            uncovered_paths: Vec::new(),
        },
        context: ReviewContextDraft::default().build(),
        stats: ReviewStats::from_files(&files),
        chapters: crate::guided_review::fallback_chapters(&files).expect("chapters"),
        files,
        checks: crate::guided_review::CheckSummary::default(),
        generated_at_ms: 0,
        insight: None,
        provenance: None,
        seen: None,
        brief: None,
    }
}

fn covered_insight() -> ReviewInsight {
    ReviewInsight {
        files: vec![FileInsight {
            file: "src/auth.rs".into(),
            relevant: vec![RelevantFile {
                file: "tests/auth.rs".into(),
                reason: "test_login tests login".into(),
            }],
        }],
        untested_symbols: Vec::new(),
        covered_symbols: vec!["src/auth.rs::login".into()],
    }
}

fn provenance(command: &str, failed: bool, verified_after_last_edit: bool) -> TurnProvenance {
    TurnProvenance {
        written_paths: ["src/auth.rs".to_string()].into_iter().collect(),
        commands: vec![CommandRun {
            command: command.into(),
            failed,
            tool: "bash".into(),
            after_last_edit: verified_after_last_edit,
        }],
        verified_after_last_edit,
        ..TurnProvenance::default()
    }
}

/// Nothing vouches for a bare change, so it is the reviewer's to judge.
#[test]
fn an_unvouched_change_needs_review() {
    let budget = review_budget(&review(), &[]);

    assert_eq!(budget.needs_review, 1);
    assert_eq!(budget.accounted, 0);
    assert!(!budget.is_fully_accounted());
}

/// The whole point: a passing test that ran *after* the last edit, over a symbol the graph says
/// it covers, is evidence a reviewer can spend.
#[test]
fn a_covered_symbol_with_a_test_run_after_the_last_edit_is_accounted_for() {
    let mut review = review();
    review.insight = Some(covered_insight());
    review.provenance = Some(provenance("cargo nextest run --workspace", false, true));

    let budget = review_budget(&review, &[]);

    assert_eq!(budget.needs_review, 0);
    assert!(budget.is_fully_accounted());
    assert_eq!(budget.reasons[0].0, "covered by `cargo nextest run --workspace`");
}

/// A green run *before* the final edit is evidence about a different program.
#[test]
fn coverage_before_the_last_edit_vouches_for_nothing() {
    let mut review = review();
    review.insight = Some(covered_insight());
    review.provenance = Some(provenance("cargo nextest run", false, false));

    assert_eq!(review_budget(&review, &[]).needs_review, 1);
}

/// A failing run is not evidence either, and neither is a command that never tested anything.
#[test]
fn only_a_passing_test_command_counts_as_coverage() {
    let mut failed = review();
    failed.insight = Some(covered_insight());
    failed.provenance = Some(provenance("cargo nextest run", true, true));
    assert_eq!(review_budget(&failed, &[]).needs_review, 1, "a failed run vouches for nothing");

    let mut unrelated = review();
    unrelated.insight = Some(covered_insight());
    unrelated.provenance = Some(provenance("cargo fmt --all", false, true));
    assert_eq!(review_budget(&unrelated, &[]).needs_review, 1, "formatting is not a test");
}

/// Risk outranks every excuse: "a test passed" is not the question a credential-shaped line asks.
#[test]
fn a_risk_signal_keeps_an_otherwise_accounted_hunk_on_the_reading_list() {
    let mut review = review();
    review.insight = Some(covered_insight());
    review.provenance = Some(provenance("cargo nextest run", false, true));
    let hunk_id = review.files[0].hunks[0].id.clone();
    let signal = AttentionSignal {
        kind: AttentionKind::SecretShaped,
        path: Some("src/auth.rs".into()),
        detail: "adds a line that looks like a credential".into(),
        aggregate: false,
        file_id: Some(review.files[0].id.clone()),
        hunk_id: Some(hunk_id.clone()),
    };

    let budget = review_budget(&review, std::slice::from_ref(&signal));

    assert!(budget.hunk_needs_review(&hunk_id));
}

/// Two hunks in `src/auth.rs` and one in `src/other.rs`, all formatting churn, so every item
/// starts out accounted for and a signal is the only thing that can put one back on the list.
fn churn_review() -> GuidedReview {
    let diff = "diff --git a/src/auth.rs b/src/auth.rs\n--- a/src/auth.rs\n+++ b/src/auth.rs\n@@ -1,1 +1,1 @@\n-let a = 1;\n+let a = 1 ;\n@@ -20,1 +20,1 @@\n-let b = 2;\n+let b = 2 ;\ndiff --git a/src/other.rs b/src/other.rs\n--- a/src/other.rs\n+++ b/src/other.rs\n@@ -1,1 +1,1 @@\n-let c = 3;\n+let c = 3 ;\n";
    let mut files = parse_unified_diff(diff).expect("fixture diff parses").files;
    for hunk in files.iter_mut().flat_map(|file| file.hunks.iter_mut()) {
        hunk.noise = NoiseKind::FormattingOnly;
    }
    let mut review = review();
    review.stats = ReviewStats::from_files(&files);
    review.chapters = crate::guided_review::fallback_chapters(&files).expect("chapters");
    review.files = files;
    review
}

fn signal(path: Option<&str>, hunk_id: Option<HunkId>, aggregate: bool) -> AttentionSignal {
    AttentionSignal {
        kind: AttentionKind::SecretShaped,
        path: path.map(str::to_owned),
        detail: "adds a line that looks like a credential".into(),
        aggregate,
        file_id: None,
        hunk_id,
    }
}

/// A signal on one hunk flags that hunk, not its neighbors.
#[test]
fn a_hunk_signal_flags_only_its_hunk() {
    let review = churn_review();
    let (first, second) = (&review.files[0].hunks[0].id, &review.files[0].hunks[1].id);

    let budget = review_budget(&review, &[signal(None, Some(first.clone()), false)]);

    assert!(budget.hunk_needs_review(first));
    assert!(!budget.hunk_needs_review(second), "the other hunk in the file is still churn");
    assert!(!budget.hunk_needs_review(&review.files[1].hunks[0].id));
}

/// A signal on a file flags every hunk in it, and nothing in any other file.
#[test]
fn a_file_signal_flags_every_hunk_in_that_file_only() {
    let review = churn_review();

    let budget = review_budget(&review, &[signal(Some("src/auth.rs"), None, false)]);

    assert!(review.files[0].hunks.iter().all(|hunk| budget.hunk_needs_review(&hunk.id)));
    assert!(!budget.hunk_needs_review(&review.files[1].hunks[0].id), "a different file");
}

/// An aggregate row names one file as an example of many, so it flags none of them.
#[test]
fn an_aggregate_signal_flags_nothing() {
    let review = churn_review();

    let budget = review_budget(&review, &[signal(Some("src/auth.rs"), None, true)]);

    assert_eq!(budget.needs_review, 0, "{:?}", budget.reasons);
}

/// Folded churn is accounted for by its class, with the class named.
#[test]
fn formatting_churn_is_accounted_for_by_its_classification() {
    let mut review = review();
    review.files[0].hunks[0].noise = NoiseKind::FormattingOnly;

    let budget = review_budget(&review, &[]);

    assert_eq!(budget.accounted, 1);
    assert_eq!(budget.reasons[0].0, "formatting only");
}

/// Who wrote a change is not evidence about it. A file the agent never touched stays on the
/// reading list, because the guess behind "not written by the agent" once folded a whole review
/// away as someone else's work.
#[test]
fn a_file_the_agent_never_wrote_still_needs_review() {
    let mut review = review();
    review.provenance = Some(TurnProvenance {
        written_paths: ["src/other.rs".to_string()].into_iter().collect(),
        ..TurnProvenance::default()
    });

    let budget = review_budget(&review, &[]);

    assert_eq!(budget.needs_review, 1);
    assert_eq!(budget.accounted, 0);
}

/// A chapter whose items are all accounted for can present as one line.
#[test]
fn chapter_roll_up_says_whether_a_chapter_asks_anything() {
    let mut review = review();
    review.files[0].hunks[0].noise = NoiseKind::Lockfile;
    let budget = review_budget(&review, &[]);

    let chapter = &review.chapters[0];
    assert!(!chapter_needs_review(&budget, chapter));
    assert_eq!(budget.chapter(&chapter.id).accounted, 1);
    assert_eq!(budget.chapter(&chapter.id).total(), 1);
}

/// ...and one with an unvouched item counts it.
#[test]
fn chapter_roll_up_counts_what_still_needs_review() {
    let review = review();
    let budget = review_budget(&review, &[]);

    let chapter = &review.chapters[0];
    assert!(chapter_needs_review(&budget, chapter));
    assert_eq!(budget.chapter(&chapter.id).needs_review, 1);
}
