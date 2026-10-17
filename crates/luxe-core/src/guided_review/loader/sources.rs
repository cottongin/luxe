//! LocalGit — untracked classification and working-tree source assembly.

use super::*;
use std::ffi::OsStr;

impl LocalGit {
    /// Classifies one eligible untracked file without reading past the configured byte limit.
    pub async fn classify_untracked_file(
        &self,
        relative_path: &std::path::Path,
        policy: UntrackedSourcePolicy,
    ) -> Result<UntrackedFileKind, GuidedReviewError> {
        let contents = self.read_eligible_untracked_file(relative_path, policy).await?;
        if contents.contains(&0) || std::str::from_utf8(&contents).is_err() {
            Ok(UntrackedFileKind::Binary)
        } else {
            Ok(UntrackedFileKind::Text)
        }
    }

    /// Captures a deterministic `/dev/null` text addition for an eligible untracked file.
    pub async fn capture_untracked_text_addition(
        &self,
        relative_path: &std::path::Path,
        policy: UntrackedSourcePolicy,
    ) -> Result<String, GuidedReviewError> {
        let contents = self.read_eligible_untracked_file(relative_path, policy).await?;
        let text = std::str::from_utf8(&contents)
            .ok()
            .filter(|_| !contents.contains(&0))
            .ok_or_else(|| ReviewSourceUnavailable::ExcludedUntracked {
                path: relative_path.to_owned(),
                constraint: "binary content requires a non-text source item".into(),
            })?;
        Ok(render_text_addition(relative_path, text))
    }

    /// Captures an explicit non-text `/dev/null` addition for an eligible binary file.
    pub async fn capture_untracked_binary_addition(
        &self,
        relative_path: &std::path::Path,
        policy: UntrackedSourcePolicy,
    ) -> Result<String, GuidedReviewError> {
        let contents = self.read_eligible_untracked_file(relative_path, policy).await?;
        if !contents.contains(&0) && std::str::from_utf8(&contents).is_ok() {
            return Err(ReviewSourceUnavailable::ExcludedUntracked {
                path: relative_path.to_owned(),
                constraint: "text content requires a text source item".into(),
            }
            .into());
        }
        Ok(render_binary_addition(relative_path, &contents))
    }

    pub(super) async fn assemble_head_working_tree_source(
        &self,
        base_oid: &str,
        policy: UntrackedSourcePolicy,
    ) -> Result<AssembledSource, GuidedReviewError> {
        let (tracked, known_tracked_paths) =
            self.capture_index_independent_tracked_diff(base_oid).await?;
        self.assemble_working_tree_source_with_known_paths(tracked, known_tracked_paths, policy)
            .await
    }

    pub(super) async fn assemble_working_tree_source(
        &self,
        tracked: String,
        policy: UntrackedSourcePolicy,
    ) -> Result<AssembledSource, GuidedReviewError> {
        let tracked_files = parse_unified_diff(&tracked)?;
        let known_tracked_paths = tracked_files
            .files
            .iter()
            .flat_map(|file| [file.old_path.as_ref(), file.new_path.as_ref()])
            .flatten()
            .cloned()
            .collect();
        self.assemble_working_tree_source_with_known_paths(tracked, known_tracked_paths, policy)
            .await
    }

    /// The tracked diff plus every eligible untracked file as an addition.
    ///
    /// An untracked file the policy will not hold (over the size limit, or not a regular file) is
    /// left out and named in [`AssembledSource::uncovered`], so one stray build artifact costs the
    /// review that file rather than the whole review.
    pub(super) async fn assemble_working_tree_source_with_known_paths(
        &self,
        tracked: String,
        known_tracked_paths: HashSet<PathBuf>,
        policy: UntrackedSourcePolicy,
    ) -> Result<AssembledSource, GuidedReviewError> {
        let mut untracked_paths = self.eligible_untracked_paths().await?;
        untracked_paths.sort();
        let mut assembled = AssembledSource { text: tracked, ..AssembledSource::default() };
        for path in untracked_paths {
            if known_tracked_paths.contains(&path) {
                continue;
            }
            let addition = match self.untracked_addition(&path, policy).await {
                Err(GuidedReviewError::Unavailable(
                    ReviewSourceUnavailable::ExcludedUntracked { .. },
                )) => {
                    assembled.uncovered.push(path);
                    continue;
                }
                other => other?,
            };
            if !assembled.text.is_empty() && !assembled.text.ends_with('\n') {
                assembled.text.push('\n');
            }
            assembled.text.push_str(&addition);
            assembled.included_untracked_files += 1;
        }
        Ok(assembled)
    }

    /// One untracked file as an addition, text or binary as its contents are, from one read.
    async fn untracked_addition(
        &self,
        path: &std::path::Path,
        policy: UntrackedSourcePolicy,
    ) -> Result<String, GuidedReviewError> {
        let contents = self.read_eligible_untracked_file(path, policy).await?;
        match std::str::from_utf8(&contents).ok().filter(|_| !contents.contains(&0)) {
            Some(text) => Ok(render_text_addition(path, text)),
            None => Ok(render_binary_addition(path, &contents)),
        }
    }

    /// Captures a final-state tracked diff without consulting or changing the user's index.
    pub(super) async fn capture_index_independent_tracked_diff(
        &self,
        base_oid: &str,
    ) -> Result<(String, HashSet<PathBuf>), GuidedReviewError> {
        if !is_git_oid(base_oid) {
            return Err(GuidedReviewError::InvalidReference { reference: base_oid.into() });
        }

        let base_paths = parse_untracked_paths(
            &self.run("list base tree paths", &["ls-tree", "-rz", "--name-only", base_oid]).await?,
        )?;
        let current_paths = self.tracked_worktree_paths().await?;
        let known_tracked_paths =
            base_paths.into_iter().chain(current_paths.iter().cloned()).collect::<HashSet<_>>();
        let root = std::env::temp_dir().join(format!("luxe-guided-review-{}", Uuid::new_v4()));
        fs::create_dir_all(&root)
            .map_err(|source| GuidedReviewError::ReadPath { path: root.clone(), error: source })?;
        let result = async {
            let base_root = root.join("base");
            let worktree_root = root.join("worktree");
            let base_index = root.join("base.index");
            let worktree_index = root.join("worktree.index");
            self.populate_private_index_snapshot(base_oid, &base_index, &base_root).await?;
            self.populate_private_index_snapshot(base_oid, &worktree_index, &worktree_root).await?;
            self.update_private_index_to_worktree(&worktree_index, &current_paths).await?;
            fs::remove_dir_all(&worktree_root).map_err(|source| GuidedReviewError::ReadPath {
                path: worktree_root.clone(),
                error: source,
            })?;
            self.checkout_private_index(&worktree_index, &worktree_root).await?;
            self.diff_snapshot_roots(&root, "base", "worktree").await
        }
        .await;
        let _ = fs::remove_dir_all(&root);
        result.map(|source| (source, known_tracked_paths))
    }

    pub(super) async fn populate_private_index_snapshot(
        &self,
        base_oid: &str,
        index_path: &std::path::Path,
        snapshot_root: &std::path::Path,
    ) -> Result<(), GuidedReviewError> {
        self.run_with_private_index(
            "prepare private working tree index",
            index_path,
            ["read-tree", base_oid],
        )
        .await?;
        self.checkout_private_index(index_path, snapshot_root).await
    }

    pub(super) async fn update_private_index_to_worktree(
        &self,
        index_path: &std::path::Path,
        current_paths: &[PathBuf],
    ) -> Result<(), GuidedReviewError> {
        self.run_with_private_index("update private working tree index", index_path, ["add", "-u"])
            .await?;
        let present_paths = current_paths
            .iter()
            .filter(|path| fs::symlink_metadata(self.repo_path.join(path)).is_ok())
            .collect::<Vec<_>>();
        if present_paths.is_empty() {
            return Ok(());
        }
        let arguments = [OsStr::new("add"), OsStr::new("--")]
            .into_iter()
            .chain(present_paths.iter().map(|path| path.as_os_str()))
            .collect::<Vec<_>>();
        let env = [("GIT_INDEX_FILE", index_path.as_os_str())];
        let output = self.git_output("update private working tree index", &arguments, &env).await?;
        if !output.status.success() {
            return Err(git_failed(
                "update private working tree index",
                output.status.code(),
                &output.stderr,
            ));
        }
        Ok(())
    }

    pub(super) async fn checkout_private_index(
        &self,
        index_path: &std::path::Path,
        snapshot_root: &std::path::Path,
    ) -> Result<(), GuidedReviewError> {
        let prefix = format!("--prefix={}/", snapshot_root.display());
        let arguments = command::os_args(&["checkout-index", "--all", &prefix]);
        let env = [("GIT_INDEX_FILE", index_path.as_os_str())];
        let operation = "materialize private working tree index";
        let output = self.git_output(operation, &arguments, &env).await?;
        if !output.status.success() {
            return Err(git_failed(
                "materialize private working tree index",
                output.status.code(),
                &output.stderr,
            ));
        }
        Ok(())
    }

    pub(super) async fn run_with_private_index<const N: usize>(
        &self,
        operation: &'static str,
        index_path: &std::path::Path,
        arguments: [&str; N],
    ) -> Result<Vec<u8>, GuidedReviewError> {
        let env = [("GIT_INDEX_FILE", index_path.as_os_str())];
        let output = self.git_output(operation, &command::os_args(&arguments), &env).await?;
        if output.status.success() {
            Ok(output.stdout)
        } else {
            Err(git_failed(operation, output.status.code(), &output.stderr))
        }
    }

    pub(super) async fn diff_snapshot_roots(
        &self,
        root: &std::path::Path,
        before_root: &str,
        after_root: &str,
    ) -> Result<String, GuidedReviewError> {
        let arguments = Self::pinned(&[
            "diff",
            "--no-index",
            "--binary",
            "--find-renames",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            before_root,
            after_root,
        ]);
        let operation = "capture working tree diff";
        let output = command::run_git_output(
            root,
            operation,
            &command::os_args(&arguments),
            &[],
            self.git_deadline,
        )
        .await?;
        if !output.status.success() && output.status.code() != Some(1) {
            return Err(git_failed(
                "capture working tree diff",
                output.status.code(),
                &output.stderr,
            ));
        }
        let source = String::from_utf8(output.stdout).map_err(|_| {
            GuidedReviewError::MalformedGitOutput {
                operation: "capture working tree diff",
                reason: "canonical diff is not valid UTF-8",
            }
        })?;
        normalize_no_index_snapshot_paths(&source, before_root, after_root)
    }

    pub(super) async fn read_eligible_untracked_file(
        &self,
        relative_path: &std::path::Path,
        policy: UntrackedSourcePolicy,
    ) -> Result<Vec<u8>, GuidedReviewError> {
        if !is_safe_relative_path(relative_path) {
            return Err(GuidedReviewError::MalformedGitOutput {
                operation: "classify untracked path",
                reason: "path is not a safe repository-relative path",
            });
        }
        let path = self.repo_path.join(relative_path);
        let metadata = tokio::fs::symlink_metadata(&path)
            .await
            .map_err(|source| GuidedReviewError::ReadPath { path: path.clone(), error: source })?;
        if !metadata.file_type().is_file() {
            return Err(ReviewSourceUnavailable::ExcludedUntracked {
                path: relative_path.to_owned(),
                constraint: "not a regular file".into(),
            }
            .into());
        }
        if metadata.len() > policy.max_file_bytes {
            return Err(ReviewSourceUnavailable::ExcludedUntracked {
                path: relative_path.to_owned(),
                constraint: format!("exceeds {} byte source limit", policy.max_file_bytes),
            }
            .into());
        }

        let file = tokio::fs::File::open(&path)
            .await
            .map_err(|source| GuidedReviewError::ReadPath { path: path.clone(), error: source })?;
        let mut contents = Vec::new();
        file.take(policy.max_file_bytes.saturating_add(1))
            .read_to_end(&mut contents)
            .await
            .map_err(|source| GuidedReviewError::ReadPath { path: path.clone(), error: source })?;
        if contents.len() as u64 > policy.max_file_bytes {
            return Err(ReviewSourceUnavailable::ExcludedUntracked {
                path: relative_path.to_owned(),
                constraint: format!("exceeds {} byte source limit", policy.max_file_bytes),
            }
            .into());
        }
        Ok(contents)
    }
}
