//! Engine guided-review handling — loading and refreshing a review.
//!
//! Bridges `Action::LoadGuidedReview` / `RefreshGuidedReview` to the luxe-core guided_review
//! model, assembling review sources off the runtime thread. Resolving a target, publishing what
//! loaded, the Guide pass after it and the turn baselines each live in a module of their own.

use super::*;
use crate::guided_review::{
    GuideFallback, GuideGroup, GuideManifestError, GuideProvenance, PullRequestResolveError,
    ResolvedPullRequest, ReviewChapter, ReviewContext, TurnProvenance, MAX_GUIDE_OUTPUT_BYTES,
};

mod baseline;
mod guide;
mod publish;
mod target;

use publish::{publish_review, ReviewPublish};
use target::{resolve_target, with_conversation};

/// Said when the engine has no project root, so no target can be captured at all.
const NO_PROJECT_ROOT: &str = "guided review loading is unavailable for this engine";

/// Said when the source captured but the project context around it did not.
const NO_REVIEW_CONTEXT: &str = "could not assemble project-local review context";

/// Reports a failed load to the front-end.
///
/// A closed channel means the UI is already gone, which is not itself worth reporting anywhere.
async fn report_failure(
    event_tx: &mpsc::Sender<Event>,
    request_id: ReviewLoadRequestId,
    target: ReviewTarget,
    error: GuidedReviewFailure,
) {
    let _ = event_tx.send(Event::GuidedReviewFailed { request_id, target, error }).await;
}

/// Whether a review may take the session's latest prompt as its goal and its transcript as its
/// record.
///
/// Only for the targets this session's work lives in: the working tree, the last turn, and a base
/// comparison, which runs through the working tree. A commit or a pull request is finished work
/// that names itself.
fn borrows_session(target: &ReviewTarget) -> bool {
    match target {
        // Your own pull request runs to the working tree, where this session's fixes live
        ReviewTarget::WorkingTree
        | ReviewTarget::LastTurn { .. }
        | ReviewTarget::Base { .. }
        | ReviewTarget::OwnPullRequest { .. } => true,
        ReviewTarget::Commit { .. } | ReviewTarget::PullRequest { .. } => false,
    }
}

impl Engine {
    pub(crate) async fn handle_load_guided_review(
        &self,
        request_id: ReviewLoadRequestId,
        target: ReviewTarget,
        event_tx: &mpsc::Sender<Event>,
    ) {
        debug!(request_id = %request_id.as_str(), ?target, "guided review load requested");
        if let Err(failure) = self.validate_last_turn_target(&target).await {
            report_failure(event_tx, request_id, target, failure).await;
            return;
        }
        let Some(project_root) = self.config.guided_review_project_root.clone() else {
            let failure = GuidedReviewFailure::unavailable(NO_PROJECT_ROOT);
            report_failure(event_tx, request_id, target, failure).await;
            return;
        };
        let (protected_revision, guide_cancel) =
            { self.active_guided_review.lock().await.begin_request(request_id.clone()) };
        let _ = event_tx
            .send(Event::GuidedReviewLoading {
                request_id: request_id.clone(),
                target: target.clone(),
            })
            .await;
        // Finished work names itself. A commit or a pull request was not made by whatever this
        // session last did, so its latest prompt is not the change's goal and its transcript is not
        // the change's record; either would put a claim about the wrong work in front of the
        // reviewer. A pull request does not live in this checkout either, so the graph of the
        // working tree is not its structure
        let (goal, provenance) = if borrows_session(&target) {
            (self.latest_user_goal().await, self.turn_provenance().await)
        } else {
            (None, None)
        };
        let insight_provider = self.insight_provider_for(&target);
        let load = ReviewLoad {
            github: self.config.github.clone(),
            context_files: self.config.review_context_files.clone(),
            goal,
            exclusions: self.review_exclusions(),
            protected_revision,
            publish: ReviewPublish {
                provenance,
                insight_provider,
                persistence_root: project_root.clone(),
                active_review: Arc::clone(&self.active_guided_review),
                guide: self.guide_run(guide_cancel).await,
                event_tx: event_tx.clone(),
                desk: self.review_desk(),
            },
            project_root,
            request_id,
            target,
        };
        tokio::spawn(load.run());
    }

    /// Refuses a Last Turn review whose turn has no complete baseline, retained or persisted.
    async fn validate_last_turn_target(
        &self,
        target: &ReviewTarget,
    ) -> Result<(), GuidedReviewFailure> {
        let ReviewTarget::LastTurn { session_id, turn_id } = target else {
            return Ok(());
        };
        let matches_retained_baseline =
            self.last_turn_baseline.lock().await.as_ref().is_some_and(|baseline| {
                baseline.session_id == *session_id && baseline.turn_id.as_str() == turn_id
            });
        let has_persisted_baseline = !matches_retained_baseline
            && self.has_persisted_turn_baseline(session_id, turn_id).await;
        if !matches_retained_baseline && !has_persisted_baseline {
            return Err(GuidedReviewFailure::unavailable(
                "the requested turn has no complete retained baseline",
            ));
        }
        Ok(())
    }

    /// The pass that annotates a review's files, for every target but a pull request.
    fn insight_provider_for(
        &self,
        target: &ReviewTarget,
    ) -> Option<Arc<dyn crate::guided_review::ReviewInsightProvider>> {
        match target {
            ReviewTarget::PullRequest { .. } => None,
            _ => self.config.review_insight_provider.clone(),
        }
    }

    /// Refreshes the open review, keeping its identity and its comments, if it is still at
    /// `expected_revision`.
    pub(crate) async fn handle_refresh_guided_review(
        &self,
        request_id: ReviewLoadRequestId,
        review_id: ReviewId,
        expected_revision: ReviewRevisionId,
        event_tx: &mpsc::Sender<Event>,
    ) {
        let review = match self.open_review(&review_id, Some(&expected_revision)).await {
            Ok((review, _)) => review,
            Err(message) => {
                // Nothing to name but the review the reader asked about, so the failure carries the
                // default target and says the rest in words
                let failure = GuidedReviewFailure {
                    kind: crate::event::GuidedReviewFailureKind::InvalidTarget,
                    message,
                };
                report_failure(event_tx, request_id, ReviewTarget::WorkingTree, failure).await;
                return;
            }
        };
        let target = review.target.clone();
        debug!(
            request_id = %request_id.as_str(),
            review_id = %review.id.as_str(),
            expected_revision = %expected_revision.as_str(),
            "guided review refresh requested"
        );
        let Some(project_root) = self.config.guided_review_project_root.clone() else {
            let failure = GuidedReviewFailure::unavailable(NO_PROJECT_ROOT);
            report_failure(event_tx, request_id, target, failure).await;
            return;
        };
        let (_, guide_cancel) =
            { self.active_guided_review.lock().await.begin_request(request_id.clone()) };
        let guide = self.guide_run(guide_cancel).await;
        // Same rules as a first load
        let provenance = if borrows_session(&target) { self.turn_provenance().await } else { None };
        let publish = ReviewPublish {
            provenance,
            insight_provider: self.insight_provider_for(&target),
            persistence_root: project_root.clone(),
            active_review: Arc::clone(&self.active_guided_review),
            guide,
            event_tx: event_tx.clone(),
            desk: self.review_desk(),
        };
        let github = self.config.github.clone();
        let _ = event_tx
            .send(Event::GuidedReviewLoading {
                request_id: request_id.clone(),
                target: target.clone(),
            })
            .await;
        let exclusions = self.review_exclusions();
        tokio::spawn(async move {
            // A pull request is read again, so a push since the last look is the new revision
            let mut context = review.context.clone();
            let mut git = LocalGit::new(&project_root).with_exclusions(exclusions);
            let mut threads = None;
            match resolve_target(github.as_ref(), &project_root, &target).await {
                Ok(Some(resolved)) => {
                    (git, threads, _) = adopt_pull_request(resolved, &mut context, git);
                }
                Ok(None) => {}
                Err(failure) => {
                    if publish.active_review.lock().await.is_current(&request_id) {
                        report_failure(&publish.event_tx, request_id, target, failure).await;
                    }
                    return;
                }
            }
            let outcome = git
                .refresh_target_review(
                    &review,
                    &expected_revision,
                    context,
                    UntrackedSourcePolicy::default(),
                )
                .await
                .map(|review| with_conversation(review, threads));
            publish_review(outcome, request_id, target, publish).await;
        });
    }
}

/// Points a capture at a resolved pull request: its own account of itself on the context, its pin
/// on the Git runner, and its threads for the review.
///
/// Returns the runner, the threads and the target the pull request resolved to.
fn adopt_pull_request(
    resolved: ResolvedPullRequest,
    context: &mut ReviewContext,
    git: LocalGit,
) -> (LocalGit, Option<Option<crate::github::ReviewThreads>>, ReviewTarget) {
    context.title = resolved.context.title.clone();
    context.pull_request = Some(resolved.context);
    (git.with_pull_request(resolved.pin), Some(resolved.threads), resolved.target)
}

/// A first load, gathered from the engine before it is spawned: the engine is not `'static`.
struct ReviewLoad {
    request_id: ReviewLoadRequestId,
    target: ReviewTarget,
    project_root: std::path::PathBuf,
    /// The session's latest prompt, when the target borrows the session
    goal: Option<String>,
    context_files: Vec<String>,
    github: Option<Arc<dyn crate::github::GitHub>>,
    exclusions: crate::guided_review::ReviewExclusions,
    /// The revision a reload of the same target keeps, so comments stay attached
    protected_revision: Option<ReviewRevisionId>,
    publish: ReviewPublish,
}

impl ReviewLoad {
    /// Assembles the context, resolves and captures the target, and publishes what came of it.
    async fn run(self) {
        let started = std::time::Instant::now();
        let Some(context) = self.load_review_context().await else {
            return;
        };
        debug!(elapsed_ms = started.elapsed().as_millis(), "guided review context assembled");
        let Some((outcome, target)) = self.capture_loaded_review(context).await else {
            return;
        };
        debug!(elapsed_ms = started.elapsed().as_millis(), "guided review captured");
        publish_review(outcome, self.request_id, target, self.publish).await;
    }

    /// The project-local context the review is read against, or `None` once its failure is
    /// reported (or dropped, for a request a newer one has replaced).
    async fn load_review_context(&self) -> Option<ReviewContext> {
        let (root, files, goal) =
            (self.project_root.clone(), self.context_files.clone(), self.goal.clone());
        let assembled = tokio::task::spawn_blocking(move || {
            ReviewContextDraft::from_project_local_sources(
                crate::guided_review::UNTITLED_REVIEW,
                &root,
                &files,
            )
            .map(|mut draft| {
                draft.goal = goal;
                draft.build()
            })
        })
        .await;
        match assembled {
            Ok(Ok(context)) => return Some(context),
            Ok(Err(error)) => {
                tracing::warn!(%error, "guided review context assembly failed");
                if !self.is_current().await {
                    debug!(request_id = %self.request_id.as_str(), "discarding stale guided review context failure");
                    return None;
                }
            }
            Err(error) => {
                tracing::warn!(%error, "guided review context task failed");
                if !self.is_current().await {
                    debug!(request_id = %self.request_id.as_str(), "discarding stale guided review context task failure");
                    return None;
                }
            }
        }
        let failure = GuidedReviewFailure::capture(NO_REVIEW_CONTEXT);
        report_failure(
            &self.publish.event_tx,
            self.request_id.clone(),
            self.target.clone(),
            failure,
        )
        .await;
        None
    }

    /// Resolves the target and captures it, or `None` once a target that cannot be resolved is
    /// reported. Answers the target the review is of, which a pull request may have refined.
    async fn capture_loaded_review(
        &self,
        mut context: ReviewContext,
    ) -> Option<(Result<GuidedReview, GuidedReviewError>, ReviewTarget)> {
        let resolved =
            match resolve_target(self.github.as_ref(), &self.project_root, &self.target).await {
                Ok(resolved) => resolved,
                Err(failure) => {
                    if self.is_current().await {
                        let (request_id, target) = (self.request_id.clone(), self.target.clone());
                        report_failure(&self.publish.event_tx, request_id, target, failure).await;
                    }
                    return None;
                }
            };
        let git = LocalGit::new(&self.project_root).with_exclusions(self.exclusions.clone());
        let (git, threads, target) = match resolved {
            Some(resolved) => adopt_pull_request(resolved, &mut context, git),
            None => (git, None, self.target.clone()),
        };
        let git = match &self.protected_revision {
            Some(revision_id) => git.preserving_revision(revision_id.clone()),
            None => git,
        };
        let outcome = git
            .load_target_review(
                ReviewId::new(),
                target.clone(),
                context,
                UntrackedSourcePolicy::default(),
            )
            .await
            .map(|review| with_conversation(review, threads));
        Some((outcome, target))
    }

    /// Whether this load is still the one the surface is waiting for.
    async fn is_current(&self) -> bool {
        self.publish.active_review.lock().await.is_current(&self.request_id)
    }
}

#[cfg(test)]
mod tests;
