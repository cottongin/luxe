//! Engine startup and shutdown — what a session does before its first action and after its last.

use super::*;

impl Engine {
    /// Loads the session, says what the front-end and the instruction files need said, restores
    /// the last turn's baseline and the review that was open, and fires `SessionStart`.
    pub(super) async fn start(&mut self, event_tx: &mpsc::Sender<Event>) {
        let sid = self.session_id.lock().await.clone();
        debug!(session_id = %sid, "engine started");

        if let Err(error) = self.load_session(sid.to_string(), "started", event_tx.clone()).await {
            error!(%error, session_id = %sid, "failed to initialize session");
            let _ = event_tx.send(Event::Error(format!("initialize session: {error}"))).await;
        }

        // Anything wrong with the instruction files, said once, to the only party who can fix it.
        for notice in prompt::instruction_notices(&self.instruction_dir()) {
            let _ = event_tx.send(Event::Notice(notice)).await;
        }

        // And whatever the front-end found before there was an engine to tell.
        for notice in std::mem::take(&mut self.config.startup_notices) {
            let _ = event_tx.send(Event::Notice(notice)).await;
        }

        if let Some(baseline) = self.restore_persisted_last_turn_baseline().await {
            let _ = event_tx
                .send(Event::LastTurnAvailable {
                    session_id: baseline.session_id.clone(),
                    turn_id: baseline.turn_id.as_str().to_string(),
                })
                .await;
            *self.last_turn_baseline.lock().await = Some(baseline);
        }
        // The review that was open comes back with its comments: the desk reads the stored session
        // and announces both, under a request no front end sent
        if let Some(review) = self.restore_persisted_guided_review().await {
            let request_id = ReviewLoadRequestId::new();
            let target = review.target.clone();
            let revision_id = review.revision_id.clone();
            self.active_guided_review.lock().await.begin_request(request_id.clone());
            let _ = event_tx
                .send(Event::GuidedReviewLoading { request_id: request_id.clone(), target })
                .await;
            if self.active_guided_review.lock().await.complete_if_current(&request_id, revision_id)
            {
                self.review_desk.send(super::review_desk::DeskCommand::Publish {
                    request_id,
                    review: Box::new(review),
                });
            }
        }

        self.session_hook(HookPoint::SessionStart, event_tx).await;

        // Register file-changed hook (format-on-save + diagnostics) if formatter registry is configured.
        if self.config.formatter_registry.is_some() {
            use std::sync::Arc;
            self.config.hook_registry.register(Arc::new(
                crate::hooks::file_changed::FileChangedHook::new(
                    self.config.formatter_registry.clone(),
                    None, // LSP client not yet wired
                ),
            ));
        }
    }

    /// Stops what runs beside the session, settles what it still owes, and fires `SessionEnd`,
    /// answering what the engine was set to for the one that takes over from it.
    pub(super) async fn stop(&self, event_tx: &mpsc::Sender<Event>) -> Carryover {
        self.cache_warmer.stop();
        self.active_guided_review.lock().await.end();
        self.flush_review_desk().await;
        self.flush_usage_at_current_leaf(event_tx).await;
        self.session_hook(HookPoint::SessionEnd, event_tx).await;
        debug!("engine stopped");
        self.carryover().await
    }
}
