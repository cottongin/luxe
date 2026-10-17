//! LocalGit — turn-baseline snapshot capture and diffing.

use super::*;
use crate::guided_review::TurnBaselineExclusion;

impl LocalGit {
    /// Captures the current workspace into an immutable snapshot without changing Git state, leaving
    /// out what it cannot hold.
    pub(super) async fn capture_turn_snapshot(
        &self,
        id: TurnBaselineId,
        policy: UntrackedSourcePolicy,
    ) -> Result<TurnSnapshot, GuidedReviewError> {
        let base = self.resolve_worktree_base().await?;
        let provenance = self.worktree_provenance().await?;
        let mut paths = self.tracked_worktree_paths().await?;
        let mut untracked_paths = self.eligible_untracked_paths().await?;
        untracked_paths.retain(|path| !is_turn_baseline_cache_path(path));
        let untracked: std::collections::HashSet<PathBuf> =
            untracked_paths.iter().cloned().collect();
        paths.extend(untracked_paths);
        paths.retain(|path| !is_turn_baseline_cache_path(path));
        paths.sort();
        if paths.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(GuidedReviewError::MalformedGitOutput {
                operation: "capture turn baseline",
                reason: "tracked and untracked paths overlap",
            });
        }

        let mut files = Vec::with_capacity(paths.len());
        let mut excluded = Vec::new();
        for path in paths {
            match self.read_eligible_untracked_file(&path, policy).await {
                Ok(contents) => files.push((path, contents)),
                // still in the index, gone from the tree: absent, which is what a deletion is
                Err(GuidedReviewError::ReadPath { error: source, .. })
                    if source.kind() == io::ErrorKind::NotFound => {}
                // one file the snapshot cannot hold must not cost the review every other file
                Err(GuidedReviewError::ReadPath { .. } | GuidedReviewError::Unavailable(_)) => {
                    excluded.push(self.exclusion(path).await);
                }
                Err(other) => return Err(other),
            }
        }
        let included_untracked_files =
            files.iter().filter(|(path, _)| untracked.contains(path)).count() as u64;
        Ok(TurnSnapshot {
            artifact: TurnBaselineArtifact::capture(
                id,
                base,
                capture_time_ms(),
                provenance.includes_staged,
                provenance.includes_unstaged,
                included_untracked_files,
                files,
            )?
            .excluding(excluded),
        })
    }

    /// Compares two complete snapshots without reading or changing the live worktree.
    pub async fn diff_turn_snapshots(
        &self,
        before: &TurnBaselineArtifact,
        after: &TurnBaselineArtifact,
    ) -> Result<String, GuidedReviewError> {
        let root = std::env::temp_dir().join(format!("luxe-guided-review-{}", Uuid::new_v4()));
        let result = async {
            let before_root = root.join("before");
            let after_root = root.join("after");
            let skip = excluded_paths(before, after, |_| true);
            before.materialize_except(&before_root, &skip)?;
            after.materialize_except(&after_root, &skip)?;
            let arguments = LocalGit::pinned(&[
                "diff",
                "--no-index",
                "--binary",
                "--src-prefix=a/",
                "--dst-prefix=b/",
                "before",
                "after",
            ]);
            let operation = "diff retained turn snapshots";
            let arguments = command::os_args(&arguments);
            let output =
                command::run_git_output(&root, operation, &arguments, &[], self.git_deadline)
                    .await?;
            if !output.status.success() && output.status.code() != Some(1) {
                return Err(git_failed(
                    "diff retained turn snapshots",
                    output.status.code(),
                    &output.stderr,
                ));
            }
            let source = String::from_utf8(output.stdout).map_err(|_| {
                GuidedReviewError::MalformedGitOutput {
                    operation: "diff retained turn snapshots",
                    reason: "Git emitted non-UTF-8 unified diff output",
                }
            })?;
            normalize_no_index_snapshot_paths(&source, "before", "after")
        }
        .await;
        let _ = fs::remove_dir_all(&root);
        result
    }

    /// Captures and retains the complete pre-turn workspace without changing Git state.
    pub async fn capture_and_persist_turn_baseline(
        &self,
        session_id: &str,
        turn_id: &str,
        policy: UntrackedSourcePolicy,
    ) -> Result<TurnBaselineArtifact, GuidedReviewError> {
        let snapshot = self
            .capture_turn_snapshot(TurnBaselineId::from_session_turn(session_id, turn_id), policy)
            .await?;
        let artifact = snapshot.artifact;
        let to_persist = artifact.clone();
        let project_root = self.repo_path.clone();
        tokio::task::spawn_blocking(move || to_persist.persist(&project_root))
            .await
            .map_err(|_| GuidedReviewError::TurnBaselinePersistenceTask)??;
        Ok(artifact)
    }

    /// Captures and retains the complete workspace immediately after a turn without changing Git state.
    pub async fn capture_and_persist_post_turn_snapshot(
        &self,
        session_id: &str,
        turn_id: &str,
        policy: UntrackedSourcePolicy,
    ) -> Result<TurnBaselineArtifact, GuidedReviewError> {
        let snapshot = self
            .capture_turn_snapshot(
                TurnBaselineId::post_turn_from_session_turn(session_id, turn_id),
                policy,
            )
            .await?;
        let artifact = snapshot.artifact;
        let to_persist = artifact.clone();
        let project_root = self.repo_path.clone();
        tokio::task::spawn_blocking(move || to_persist.persist(&project_root))
            .await
            .map_err(|_| GuidedReviewError::TurnBaselinePersistenceTask)??;
        Ok(artifact)
    }
}

/// The paths either snapshot left out that `keep` accepts, sorted and once each.
fn excluded_paths(
    before: &TurnBaselineArtifact,
    after: &TurnBaselineArtifact,
    keep: impl Fn(&TurnBaselineExclusion) -> bool,
) -> Vec<PathBuf> {
    let excluded = before.manifest.excluded.iter().chain(&after.manifest.excluded);
    let mut paths: Vec<PathBuf> = excluded
        .filter(|exclusion| keep(exclusion))
        .map(|exclusion| exclusion.path.clone())
        .collect();
    paths.sort();
    paths.dedup();
    paths
}

/// What a comparison of the two snapshots cannot show: every file either left out, except one
/// both left out untouched, which the turn did not change.
pub(super) fn uncovered_paths(
    before: &TurnBaselineArtifact,
    after: &TurnBaselineArtifact,
) -> Vec<PathBuf> {
    let untouched = |exclusion: &TurnBaselineExclusion| {
        let sides = [&before.manifest.excluded, &after.manifest.excluded];
        sides.iter().all(|side| side.iter().any(|other| exclusion.unchanged_in(other)))
    };
    excluded_paths(before, after, |exclusion| !untouched(exclusion))
}

impl LocalGit {
    /// `path` as a snapshot records leaving it out: with its size and age, when they can be read.
    async fn exclusion(&self, path: PathBuf) -> TurnBaselineExclusion {
        let metadata = tokio::fs::symlink_metadata(self.repo_path.join(&path)).await.ok();
        let modified_ms = metadata
            .as_ref()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .and_then(|age| u64::try_from(age.as_millis()).ok());
        TurnBaselineExclusion {
            path,
            byte_len: metadata.map(|metadata| metadata.len()),
            modified_ms,
        }
    }
}
