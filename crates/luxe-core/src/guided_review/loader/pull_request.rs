//! Pull request — pinning a pull request's commits locally, fetching them when they are missing.

use super::*;

/// How long fetching a pull request's two commits may take before it is reaped.
const PULL_REQUEST_FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// The first few lines of git's own error text, for a failure the reader has to act on.
fn bounded_detail(stderr: &str) -> String {
    let lines: Vec<&str> = stderr.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
    match lines.as_slice() {
        [] => "git gave no reason".into(),
        lines => lines.iter().take(3).copied().collect::<Vec<_>>().join(" "),
    }
}

impl LocalGit {
    /// Captures and pins a pull request from where it branched off its base to its head.
    ///
    /// Never touches the working tree, the index or a ref. The commits are fetched by SHA only when
    /// they are not already here, so a second look at the same revision works offline, and
    /// `--no-write-fetch-head` keeps even `FETCH_HEAD` as it was. The comparison is tree to tree from
    /// the merge base, which is what GitHub's "Files changed" shows.
    pub async fn capture_and_persist_pull_request_source(
        &self,
        number: u64,
    ) -> Result<WorkingTreeCapture, GuidedReviewError> {
        let pin = self
            .pull_request
            .as_ref()
            .filter(|pin| pin.number == number)
            .ok_or(ReviewSourceUnavailable::PullRequestNotResolved { number })?;
        for oid in [&pin.base_oid, &pin.head_oid] {
            if !is_git_oid(oid) {
                return Err(GuidedReviewError::InvalidReference { reference: oid.clone() });
            }
        }
        if !(self.has_commit(&pin.base_oid).await && self.has_commit(&pin.head_oid).await) {
            self.fetch_pull_request(pin).await?;
        }
        let merge_base = self
            .run("find pull request merge base", &["merge-base", &pin.base_oid, &pin.head_oid])
            .await
            .ok()
            .and_then(|output| parse_resolved_oid(&output))
            .ok_or(ReviewSourceUnavailable::PullRequestNoMergeBase { number })?;
        let diff = self
            .run(
                "capture pull request diff",
                &Self::pinned(&[
                    "diff",
                    "--binary",
                    "--find-renames",
                    "--src-prefix=a/",
                    "--dst-prefix=b/",
                    &merge_base,
                    &pin.head_oid,
                    "--",
                ]),
            )
            .await?;
        let canonical_source =
            String::from_utf8(diff).map_err(|_| GuidedReviewError::MalformedGitOutput {
                operation: "capture pull request diff",
                reason: "canonical diff is not valid UTF-8",
            })?;
        parse_unified_diff(&canonical_source)?;
        self.persist_working_tree_capture(
            ReviewBase::PullRequest {
                number,
                repository: pin.repository.clone(),
                base_ref: pin.base_ref.clone(),
                head_ref: pin.head_ref.clone(),
                merge_base,
                head_oid: pin.head_oid.clone(),
            },
            WorktreeProvenance { includes_staged: false, includes_unstaged: false },
            canonical_source,
            0,
        )
        .await
    }

    /// Captures the checked-out branch's own pull request: from where it branched off its base to
    /// the working tree, so a fix made here is part of what is reviewed.
    ///
    /// The base commit is fetched by SHA when it is not already here, exactly as for someone
    /// else's pull request, and nothing in the checkout moves. The base is recorded as a plain
    /// merge base, so everything a working-tree review does — refresh, fixes, verification — works
    /// the same here.
    pub async fn capture_and_persist_own_pull_request_source(
        &self,
        number: u64,
        policy: UntrackedSourcePolicy,
    ) -> Result<WorkingTreeCapture, GuidedReviewError> {
        let pin = self
            .pull_request
            .as_ref()
            .filter(|pin| pin.number == number)
            .ok_or(ReviewSourceUnavailable::PullRequestNotResolved { number })?;
        for oid in [&pin.base_oid, &pin.head_oid] {
            if !is_git_oid(oid) {
                return Err(GuidedReviewError::InvalidReference { reference: oid.clone() });
            }
        }
        if !self.has_commit(&pin.base_oid).await {
            self.fetch_pull_request(pin).await?;
        }
        let merge_base = self
            .run("find pull request merge base", &["merge-base", &pin.base_oid, "HEAD"])
            .await
            .ok()
            .and_then(|output| parse_resolved_oid(&output))
            .ok_or(ReviewSourceUnavailable::PullRequestNoMergeBase { number })?;
        let base = ReviewBase::MergeBase { reference: pin.base_ref.clone(), oid: merge_base };
        let captured = self.capture_stable_worktree_source(&base, policy).await?;
        self.persist_working_tree_capture(
            base,
            captured.provenance,
            captured.canonical_source,
            captured.included_untracked_files,
        )
        .await
    }

    /// Whether `oid` names a commit already in this repository.
    async fn has_commit(&self, oid: &str) -> bool {
        self.run("look for commit", &["cat-file", "-e", &format!("{oid}^{{commit}}")]).await.is_ok()
    }

    /// Fetches a pull request's base and head commits by SHA, and nothing else.
    ///
    /// Bounded and non-interactive, because a fetch is the one step here that waits on the network
    /// and on credentials: a prompt for a password would sit invisibly behind the TUI, and Git
    /// Credential Manager on Windows would open a window.
    async fn fetch_pull_request(&self, pin: &PullRequestPin) -> Result<(), GuidedReviewError> {
        let git = crate::program::resolve("git").unwrap_or_else(|| PathBuf::from("git"));
        let mut command = crate::program::captured_command(
            git,
            &[
                "fetch",
                "--quiet",
                "--no-tags",
                "--no-write-fetch-head",
                "--",
                &pin.fetch_from,
                &pin.base_oid,
                &pin.head_oid,
            ],
            &self.repo_path,
        );
        command.env("GIT_TERMINAL_PROMPT", "0").env("GCM_INTERACTIVE", "never");
        let failed = |detail: String| ReviewSourceUnavailable::PullRequestFetch {
            number: pin.number,
            detail,
        };
        let output = crate::program::run_bounded(command, PULL_REQUEST_FETCH_TIMEOUT)
            .await
            .map_err(|error| failed(error.to_string()))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(failed(bounded_detail(&stderr)).into());
        }
        Ok(())
    }
}
