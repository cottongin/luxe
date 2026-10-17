//! Persistence — writing a captured source as an immutable artifact.

use super::*;

impl LocalGit {
    pub(super) async fn persist_working_tree_capture(
        &self,
        base: ReviewBase,
        provenance: WorktreeProvenance,
        canonical_source: String,
        included_untracked_files: u64,
    ) -> Result<WorkingTreeCapture, GuidedReviewError> {
        let artifact = ReviewSourceArtifact::capture(canonical_source.into_bytes());
        self.persist_source_artifact(&artifact).await?;
        let captured_at_ms = artifact.manifest.captured_at_ms.unwrap_or_default();
        Ok(WorkingTreeCapture {
            source: ReviewSource {
                base,
                content_digest: artifact.manifest.revision_id.clone(),
                includes_staged: provenance.includes_staged,
                includes_unstaged: provenance.includes_unstaged,
                included_untracked_files,
                captured_at_ms,
                uncovered_paths: Vec::new(),
            },
            artifact,
        })
    }

    pub(super) async fn persist_source_artifact(
        &self,
        artifact: &ReviewSourceArtifact,
    ) -> Result<(), GuidedReviewError> {
        let artifact_to_persist = artifact.clone();
        let project_root = self.repo_path.clone();
        let retention = self.source_retention.clone();
        let mut protected_revisions = self.protected_revisions.clone();
        protected_revisions.insert(artifact.manifest.revision_id.clone());
        tokio::task::spawn_blocking(move || {
            artifact_to_persist.persist(&project_root)?;
            ReviewSourceArtifact::prune_expired(
                &project_root,
                &retention,
                capture_time_ms(),
                &protected_revisions,
            )?;
            ReviewSourceArtifact::prune_to_byte_budget(
                &project_root,
                &retention,
                &protected_revisions,
            )?;
            Ok::<(), SourceStoreError>(())
        })
        .await
        .map_err(|_| GuidedReviewError::SourcePersistenceTask)??;
        Ok(())
    }
}
