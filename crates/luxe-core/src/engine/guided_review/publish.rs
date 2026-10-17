//! Publish — enrich, claim, persist and announce a loaded or refreshed review.

use super::*;

/// Everything a completed review load needs in order to be published.
///
/// Gathered before the spawn because the engine is not `'static` and cannot be borrowed across it;
/// bundled because otherwise [`publish_review`] takes nine positional arguments and every caller
/// gets to put them in the wrong order once.
pub(super) struct ReviewPublish {
    /// Which agent turn produced the change under review, when there was one.
    pub(super) provenance: Option<TurnProvenance>,
    /// Optional pass that annotates the files with a model's read of them.
    pub(super) insight_provider: Option<Arc<dyn crate::guided_review::ReviewInsightProvider>>,
    /// Project root the review's artifacts are stored under.
    pub(super) persistence_root: std::path::PathBuf,
    /// The in-flight request, so a superseded load can be discarded rather than shown.
    pub(super) active_review: Arc<tokio::sync::Mutex<ActiveGuidedReview>>,
    /// What the Guide pass that follows runs on, and bills
    pub(super) guide: crate::engine::guide_run::GuideRunContext,
    /// The front-end, for failures and the Guide pass's spend.
    pub(super) event_tx: mpsc::Sender<Event>,
    /// Where the review is published: the desk binds its session and announces both.
    pub(super) desk: crate::engine::review_desk::ReviewDeskHandle,
}

/// Publishes a loaded or refreshed review: enrich, claim, persist, start the Guide, announce.
///
/// Load and refresh differ only in where the review came from; everything after that — including
/// the staleness check that makes a superseded request disappear instead of overwriting a newer
/// one — has to be identical, so it is written once.
pub(super) async fn publish_review(
    outcome: Result<GuidedReview, GuidedReviewError>,
    request_id: ReviewLoadRequestId,
    target: ReviewTarget,
    publish: ReviewPublish,
) {
    let ReviewPublish {
        provenance,
        insight_provider,
        persistence_root,
        active_review,
        guide,
        event_tx,
        desk,
    } = publish;

    let mut review = match outcome {
        Ok(review) => review,
        Err(error) => {
            if !active_review.lock().await.is_current(&request_id) {
                debug!(request_id = %request_id.as_str(), "discarding stale guided review failure");
                return;
            }
            let failure = GuidedReviewFailure::from_error(&error);
            report_failure(&event_tx, request_id, target, failure).await;
            return;
        }
    };

    if let Some(provider) = &insight_provider {
        let started = std::time::Instant::now();
        review.insight = provider.insight(&review.files).await;
        debug!(elapsed_ms = started.elapsed().as_millis(), "guided review insight computed");
    }
    review.provenance = provenance;
    // What the last review of this target left behind, so this one can lead with what is new
    // rather than re-presenting work the reviewer already read.
    review.seen =
        crate::guided_review::SeenReviewStore::new(&persistence_root).load(&review.target);

    // Claim the request before anything is written and hold it until published: a newer load
    // owns the surface, and one begun mid-write must find this one already landed, never after.
    let mut active = active_review.lock().await;
    if !active.complete_if_current(&request_id, review.revision_id.clone()) {
        debug!(request_id = %request_id.as_str(), "discarding stale guided review result");
        return;
    }
    save_record(persistence_root.clone(), &review, "loaded review").await;
    #[cfg(test)]
    super::tests::paused_before_desk_send(&persistence_root).await;

    // The diff goes on screen now, with deterministic chapters; the model-authored Guide arrives
    // as an in-place upgrade rather than a load-time stall.
    Engine::spawn_guide_generation(
        guide,
        Arc::clone(&active_review),
        review.clone(),
        request_id.clone(),
        persistence_root,
        event_tx,
        desk.clone(),
    );
    desk.send(crate::engine::review_desk::DeskCommand::Publish {
        request_id,
        review: Box::new(review),
    });
}

/// Writes `review`'s active-review record under `root`, saying so when it cannot.
pub(super) async fn save_record(
    root: std::path::PathBuf,
    review: &GuidedReview,
    which: &'static str,
) {
    #[cfg(test)]
    super::tests::paused_before_record_save(&root).await;
    let record = ActiveReviewRecord::from_review(review);
    let store = ActiveReviewRecordStore::new(root);
    match tokio::task::spawn_blocking(move || store.save(&record)).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => warn!(%error, which, "could not persist the active guided review"),
        Err(error) => warn!(%error, which, "active guided review persistence task failed"),
    }
}
