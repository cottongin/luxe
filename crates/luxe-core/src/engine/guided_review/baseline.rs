//! Baseline — the turn snapshots and persisted review a restarted engine picks up again.

use super::*;
use crate::guided_review::{
    prune_turn_baselines, LastTurnRecordStoreError, UNREFERENCED_BASELINE_AGE,
};

impl Engine {
    /// Restores a complete retained Last Turn baseline without reading the mutable workspace.
    pub(crate) async fn restore_persisted_last_turn_baseline(
        &self,
    ) -> Option<RetainedTurnBaseline> {
        let project_root = self.config.guided_review_project_root.clone()?;
        let restored =
            tokio::task::spawn_blocking(move || -> Result<Option<RetainedTurnBaseline>, String> {
                let store = LastTurnRecordStore::new(&project_root);
                let record = store.load().map_err(|error| error.to_string())?;
                let Some(record) = record else {
                    return Ok(None);
                };
                TurnBaselineArtifact::load(&project_root, record.baseline_id())
                    .map_err(|error| error.to_string())?;
                TurnBaselineArtifact::load(&project_root, record.after_snapshot_id())
                    .map_err(|error| error.to_string())?;
                Ok(Some(RetainedTurnBaseline {
                    session_id: record.session_id,
                    turn_id: AgentTurnId::from(record.turn_id),
                }))
            })
            .await;
        match restored {
            Ok(Ok(baseline)) => baseline,
            Ok(Err(error)) => {
                warn!(%error, "could not restore persisted Last Turn baseline");
                None
            }
            Err(error) => {
                warn!(%error, "persisted Last Turn restore task failed");
                None
            }
        }
    }

    /// What this engine leaves out of a review: luxe's own directory, plus `review.exclude`.
    pub(crate) fn review_exclusions(&self) -> crate::guided_review::ReviewExclusions {
        crate::guided_review::ReviewExclusions::new(&self.config.review_exclude)
    }

    /// A Git runner that already knows what this project leaves out of its reviews.
    pub(in crate::engine) fn review_git(&self, project_root: std::path::PathBuf) -> LocalGit {
        LocalGit::new(project_root).with_exclusions(self.review_exclusions())
    }

    pub(crate) async fn restore_persisted_guided_review(&self) -> Option<GuidedReview> {
        let project_root = self.config.guided_review_project_root.clone()?;
        let exclusions = self.review_exclusions();
        let restored = tokio::task::spawn_blocking(move || {
            let store = ActiveReviewRecordStore::new(&project_root);
            let record = store.load().map_err(|error| error.to_string())?;
            record
                .map(|record| {
                    record.restore(&project_root, &exclusions).map_err(|error| error.to_string())
                })
                .transpose()
        })
        .await;
        match restored {
            Ok(Ok(review)) => review,
            Ok(Err(error)) => {
                warn!(%error, "could not restore persisted guided review");
                None
            }
            Err(error) => {
                warn!(%error, "persisted guided review restore task failed");
                None
            }
        }
    }

    /// Checks whether a complete persisted pre-turn snapshot can serve a restarted engine.
    pub(crate) async fn has_persisted_turn_baseline(
        &self,
        session_id: &str,
        turn_id: &str,
    ) -> bool {
        let Some(project_root) = self.config.guided_review_project_root.clone() else {
            return false;
        };
        let session_id = session_id.to_owned();
        let turn_id = turn_id.to_owned();
        matches!(
            tokio::task::spawn_blocking(move || {
                let baseline_id = TurnBaselineId::from_session_turn(&session_id, &turn_id);
                TurnBaselineArtifact::load(&project_root, &baseline_id).is_ok()
            })
            .await,
            Ok(true)
        )
    }

    pub(crate) async fn capture_pre_turn_baseline(
        &self,
        turn_id: &AgentTurnId,
    ) -> Result<Option<RetainedTurnBaseline>, GuidedReviewError> {
        let Some(project_root) = self.config.guided_review_project_root.clone() else {
            return Ok(None);
        };
        let session_id = self.current_session_id().await.to_string();
        let artifact = self
            .review_git(project_root.clone())
            .capture_and_persist_turn_baseline(
                &session_id,
                turn_id.as_str(),
                UntrackedSourcePolicy::default(),
            )
            .await?;
        debug!(
            turn_id = %turn_id.as_str(),
            baseline_id = %artifact.manifest.id.as_str(),
            "captured pre-turn review baseline"
        );
        Ok(Some(RetainedTurnBaseline { session_id, turn_id: turn_id.clone() }))
    }

    pub(crate) async fn capture_post_turn_baseline(
        &self,
        baseline: &RetainedTurnBaseline,
    ) -> Result<(), GuidedReviewError> {
        let Some(project_root) = self.config.guided_review_project_root.clone() else {
            return Ok(());
        };
        let artifact = self
            .review_git(project_root.clone())
            .capture_and_persist_post_turn_snapshot(
                &baseline.session_id,
                baseline.turn_id.as_str(),
                UntrackedSourcePolicy::default(),
            )
            .await?;
        debug!(
            turn_id = %baseline.turn_id.as_str(),
            snapshot_id = %artifact.manifest.id.as_str(),
            "captured post-turn review snapshot"
        );
        let record = LastTurnRecord::new(&baseline.session_id, baseline.turn_id.as_str())
            .map_err(|error| GuidedReviewError::LastTurnRecord(error.to_string()))?;
        tokio::task::spawn_blocking(move || {
            LastTurnRecordStore::new(&project_root).save(&record)?;
            let keep = [record.baseline_id(), record.after_snapshot_id()];
            prune_turn_baselines(&project_root, &keep, UNREFERENCED_BASELINE_AGE);
            Ok::<_, LastTurnRecordStoreError>(())
        })
        .await
        .map_err(|_| GuidedReviewError::LastTurnRecordTask)?
        .map_err(|error| GuidedReviewError::LastTurnRecord(error.to_string()))?;
        Ok(())
    }
}
