//! Engine GitHub change — what a status row's key changes on GitHub: the branch brought up to
//! date, the draft marked ready, the pull request merged, auto-merge turned on or off.
//!
//! Each is the reader's own act, taken from a key and, for a merge, a confirmation they read, so
//! the keypress is the approval, the way posting a review is; none of this is reachable from a
//! tool the agent calls. Each names the head the reader saw where GitHub takes one, so a push that
//! landed in between is refused rather than merged unread.

use super::posting::sentence;
use super::*;
use crate::github::overview::{MergeMethod, MergeRequest};
use crate::github::GitHubError;

/// A change made to a pull request on GitHub.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "change", rename_all = "snake_case")]
pub enum PullRequestChange {
    /// Its branch is being brought up to date with its base; GitHub finishes after answering
    BranchUpdated,
    MarkedReady,
    Merged {
        /// The commit the merge made on the base
        sha: String,
        /// Whether its branch is gone
        branch_deleted: bool,
    },
    /// It merges by itself, with this method, once nothing stands in the way
    AutoMergeEnabled {
        method: MergeMethod,
    },
    AutoMergeDisabled,
}

/// What the reader asked to change.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "change", rename_all = "snake_case")]
pub enum PullRequestWrite {
    UpdateBranch {
        /// The head the reader saw
        head_oid: String,
    },
    MarkReady,
    Merge(MergeRequest),
    EnableAutoMerge {
        /// GitHub's id for the pull request, which its mutation names
        node_id: String,
        method: MergeMethod,
        /// The head the reader saw
        head_oid: String,
    },
    DisableAutoMerge {
        /// GitHub's id for the pull request
        node_id: String,
    },
}

impl Engine {
    /// Makes `write` to pull request `number` and answers what changed, or why nothing did.
    pub(crate) fn handle_change_pull_request(
        &self,
        repository: Option<String>,
        number: u64,
        write: PullRequestWrite,
        event_tx: &mpsc::Sender<Event>,
    ) {
        let github = self.config.github.clone();
        let root = self.config.guided_review_project_root.clone();
        let event_tx = event_tx.clone();
        tokio::spawn(async move {
            let asked = repository.clone();
            let changed = async {
                let (github, repository) =
                    super::github_overview::reach(github, root, repository).await?;
                let done = match &write {
                    PullRequestWrite::UpdateBranch { head_oid } => github
                        .update_branch(&repository, number, head_oid)
                        .await
                        .map(|()| PullRequestChange::BranchUpdated),
                    PullRequestWrite::MarkReady => github
                        .mark_ready(&repository, number)
                        .await
                        .map(|()| PullRequestChange::MarkedReady),
                    PullRequestWrite::Merge(request) => github
                        .merge_pull_request(&repository, number, request)
                        .await
                        .map(|merged| PullRequestChange::Merged {
                            sha: merged.sha,
                            branch_deleted: merged.branch_deleted,
                        }),
                    PullRequestWrite::EnableAutoMerge { node_id, method, head_oid } => github
                        .set_auto_merge(&repository, node_id, Some((*method, head_oid)))
                        .await
                        .map(|()| PullRequestChange::AutoMergeEnabled { method: *method }),
                    PullRequestWrite::DisableAutoMerge { node_id } => github
                        .set_auto_merge(&repository, node_id, None)
                        .await
                        .map(|()| PullRequestChange::AutoMergeDisabled),
                };
                done.map_err(|error| refused(&write, &error))
            };
            let (change, error) = match changed.await {
                Ok(change) => (Some(change), None),
                Err(reason) => (None, Some(reason)),
            };
            let event = Event::PullRequestChanged { repository: asked, number, change, error };
            let _ = event_tx.send(event).await;
        });
    }
}

/// Why `write` did not happen, as a sentence: GitHub's own reason when it gave one.
fn refused(write: &PullRequestWrite, error: &GitHubError) -> String {
    let what = match write {
        PullRequestWrite::UpdateBranch { .. } => "the update",
        PullRequestWrite::MarkReady => "marking it ready",
        PullRequestWrite::Merge(_) => "the merge",
        PullRequestWrite::EnableAutoMerge { .. } => "auto-merge",
        PullRequestWrite::DisableAutoMerge { .. } => "turning auto-merge off",
    };
    match error {
        GitHubError::Rejected { reasons } => {
            let said = reasons.join("; ");
            format!("GitHub refused {what}: {}", said.trim_end_matches('.').to_owned() + ".")
        }
        other => sentence(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// GitHub's own reason reaches the reader, named for what was refused.
    #[test]
    fn a_refusal_says_what_was_refused_and_why() {
        let merge = PullRequestWrite::Merge(MergeRequest {
            method: MergeMethod::Merge,
            head_oid: "abc".into(),
            delete_branch: None,
        });
        let moved = GitHubError::Rejected {
            reasons: vec!["Head branch was modified. Review and try the merge again.".into()],
        };
        assert_eq!(
            refused(&merge, &moved),
            "GitHub refused the merge: Head branch was modified. Review and try the merge again."
        );
        let update = PullRequestWrite::UpdateBranch { head_oid: "abc".into() };
        assert!(
            refused(&update, &GitHubError::NotAuthenticated { host: None }).contains("signed in")
        );
    }
}
