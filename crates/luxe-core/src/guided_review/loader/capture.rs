//! Capture — reading a local target's diff: a turn, the working tree, a base, a commit, held stable.

use super::*;

impl LocalGit {
    /// Pins a complete comparison between retained pre-turn and post-turn snapshots.
    pub async fn capture_and_persist_last_turn_source(
        &self,
        session_id: &str,
        turn_id: &str,
        _policy: UntrackedSourcePolicy,
    ) -> Result<WorkingTreeCapture, GuidedReviewError> {
        let before_id = TurnBaselineId::from_session_turn(session_id, turn_id);
        let after_id = TurnBaselineId::post_turn_from_session_turn(session_id, turn_id);
        let project_root = self.repo_path.clone();
        let (before, after) = tokio::task::spawn_blocking(move || {
            Ok::<_, TurnBaselineStoreError>((
                TurnBaselineArtifact::load(&project_root, &before_id)?,
                TurnBaselineArtifact::load(&project_root, &after_id)?,
            ))
        })
        .await
        .map_err(|_| GuidedReviewError::TurnBaselineLoadTask)?
        .map_err(|error| match error {
            TurnBaselineStoreError::Io { error: source, .. }
                if source.kind() == io::ErrorKind::NotFound =>
            {
                GuidedReviewError::Unavailable(ReviewSourceUnavailable::MissingTurnBaseline)
            }
            error => error.into(),
        })?;
        let baseline_revision = before.revision_id();
        let canonical_source = self.diff_turn_snapshots(&before, &after).await?;
        parse_unified_diff(&canonical_source)?;
        let mut capture = self
            .persist_working_tree_capture(
                ReviewBase::TurnBaseline { revision_id: baseline_revision },
                WorktreeProvenance {
                    includes_staged: after.manifest.includes_staged,
                    includes_unstaged: after.manifest.includes_unstaged,
                },
                canonical_source,
                after.manifest.included_untracked_files,
            )
            .await?;
        capture.source.uncovered_paths = super::turn::uncovered_paths(&before, &after);
        Ok(capture)
    }

    /// Captures and pins the current working tree using `HEAD` or the empty tree when unborn.
    pub async fn capture_and_persist_working_tree_source(
        &self,
        policy: UntrackedSourcePolicy,
    ) -> Result<WorkingTreeCapture, GuidedReviewError> {
        let base = self.resolve_worktree_base().await?;
        let captured = self.capture_stable_worktree_source(&base, policy).await?;
        self.persist_captured_worktree(base, captured).await
    }

    /// Captures and pins a resolved Base comparison through the current working tree.
    pub async fn capture_and_persist_base_working_tree_source(
        &self,
        reference: &str,
        policy: UntrackedSourcePolicy,
    ) -> Result<WorkingTreeCapture, GuidedReviewError> {
        let base = self.resolve_base_merge_base(reference).await?;
        let ReviewBase::MergeBase { .. } = &base else {
            unreachable!("base resolution only produces merge-base provenance")
        };
        let captured = self.capture_stable_worktree_source(&base, policy).await?;
        self.persist_captured_worktree(base, captured).await
    }

    /// Captures and pins a selected commit against its first parent or the empty tree for roots.
    pub async fn capture_and_persist_commit_source(
        &self,
        oid: &str,
    ) -> Result<WorkingTreeCapture, GuidedReviewError> {
        let base = self.resolve_commit_first_parent(oid).await?;
        let canonical_source =
            String::from_utf8(self.capture_commit_diff(&base).await?).map_err(|_| {
                GuidedReviewError::MalformedGitOutput {
                    operation: "capture commit diff",
                    reason: "canonical diff is not valid UTF-8",
                }
            })?;
        parse_unified_diff(&canonical_source)?;
        self.persist_working_tree_capture(
            base,
            WorktreeProvenance { includes_staged: false, includes_unstaged: false },
            canonical_source,
            0,
        )
        .await
    }

    /// Captures and pins one complete `HEAD`-to-worktree source before returning it.
    pub async fn capture_and_persist_head_working_tree_source(
        &self,
        base_oid: &str,
        policy: UntrackedSourcePolicy,
    ) -> Result<WorkingTreeCapture, GuidedReviewError> {
        let base = ReviewBase::Head { oid: base_oid.into() };
        let captured = self.capture_stable_worktree_source(&base, policy).await?;
        self.persist_captured_worktree(base, captured).await
    }

    /// Pins a stable worktree capture, naming the untracked files it left out.
    async fn persist_captured_worktree(
        &self,
        base: ReviewBase,
        captured: CapturedWorktreeSource,
    ) -> Result<WorkingTreeCapture, GuidedReviewError> {
        let mut capture = self
            .persist_working_tree_capture(
                base,
                captured.provenance,
                captured.canonical_source,
                captured.included_untracked_files,
            )
            .await?;
        capture.source.uncovered_paths = captured.uncovered_paths;
        Ok(capture)
    }

    pub(super) async fn capture_stable_worktree_source(
        &self,
        base: &ReviewBase,
        policy: UntrackedSourcePolicy,
    ) -> Result<CapturedWorktreeSource, GuidedReviewError> {
        let first = self.capture_worktree_source_once(base, policy).await?;
        let second = self.capture_worktree_source_once(base, policy).await?;
        ensure_stable_worktree_capture(first, second)
    }

    pub(super) async fn capture_worktree_source_once(
        &self,
        base: &ReviewBase,
        policy: UntrackedSourcePolicy,
    ) -> Result<CapturedWorktreeSource, GuidedReviewError> {
        let provenance = self.worktree_provenance().await?;
        let assembled = match base {
            ReviewBase::Head { oid } | ReviewBase::MergeBase { oid, .. } => {
                self.assemble_head_working_tree_source(oid, policy).await?
            }
            ReviewBase::EmptyTree => {
                self.assemble_working_tree_source(String::new(), policy).await?
            }
            _ => unreachable!("worktree capture requires a resolved tree base"),
        };
        Ok(CapturedWorktreeSource {
            provenance,
            canonical_source: assembled.text,
            included_untracked_files: assembled.included_untracked_files,
            uncovered_paths: assembled.uncovered,
        })
    }

    /// Captures the exact first-parent commit diff without consulting the mutable worktree.
    pub(super) async fn capture_commit_diff(
        &self,
        base: &ReviewBase,
    ) -> Result<Vec<u8>, GuidedReviewError> {
        let ReviewBase::CommitParent { commit_oid, parent_oid } = base else {
            unreachable!("commit capture requires commit-parent provenance")
        };
        match parent_oid {
            Some(parent_oid) => {
                self.run(
                    "capture commit diff",
                    &Self::pinned(&[
                        "diff",
                        "--binary",
                        "--find-renames",
                        "--src-prefix=a/",
                        "--dst-prefix=b/",
                        parent_oid,
                        commit_oid,
                        "--",
                    ]),
                )
                .await
            }
            None => {
                self.run(
                    "capture root commit diff",
                    &Self::pinned(&[
                        "diff-tree",
                        "--binary",
                        "--find-renames",
                        "--root",
                        "--no-commit-id",
                        "--src-prefix=a/",
                        "--dst-prefix=b/",
                        commit_oid,
                    ]),
                )
                .await
            }
        }
    }

    /// Captures one final-state tracked diff from a resolved `HEAD` object.
    pub async fn capture_tracked_diff(&self, base_oid: &str) -> Result<Vec<u8>, GuidedReviewError> {
        if !is_git_oid(base_oid) {
            return Err(GuidedReviewError::InvalidReference { reference: base_oid.into() });
        }
        self.run(
            "capture working tree diff",
            &Self::pinned(&[
                "diff",
                "--binary",
                "--find-renames",
                "--src-prefix=a/",
                "--dst-prefix=b/",
                base_oid,
                "--",
            ]),
        )
        .await
    }

    pub(super) async fn has_diff(
        &self,
        operation: &'static str,
        arguments: &[&str],
    ) -> Result<bool, GuidedReviewError> {
        let output = self.git_output(operation, &command::os_args(arguments), &[]).await?;
        match output.status.code() {
            Some(0) => Ok(false),
            Some(1) => Ok(true),
            exit_code => Err(git_failed(operation, exit_code, &output.stderr)),
        }
    }
}
