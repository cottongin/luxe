//! LocalGit — low-level Git command runners and base/target resolvers.

use super::*;

/// How long `git apply` gets to consume a chapter patch before it is reaped.
const STAGE_PATCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

impl LocalGit {
    /// Runs one fixed Git operation and returns its raw stdout on success.
    pub async fn run(
        &self,
        operation: &'static str,
        arguments: &[&str],
    ) -> Result<Vec<u8>, GuidedReviewError> {
        let output = self.git_output(operation, &command::os_args(arguments), &[]).await?;
        if output.status.success() {
            Ok(output.stdout)
        } else {
            Err(git_failed(operation, output.status.code(), &output.stderr))
        }
    }

    /// Runs `git arguments` with `env` set, under this runner's deadline, whatever its status.
    pub(super) async fn git_output(
        &self,
        operation: &'static str,
        arguments: &[&std::ffi::OsStr],
        env: &[(&str, &std::ffi::OsStr)],
    ) -> Result<std::process::Output, GuidedReviewError> {
        command::run_git_output(&self.repo_path, operation, arguments, env, self.git_deadline).await
    }

    /// `arguments` for a `git diff` whose output is Git's own rendering whatever the user's config
    /// says: no color, no external diff, no textconv. The flags go after the subcommand.
    pub(super) fn pinned<'a>(arguments: &[&'a str]) -> Vec<&'a str> {
        let (command, rest) = arguments.split_first().map_or(("", &[][..]), |(c, r)| (*c, r));
        let pins = crate::file_diff::OWN_RENDERING;
        [command].into_iter().chain(pins).chain(rest.iter().copied()).collect()
    }

    /// Resolves `HEAD` for a worktree source, using the empty tree for an unborn repository.
    pub async fn resolve_worktree_base(&self) -> Result<ReviewBase, GuidedReviewError> {
        let inside_worktree =
            self.run("verify worktree", &["rev-parse", "--is-inside-work-tree"]).await?;
        if String::from_utf8(inside_worktree).ok().is_none_or(|value| value.trim() != "true") {
            return Err(ReviewSourceUnavailable::NotWorkingTree.into());
        }

        let arguments = command::os_args(&["rev-parse", "--verify", "--quiet", "HEAD^{commit}"]);
        let output = self.git_output("resolve head", &arguments, &[]).await?;
        if output.status.success() {
            let oid = String::from_utf8(output.stdout)
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| is_git_oid(value))
                .ok_or_else(|| GuidedReviewError::InvalidReference { reference: "HEAD".into() })?;
            Ok(ReviewBase::Head { oid })
        } else if output.status.code() == Some(1) {
            Ok(ReviewBase::EmptyTree)
        } else {
            Err(git_failed("resolve head", output.status.code(), &output.stderr))
        }
    }

    /// Resolves a local Base target to its merge-base with the current `HEAD`.
    pub async fn resolve_base_merge_base(
        &self,
        reference: &str,
    ) -> Result<ReviewBase, GuidedReviewError> {
        if reference.is_empty() || reference.chars().any(char::is_control) {
            return Err(GuidedReviewError::InvalidReference { reference: reference.into() });
        }
        let expression = format!("{reference}^{{commit}}");
        let arguments = command::os_args(&[
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &expression,
        ]);
        let resolved = self.git_output("resolve base reference", &arguments, &[]).await?;
        let requested_oid = parse_resolved_oid(&resolved.stdout)
            .filter(|_| resolved.status.success())
            .ok_or_else(|| GuidedReviewError::InvalidReference { reference: reference.into() })?;
        let ReviewBase::Head { oid: head_oid } = self.resolve_worktree_base().await? else {
            return Err(ReviewSourceUnavailable::NoMergeBase { reference: reference.into() }.into());
        };
        let merge_base = match self
            .run("resolve base merge point", &["merge-base", &requested_oid, &head_oid])
            .await
        {
            Ok(output) => parse_resolved_oid(&output).ok_or_else(|| {
                ReviewSourceUnavailable::NoMergeBase { reference: reference.into() }
            })?,
            Err(GuidedReviewError::GitFailed { .. }) => {
                return Err(
                    ReviewSourceUnavailable::NoMergeBase { reference: reference.into() }.into()
                )
            }
            Err(error) => return Err(error),
        };
        Ok(ReviewBase::MergeBase { reference: reference.into(), oid: merge_base })
    }

    /// Resolves a local commit target and records only its first parent for comparison.
    pub async fn resolve_commit_first_parent(
        &self,
        oid: &str,
    ) -> Result<ReviewBase, GuidedReviewError> {
        if oid.is_empty() || oid.chars().any(char::is_control) {
            return Err(GuidedReviewError::InvalidReference { reference: oid.into() });
        }
        let expression = format!("{oid}^{{commit}}");
        let arguments = command::os_args(&[
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &expression,
        ]);
        let resolved = self.git_output("resolve commit target", &arguments, &[]).await?;
        let commit_oid = parse_resolved_oid(&resolved.stdout)
            .filter(|_| resolved.status.success())
            .ok_or_else(|| GuidedReviewError::InvalidReference { reference: oid.into() })?;
        let parents = self
            .run("resolve commit parents", &["rev-list", "--parents", "-n", "1", &commit_oid])
            .await?;
        let mut fields = std::str::from_utf8(&parents)
            .ok()
            .map(str::split_whitespace)
            .ok_or_else(|| GuidedReviewError::InvalidReference { reference: oid.into() })?;
        if fields.next() != Some(commit_oid.as_str()) {
            return Err(GuidedReviewError::InvalidReference { reference: oid.into() });
        }
        let parent_oid = match fields.next() {
            Some(parent) if is_git_oid(parent) => Some(parent.into()),
            Some(_) => return Err(GuidedReviewError::InvalidReference { reference: oid.into() }),
            None => None,
        };
        if !fields.all(is_git_oid) {
            return Err(GuidedReviewError::InvalidReference { reference: oid.into() });
        }
        Ok(ReviewBase::CommitParent { commit_oid, parent_oid })
    }

    /// Reports index and worktree changes without producing additional patch streams.
    pub async fn worktree_provenance(&self) -> Result<WorktreeProvenance, GuidedReviewError> {
        Ok(WorktreeProvenance {
            includes_staged: self
                .has_diff("check staged changes", &["diff", "--quiet", "--cached", "--"])
                .await?,
            includes_unstaged: self
                .has_diff("check unstaged changes", &["diff", "--quiet", "--"])
                .await?,
        })
    }

    /// Lists tracked paths using Git's NUL-safe path protocol before a pre-turn capture.
    pub async fn tracked_worktree_paths(&self) -> Result<Vec<PathBuf>, GuidedReviewError> {
        let output = self.run("list tracked paths", &["ls-files", "-z"]).await?;
        parse_untracked_paths(&output)
    }

    /// Lists untracked paths using Git's NUL-safe, ignore-aware path protocol.
    pub async fn eligible_untracked_paths(&self) -> Result<Vec<PathBuf>, GuidedReviewError> {
        let output = self
            .run("list untracked paths", &["ls-files", "--others", "--exclude-standard", "-z"])
            .await?;
        Ok(parse_untracked_paths(&output)?
            .into_iter()
            .filter(|path| !is_review_cache_path(path))
            .collect())
    }

    /// What the reviewed commits say about themselves, for a source that has commits.
    ///
    /// A `base`/`commit` review of finished work carries no turn provenance and usually no active
    /// task, so the Brief loses its intent sections and the header falls back to the word "review" —
    /// while the closest thing to authored intent that exists, the commit messages, sits unread one
    /// `git log` away. Working-tree and last-turn sources get their intent from the session instead.
    pub(crate) async fn source_intent(&self, base: &ReviewBase) -> Option<SourceIntent> {
        let range = match base {
            ReviewBase::CommitParent { commit_oid, parent_oid } => match parent_oid {
                Some(parent) => format!("{parent}..{commit_oid}"),
                None => commit_oid.clone(),
            },
            ReviewBase::MergeBase { oid, .. } => format!("{oid}..HEAD"),
            ReviewBase::PullRequest { merge_base, head_oid, .. } => {
                format!("{merge_base}..{head_oid}")
            }
            // Nothing committed distinguishes these from the session that produced them.
            ReviewBase::Head { .. } | ReviewBase::EmptyTree | ReviewBase::TurnBaseline { .. } => {
                return None
            }
        };
        let output = self
            .run(
                "read reviewed commit messages",
                &[
                    "log",
                    "--no-merges",
                    &format!("--max-count={MAX_INTENT_COMMITS}"),
                    "--format=%s%x1f%b%x1e",
                    "--end-of-options",
                    &range,
                ],
            )
            .await
            .ok()?;
        parse_source_intent(&String::from_utf8_lossy(&output))
    }
}

/// Commits described by one review source, newest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceIntent {
    /// Header-worthy name for the change: the newest subject, plus how much else rides with it.
    pub(crate) title: String,
    /// What the commits say beyond the title; empty when the title already said all of it.
    pub(crate) summary: String,
    /// How many commits this was read from — the reviewed range's, never the project's
    pub(crate) commits: usize,
}

/// Commits read for a source's intent; more than this is a merge, not a change under review.
const MAX_INTENT_COMMITS: usize = 20;

/// Parses `--format=%s%x1f%b%x1e` records into a title and a bounded summary.
fn parse_source_intent(output: &str) -> Option<SourceIntent> {
    let commits: Vec<(String, String)> = output
        .split('\u{1e}')
        .filter_map(|record| {
            let mut parts = record.trim_start_matches('\n').splitn(2, '\u{1f}');
            let subject = parts.next()?.trim().to_owned();
            let body = parts.next().unwrap_or_default().trim().to_owned();
            (!subject.is_empty()).then_some((subject, body))
        })
        .collect();
    let (newest_subject, newest_body) = commits.first()?;
    let title = match commits.len() {
        1 => newest_subject.clone(),
        count => format!("{newest_subject} (+{} more)", count - 1),
    };
    // One commit's subject is already the review's title, so listing it back is a heading with its
    // own echo underneath. Only the body says anything the title did not.
    let mut summary = String::new();
    if commits.len() > 1 {
        for (subject, _) in &commits {
            summary.push_str(&format!("- {subject}\n"));
        }
        if !newest_body.is_empty() {
            summary.push('\n');
        }
    }
    summary.push_str(newest_body);
    // A commit message can carry a pasted token; nothing here is worth showing one.
    Some(SourceIntent {
        title,
        summary: crate::redact::redact_secrets(summary.trim(), &[]),
        commits: commits.len(),
    })
}

impl LocalGit {
    /// Stages and commits one chapter at a time, refusing anything it cannot do safely.
    ///
    /// Guardrails, in order, because a review tool that rewrites your index by surprise is worse
    /// than one that does nothing:
    ///
    /// - the index must be clean, or a pre-staged change would be swept into a chapter commit;
    /// - the working tree must still match the reviewed revision, or the commits would not contain
    ///   what the reviewer read;
    /// - each chapter stages all-or-nothing (`git apply --cached`), and a failure leaves the index
    ///   as it was found.
    ///
    /// A review that does not start at `HEAD` — a base comparison past commits already made, or
    /// your own pull request — cannot stage a chapter's patch, which counts from its base and not
    /// from the index. Committed `whole` (every chapter in one commit) it stages the reviewed
    /// files as they are, which is exactly what was read once the working tree is known to match;
    /// one commit per chapter is refused, saying so.
    ///
    /// Never pushes, never amends, never touches a branch.
    pub async fn commit_review_chapters(
        &self,
        review: &GuidedReview,
        commits: &[super::super::ChapterCommit],
        whole: bool,
    ) -> Result<Vec<String>, GuidedReviewError> {
        if commits.is_empty() {
            return Ok(Vec::new());
        }
        self.ensure_index_is_clean().await?;
        let from_head = self.starts_at_head(review).await?;
        if !from_head && !whole {
            return Err(GuidedReviewError::CommitRefused {
                reason: "this review starts before HEAD, so one commit per chapter cannot be \
                         staged; s squashes the whole review into one",
            });
        }
        self.ensure_worktree_matches(review).await?;
        if !from_head {
            let [commit] = commits else {
                return Err(GuidedReviewError::CommitRefused {
                    reason: "a review that starts before HEAD commits as one; s squashes it",
                });
            };
            return self.commit_reviewed_files(review, &commit.message).await.map(|oid| vec![oid]);
        }

        let mut created = Vec::new();
        for commit in commits {
            if let Err(error) = self.stage_patch(&commit.patch).await {
                // Put the index back the way we found it before surfacing the failure.
                let _ = self.run("reset staged chapter", &["reset", "--quiet"]).await;
                return Err(error);
            }
            match self.commit_index(&commit.message).await {
                Ok(oid) => created.push(oid),
                Err(error) => {
                    let _ = self.run("reset staged chapter", &["reset", "--quiet"]).await;
                    return Err(error);
                }
            }
        }
        Ok(created)
    }

    /// Whether the review's comparison begins at the commit checked out now.
    pub(crate) async fn starts_at_head(
        &self,
        review: &GuidedReview,
    ) -> Result<bool, GuidedReviewError> {
        let ReviewBase::MergeBase { oid, .. } = &review.source.base else {
            return Ok(true);
        };
        let head = self.run("resolve HEAD", &["rev-parse", "HEAD"]).await?;
        Ok(parse_resolved_oid(&head).as_deref() == Some(oid.as_str()))
    }

    /// Stages every file the review covers as it stands, and commits it.
    async fn commit_reviewed_files(
        &self,
        review: &GuidedReview,
        message: &str,
    ) -> Result<String, GuidedReviewError> {
        let mut args = vec!["add".to_string(), "-A".to_string(), "--".to_string()];
        for file in &review.files {
            for path in [&file.old_path, &file.new_path].into_iter().flatten() {
                let path = crate::paths::display(path);
                if !args.contains(&path) {
                    args.push(path);
                }
            }
        }
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        if let Err(error) = self.run("stage the reviewed files", &refs).await {
            let _ = self.run("reset staged review", &["reset", "--quiet"]).await;
            return Err(error);
        }
        match self.commit_index(message).await {
            Ok(oid) => Ok(oid),
            Err(error) => {
                let _ = self.run("reset staged review", &["reset", "--quiet"]).await;
                Err(error)
            }
        }
    }

    /// Rejects a pre-staged index, which a chapter commit would otherwise absorb.
    async fn ensure_index_is_clean(&self) -> Result<(), GuidedReviewError> {
        let staged = self.run("check staged changes", &["diff", "--cached", "--name-only"]).await?;
        if staged.iter().any(|byte| !byte.is_ascii_whitespace()) {
            return Err(GuidedReviewError::CommitRefused {
                reason: "the index already has staged changes; commit or reset them first",
            });
        }
        Ok(())
    }

    /// Rejects a worktree that has moved past the revision under review.
    async fn ensure_worktree_matches(
        &self,
        review: &GuidedReview,
    ) -> Result<(), GuidedReviewError> {
        let policy = UntrackedSourcePolicy::default();
        let capture = match (&review.target, &review.source.base) {
            // Your own pull request names its base already. Capturing the target again would ask
            // GitHub for the pull request, which the commit has no reason to wait on — and the
            // working tree against that base is the whole comparison
            (ReviewTarget::OwnPullRequest { .. }, base @ ReviewBase::MergeBase { .. }) => {
                let captured = self.capture_stable_worktree_source(base, policy).await?;
                self.persist_working_tree_capture(
                    base.clone(),
                    captured.provenance,
                    captured.canonical_source,
                    captured.included_untracked_files,
                )
                .await?
            }
            _ => self.capture_and_persist_target_source(&review.target, policy).await?,
        };
        if capture.source.content_digest != review.revision_id {
            return Err(GuidedReviewError::CommitRefused {
                reason: "the working tree no longer matches the reviewed revision; refresh first",
            });
        }
        Ok(())
    }

    /// Applies one chapter patch to the index only, all or nothing.
    pub(super) async fn stage_patch(&self, patch: &str) -> Result<(), GuidedReviewError> {
        const OPERATION: &str = "stage chapter patch";
        let arguments = command::os_args(&["apply", "--cached", "--whitespace=nowarn", "-"]);
        let deadline = self.git_deadline.min(STAGE_PATCH_TIMEOUT);
        let output = command::run_git_with_input(
            &self.repo_path,
            OPERATION,
            &arguments,
            &[],
            patch.as_bytes(),
            deadline,
        )
        .await?;
        if output.status.success() {
            Ok(())
        } else {
            Err(git_failed(OPERATION, output.status.code(), &output.stderr))
        }
    }

    /// Commits exactly what is staged and returns the new object ID.
    async fn commit_index(&self, message: &str) -> Result<String, GuidedReviewError> {
        self.run("commit chapter", &["commit", "--quiet", "--no-verify", "-m", message]).await?;
        let oid = self.run("resolve chapter commit", &["rev-parse", "HEAD"]).await?;
        Ok(String::from_utf8_lossy(&oid).trim().to_owned())
    }
}
