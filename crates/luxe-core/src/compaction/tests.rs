use super::*;
use crate::provider::ModelRef;

#[test]
fn circuit_breaker_trips_after_the_failure_cap() {
    assert!(!compaction_circuit_open(0));
    assert!(!compaction_circuit_open(MAX_COMPACTION_FAILURES - 1));
    assert!(compaction_circuit_open(MAX_COMPACTION_FAILURES));
    assert!(compaction_circuit_open(MAX_COMPACTION_FAILURES + 5));
}

/// The backstop behind the arithmetic: a summary that freed almost nothing has spent a provider
/// call to leave occupancy exactly where it was, and the next iteration will ask again.
#[test]
fn a_compaction_that_freed_almost_nothing_does_not_count_as_working() {
    assert!(compaction_was_effective(1_000, 800), "a fifth freed is the bar");
    assert!(compaction_was_effective(1_000, 100), "and most of it, comfortably");
    assert!(!compaction_was_effective(1_000, 801), "just under it is not");
    assert!(!compaction_was_effective(1_000, 900), "nor a tenth");
    // A transcript that somehow grew has not freed anything, rather than a negative share.
    assert!(!compaction_was_effective(1_000, 1_200));
    // Nothing to free is not a failure to free it — the alternative reads as a broken
    // summarizer and trips a breaker over an empty session.
    assert!(compaction_was_effective(0, 0));
}

#[test]
fn summarizer_prefers_the_override_else_active() {
    let active = ModelInfo { r#ref: ModelRef::new("openai", "gpt-primary"), ..Default::default() };
    // No override -> the active model writes the summary.
    assert_eq!(summarizer_model(None, &active).r#ref.id, "gpt-primary");
    // Override -> that ref (provider + id), so a cheap cross-provider summarizer is honored.
    let cheap =
        ModelInfo { r#ref: ModelRef::new("anthropic", "claude-haiku"), ..Default::default() };
    let picked = summarizer_model(Some(&cheap), &active);
    assert_eq!(picked.r#ref.provider, "anthropic");
    assert_eq!(picked.r#ref.id, "claude-haiku");
}
