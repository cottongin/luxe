//! Publishing — making a review the open one and binding its session.

use super::*;

impl ReviewDesk {
    /// Makes `review` the open one and binds its session: the held one across a refresh, the
    /// stored one after a restart, or a new one.
    pub(super) async fn publish(
        &mut self,
        request_id: ReviewLoadRequestId,
        review: GuidedReview,
    ) -> Vec<Effect> {
        let mut effects = vec![Effect::Event(Event::GuidedReviewLoaded {
            request_id,
            review: Box::new(review.clone()),
        })];
        let carried = self.session.as_ref().is_some_and(|session| session.id == review.id);
        let mut interrupted = 0;
        if !carried {
            self.answers.clear();
            match self.load(&review.id).await {
                Some(stored) => {
                    self.session = Some(stored);
                    self.persisted = true;
                }
                None => {
                    self.session = Some(ReviewSession::new(
                        review.id.clone(),
                        review.target.clone(),
                        review.source.clone(),
                        review.generated_at_ms,
                    ));
                    self.persisted = false;
                }
            }
        }
        self.review = Some(review);
        let Some(mut candidate) = self.bound() else {
            return effects;
        };
        // A session read from disk outlived the process that dispatched it, so a turn it says is
        // running died with that process. Retryable is the honest state, and it never redispatches
        // on its own.
        if !carried {
            let dispatched: Vec<SubmissionId> = candidate
                .submissions
                .iter()
                .filter(|submission| submission.dispatch == SubmissionDispatch::Dispatched)
                .map(|submission| submission.id.clone())
                .collect();
            for submission_id in &dispatched {
                if candidate.fail_submission(submission_id).is_ok() {
                    interrupted += 1;
                }
            }
        }
        if self.persisted {
            if self.session.as_ref() != Some(&candidate) {
                let request_id = ReviewSessionSaveRequestId::new();
                match self.save(&candidate).await {
                    Ok(stored) => self.session = Some(stored),
                    Err(error) => effects.push(self.save_failed(request_id, &error).await),
                }
            }
        } else {
            // Nothing is on disk to guard yet. New threads join in place, and the first edit
            // saves them with everything else.
            self.session = Some(candidate);
        }
        if interrupted > 0 {
            effects.push(Effect::Event(Event::Notice(
                "A review feedback turn was cut off when luxe last stopped. It can be sent again \
                 from the review."
                    .into(),
            )));
        }
        if let Some(session) = self.session.clone() {
            effects
                .push(Effect::Event(Event::ReviewSessionRestored { session: Box::new(session) }));
        }
        effects
    }

    /// The held session as it should stand against the held review: moved onto a refreshed source,
    /// and, for the reader's own pull request, carrying its threads.
    ///
    /// Every change starts from this, so a reconciliation that could not be saved when the refresh
    /// arrived is saved by the next change rather than lost.
    pub(super) fn bound(&self) -> Option<ReviewSession> {
        let review = self.review.as_ref()?;
        let mut session = self.session.clone().filter(|session| session.id == review.id)?;
        if session.source != review.source {
            session.reconcile_refreshed_source(review.source.clone(), |anchor| {
                remap_finding_anchor(anchor, &review.files, &review.chapters)
            });
        }
        if matches!(review.target, ReviewTarget::OwnPullRequest { .. }) {
            session.adopt_github_threads(thread_findings(review));
        }
        Some(session)
    }
}
