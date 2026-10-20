//! Storage — reading and writing the open review's session, and the progress and seen records.

use super::*;

/// Why a session could not be written.
#[derive(Debug)]
pub(super) enum SaveError {
    /// This engine has no project to keep the session in
    NoProject,
    /// The store refused, or the disk did
    Store(ReviewSessionStoreError),
    /// The blocking write never reported back
    Task,
}

impl ReviewDesk {
    pub(super) async fn record_progress(
        &mut self,
        review_id: &ReviewId,
        progress: ReviewProgress,
    ) -> Vec<Effect> {
        if self.review.as_ref().is_none_or(|review| &review.id != review_id) {
            return Vec::new();
        }
        let Some(mut candidate) = self.bound() else {
            return Vec::new();
        };
        if candidate.progress == progress {
            return Vec::new();
        }
        candidate.progress = progress;
        match self.save(&candidate).await {
            Ok(stored) => {
                self.session = Some(stored.clone());
                vec![saved(ReviewSessionSaveRequestId::new(), stored)]
            }
            // Where the reading stopped is worth keeping, not worth an error in the transcript;
            // the next step tries again with everything walked by then.
            Err(error) => {
                warn!(?error, "could not save how far the review was read");
                Vec::new()
            }
        }
    }

    pub(super) async fn record_seen(&self, review_id: &ReviewId) {
        let (Some(root), Some(review), Some(session)) =
            (self.project_root.clone(), self.review.as_ref(), self.session.as_ref())
        else {
            return;
        };
        if &review.id != review_id {
            return;
        }
        let Some(seen) =
            SeenReview::from_walked(review, &session.progress.walked_chapter_ids, now_ms())
        else {
            return;
        };
        let target = review.target.clone();
        // An optimisation for the next review, never a reason to fail this one
        match tokio::task::spawn_blocking(move || SeenReviewStore::new(&root).record(&target, seen))
            .await
        {
            Ok(Ok(())) => {}
            Ok(Err(error)) => warn!(%error, "could not record what this review covered"),
            Err(error) => warn!(%error, "recording what this review covered did not finish"),
        }
    }

    pub(super) async fn load(&self, review_id: &ReviewId) -> Option<ReviewSession> {
        let root = self.project_root.clone()?;
        let review_id = review_id.clone();
        match tokio::task::spawn_blocking(move || ReviewSessionStore::new(root).load(&review_id))
            .await
        {
            Ok(Ok(session)) => session,
            Ok(Err(error)) => {
                warn!(%error, "could not read this review's stored comments");
                None
            }
            Err(error) => {
                warn!(%error, "reading this review's stored comments did not finish");
                None
            }
        }
    }

    /// Writes `candidate` over the stored revision, answering what the store now holds.
    pub(super) async fn save(
        &mut self,
        candidate: &ReviewSession,
    ) -> Result<ReviewSession, SaveError> {
        let root = self.project_root.clone().ok_or(SaveError::NoProject)?;
        let expected = if self.persisted {
            self.session.as_ref().map(|session| session.revision)
        } else {
            None
        };
        let candidate = candidate.clone();
        let stored = tokio::task::spawn_blocking(move || {
            ReviewSessionStore::new(root).save(expected, &candidate, now_ms())
        })
        .await
        .map_err(|_| SaveError::Task)?
        .map_err(SaveError::Store)?;
        self.persisted = true;
        Ok(stored)
    }

    /// The refusal for a write that failed, after taking whatever the store holds now when the
    /// failure was another writer getting there first.
    pub(super) async fn save_failed(
        &mut self,
        request_id: ReviewSessionSaveRequestId,
        error: &SaveError,
    ) -> Effect {
        let review_id = self
            .session
            .as_ref()
            .map(|session| session.id.clone())
            .or_else(|| self.review.as_ref().map(|review| review.id.clone()))
            .unwrap_or_default();
        let message = match error {
            SaveError::NoProject => {
                "Review comments cannot be saved without a project.".to_string()
            }
            SaveError::Store(ReviewSessionStoreError::RevisionConflict { .. }) => {
                // Another luxe on this project wrote first. Holding on to the older revision would
                // refuse every later change too, so take theirs and say so.
                if let Some(stored) = self.load(&review_id).await {
                    self.session = Some(stored);
                    self.persisted = true;
                }
                "Another luxe changed this review's comments at the same time, so this change was \
                 not saved. The review shows theirs now; make the change again."
                    .to_string()
            }
            SaveError::Store(error) => {
                warn!(%error, "could not save the review's comments");
                "The review's comments could not be saved, so nothing changed.".to_string()
            }
            SaveError::Task => {
                warn!("saving the review's comments did not finish");
                "The review's comments could not be saved, so nothing changed.".to_string()
            }
        };
        Effect::Event(Event::ReviewSessionSaveFailed { request_id, review_id, message })
    }
}
