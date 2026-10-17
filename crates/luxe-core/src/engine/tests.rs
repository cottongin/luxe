use super::{ActiveGuidedReview, Engine, ReviewLoadRequestId, ReviewRevisionId};
use crate::guided_review::{
    CheckSummary, GuidedReview, ReviewBase, ReviewContext, ReviewId, ReviewSource, ReviewStats,
    ReviewTarget,
};
use crate::provider::{
    CompletionRequest, ModelInfo, Provider, ProviderError, ProviderEvent, StopReason,
};
use futures::stream::{self, BoxStream};
use std::sync::Arc;

pub(super) struct OversizedGuideProvider;

#[async_trait::async_trait]
impl Provider for OversizedGuideProvider {
    fn name(&self) -> &str {
        "oversized-guide"
    }

    async fn stream(
        &self,
        _request: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        Ok(Box::pin(stream::iter(vec![
            ProviderEvent::TextDelta {
                index: 0,
                delta: "x".repeat(crate::guided_review::MAX_GUIDE_OUTPUT_BYTES + 1),
            },
            ProviderEvent::Done { reason: StopReason::EndTurn },
        ])))
    }
}

struct ControlCharacterGuideProvider;

#[async_trait::async_trait]
impl Provider for ControlCharacterGuideProvider {
    fn name(&self) -> &str {
        "control-character-guide"
    }

    async fn stream(
        &self,
        request: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        // Echo back exactly the item IDs the batch supplied, so coverage holds and only the
        // control-character filtering is under test.
        let prompt = request
            .messages
            .last()
            .and_then(|message| {
                message.content.iter().find_map(|block| match block {
                    crate::message::ContentBlock::Text(text) => Some(text.clone()),
                    _ => None,
                })
            })
            .unwrap_or_default();
        let item_ids: Vec<String> = prompt
            .match_indices("\"item_id\":\"")
            .filter_map(|(index, marker)| {
                let rest = &prompt[index + marker.len()..];
                rest.find('"').map(|end| rest[..end].to_owned())
            })
            .collect();
        let delta = serde_json::json!({
            "chapters": [{
                "title": "Core\u{0007}",
                "rationale": "Groups\u{0007} source.",
                "kind": "core",
                "item_ids": item_ids,
            }]
        })
        .to_string();
        Ok(Box::pin(stream::iter(vec![
            ProviderEvent::TextDelta { index: 0, delta },
            ProviderEvent::Done { reason: StopReason::EndTurn },
        ])))
    }
}

fn empty_review() -> GuidedReview {
    let revision_id = ReviewRevisionId::from_canonical_bytes(b"review source");
    GuidedReview {
        conversation: None,
        id: ReviewId::new(),
        revision_id: revision_id.clone(),
        target: ReviewTarget::WorkingTree,
        source: ReviewSource {
            base: ReviewBase::EmptyTree,
            content_digest: revision_id,
            includes_staged: false,
            includes_unstaged: false,
            included_untracked_files: 0,
            captured_at_ms: 0,
            uncovered_paths: Vec::new(),
        },
        context: ReviewContext {
            title: "No changes".into(),
            goal: None,
            phase_index: None,
            phase_title: None,
            planning_excerpt: None,
            acceptance_excerpt: None,
            source_intent: None,
            source_commits: 0,
            pull_request: None,
        },
        files: Vec::new(),
        chapters: Vec::new(),
        checks: CheckSummary::default(),
        stats: ReviewStats::default(),
        generated_at_ms: 0,
        insight: None,
        provenance: None,
        seen: None,
        brief: None,
    }
}

/// A review with nothing to chapter never reaches the provider.
#[tokio::test]
async fn an_empty_review_needs_no_guide_pass() {
    let (_keep, cancel) = tokio::sync::watch::channel(false);
    let cancel = crate::engine::review_state::GuideCancel::detached(cancel);

    assert!(Engine::collect_guide(
        Arc::new(OversizedGuideProvider),
        ModelInfo::default(),
        cancel,
        &empty_review(),
        None,
    )
    .await
    .is_none());
}

/// An unusable reply keeps the structural chapters *and* says why, so "structural guide" is not the
/// only thing a reviewer ever learns about a Guide that never arrived.
#[tokio::test]
async fn oversized_provider_guide_output_uses_structural_grouping() {
    let (_keep, cancel) = tokio::sync::watch::channel(false);
    let cancel = crate::engine::review_state::GuideCancel::detached(cancel);
    let review = single_file_review();

    let outcome = Engine::collect_guide(
        Arc::new(OversizedGuideProvider),
        ModelInfo::default(),
        cancel,
        &review,
        None,
    )
    .await
    .expect("a failed pass still reports its outcome");
    let (chapters, provenance) = (outcome.chapters, outcome.provenance);

    assert_eq!(chapters, review.chapters, "the structural chapters stand");
    assert_eq!(
        provenance,
        crate::guided_review::GuideProvenance::Structural {
            reason: Some(crate::guided_review::GuideFallback::Truncated)
        }
    );
}

#[tokio::test]
async fn provider_guide_output_filters_controls_before_presentation() {
    let (_keep, cancel) = tokio::sync::watch::channel(false);
    let cancel = crate::engine::review_state::GuideCancel::detached(cancel);
    let review = single_file_review();
    let outcome = Engine::collect_guide(
        Arc::new(ControlCharacterGuideProvider),
        ModelInfo::default(),
        cancel,
        &review,
        None,
    )
    .await
    .expect("valid provider JSON should chapter the review");
    let (chapters, provenance) = (outcome.chapters, outcome.provenance);

    assert_eq!(chapters[0].title, "Core");
    assert_eq!(chapters[0].rationale, "Groups source.");
    assert_eq!(provenance, luxe_core_guide_provenance_model());
    assert_eq!(crate::guided_review::validate_chapter_coverage(&review.files, &chapters), Ok(()));
}

pub(super) fn luxe_core_guide_provenance_model() -> crate::guided_review::GuideProvenance {
    crate::guided_review::GuideProvenance::Model { batches: 1 }
}

/// A one-hunk review, so a Guide pass has something real to cover.
pub(super) fn single_file_review() -> GuidedReview {
    let mut review = empty_review();
    review.files = crate::guided_review::parse_unified_diff(
        "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,1 +1,1 @@\n-let value = 1;\n+let value = 2;\n",
    )
    .expect("fixture diff parses")
    .files;
    review
}

#[test]
fn stale_review_completion_cannot_replace_a_newer_active_request() {
    let mut active = ActiveGuidedReview::default();
    let first_request = ReviewLoadRequestId::new();
    let second_request = ReviewLoadRequestId::new();
    let first_revision = ReviewRevisionId::from_canonical_bytes(b"first review");
    let second_revision = ReviewRevisionId::from_canonical_bytes(b"second review");

    assert_eq!(active.begin_request(first_request.clone()).0, None);
    assert!(active.complete_if_current(&first_request, first_revision.clone()));
    assert_eq!(active.begin_request(second_request.clone()).0, Some(first_revision.clone()));

    assert!(!active.complete_if_current(&first_request, first_revision.clone()));
    assert!(!active.is_current(&first_request));
    assert_eq!(active.revision_id, Some(first_revision));

    assert!(active.complete_if_current(&second_request, second_revision.clone()));
    assert!(active.is_current(&second_request));
    assert_eq!(active.revision_id, Some(second_revision));
}

/// Superseding a review cancels the guide it replaced, rather than paying for an answer that can
/// no longer be published — `is_current` already refuses to publish it.
#[test]
fn a_superseded_review_stops_paying_for_the_guide_it_replaced() {
    let mut active = ActiveGuidedReview::default();

    let (_, first) = active.begin_request(ReviewLoadRequestId::new());
    assert!(!first.is_cancelled());

    let (_, second) = active.begin_request(ReviewLoadRequestId::new());
    assert!(first.is_cancelled(), "the one it replaced is told to stop");
    assert!(!second.is_cancelled(), "and the one replacing it is not");
}

#[test]
fn stale_review_context_failure_is_discarded_after_a_newer_request() {
    let mut active = ActiveGuidedReview::default();
    let context_request = ReviewLoadRequestId::new();
    let newer_request = ReviewLoadRequestId::new();

    active.begin_request(context_request.clone());
    active.begin_request(newer_request.clone());

    assert!(!active.is_current(&context_request));
    assert!(active.is_current(&newer_request));
}

/// A superseded request cannot complete, so its review is never published over a newer one's.
#[test]
fn a_superseded_request_cannot_complete() {
    let mut active = ActiveGuidedReview::default();
    let first_request = ReviewLoadRequestId::new();
    let second_request = ReviewLoadRequestId::new();
    let review = empty_review();

    active.begin_request(first_request.clone());
    assert!(active.complete_if_current(&first_request, review.revision_id.clone()));
    assert_eq!(active.revision_id.as_ref(), Some(&review.revision_id));

    active.begin_request(second_request);
    assert!(
        !active.complete_if_current(&first_request, ReviewRevisionId::from_canonical_bytes(b"x"))
    );
    assert_eq!(active.revision_id.as_ref(), Some(&review.revision_id), "the pinned revision holds");
}

/// Cuts the first reply off mid-object, then answers properly with a bigger budget.
///
/// Models the real failure: the guide's output allowance was too small, the JSON closed short, and
/// a truncated object is an invalid one — so the whole batch fell back to template titles, quietly,
/// with the review still rendering as if nothing had gone wrong.
struct TruncatesOnceGuideProvider {
    attempts: std::sync::atomic::AtomicUsize,
    budgets: std::sync::Mutex<Vec<u32>>,
}

#[async_trait::async_trait]
impl Provider for TruncatesOnceGuideProvider {
    fn name(&self) -> &str {
        "truncates-once"
    }

    async fn stream(
        &self,
        request: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        self.budgets
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(request.max_tokens.unwrap_or(0));
        let attempt = self.attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let prompt = request
            .messages
            .last()
            .and_then(|message| {
                message.content.iter().find_map(|block| match block {
                    crate::message::ContentBlock::Text(text) => Some(text.clone()),
                    _ => None,
                })
            })
            .unwrap_or_default();
        let item_ids: Vec<String> = prompt
            .match_indices("\"item_id\":\"")
            .filter_map(|(index, marker)| {
                let rest = &prompt[index + marker.len()..];
                rest.find('"').map(|end| rest[..end].to_owned())
            })
            .collect();

        let (delta, reason) = if attempt == 0 {
            // A reply that stops mid-object, exactly as an exhausted output budget leaves it.
            (r#"{"chapters":[{"title":"Core","rationale":"Group"#.to_string(), StopReason::Length)
        } else {
            (
                serde_json::json!({
                    "chapters": [{
                        "title": "Real title",
                        "rationale": "Because the budget was enough this time.",
                        "kind": "core",
                        "item_ids": item_ids,
                    }],
                    "brief": {
                        "intent": "The guide was being cut off.",
                        "account": ["Doubles the allowance after a truncated reply."],
                        "verification": "Covered by this test.",
                    },
                })
                .to_string(),
                StopReason::EndTurn,
            )
        };
        Ok(Box::pin(stream::iter(vec![
            ProviderEvent::TextDelta { index: 0, delta },
            ProviderEvent::Done { reason },
        ])))
    }
}

#[tokio::test]
async fn a_cut_off_guide_is_retried_with_a_bigger_budget_before_falling_back() {
    let (_keep, cancel) = tokio::sync::watch::channel(false);
    let cancel = crate::engine::review_state::GuideCancel::detached(cancel);
    let review = single_file_review();
    let provider = Arc::new(TruncatesOnceGuideProvider {
        attempts: std::sync::atomic::AtomicUsize::new(0),
        budgets: std::sync::Mutex::new(Vec::new()),
    });

    let outcome = Engine::collect_guide(
        Arc::clone(&provider) as Arc<dyn Provider + Send + Sync>,
        ModelInfo::default(),
        cancel,
        &review,
        None,
    )
    .await
    .expect("the pass reports an outcome");

    assert_eq!(
        outcome.chapters[0].title, "Real title",
        "the retry's chapters are used, not the structural fallback"
    );
    assert!(
        matches!(outcome.provenance, crate::guided_review::GuideProvenance::Model { .. }),
        "and it is reported as model-written: {:?}",
        outcome.provenance
    );
    let brief = outcome.brief.expect("the retry also carried the brief");
    assert_eq!(brief.intent, "The guide was being cut off.");

    let budgets = provider.budgets.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
    assert_eq!(budgets.len(), 2, "exactly one retry, not a loop: {budgets:?}");
    assert_eq!(budgets[1], budgets[0] * 2, "and it doubles the allowance: {budgets:?}");
}

/// The one batch that writes the brief is asked for it; the others are not, or a multi-batch
/// review would produce a competing summary of the whole change per batch.
#[test]
fn exactly_one_batch_is_asked_for_the_brief() {
    let review = single_file_review();
    let batches = crate::guided_review::guide_batches(&review.context, &review.files, None);
    assert!(!batches.is_empty());
    assert_eq!(
        batches.iter().filter(|batch| batch.writes_brief).count(),
        1,
        "one brief, however many batches"
    );
    assert!(batches[0].writes_brief, "and it is the batch that always exists");
    assert!(batches[0].prompt.contains("\"brief\""), "the prompt actually asks for it");
}
