//! Review outcomes — the finish chooser's choices and the commit preview.

/// A pending commit, showing the messages before any history is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommitPreview {
    /// Collapse every chapter into one commit.
    pub(crate) squash: bool,
    /// Whether the request is in flight.
    pub(crate) running: bool,
}

/// Explicit user choice shown before closing or submitting a review.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FinishReviewChoice {
    /// Record an accepted outcome without contacting the agent.
    Accept,
    /// Record acceptance and turn the reviewed chapters into commits.
    AcceptAndCommit,
    /// Send actionable findings to the active agent.
    RequestFixes,
    /// Send review comments without requesting edits.
    CommentOnly,
    /// Reply to and resolve your own pull request's threads on GitHub.
    AnswerOnGitHub,
    /// Commit the reviewed work, push the branch, and open its pull request.
    OpenPullRequest,
    /// Commit the reviewed work chapter by chapter and open it as a stack of pull requests.
    OpenStack,
    /// Close the local chooser without changing durable review state.
    Cancel,
}

impl FinishReviewChoice {
    /// Moves through the finish-review choices in presentation order; `answers` offers answering
    /// on GitHub, which only your own pull request's review has, and `opens` offers opening the
    /// branch's pull request, which only a branch without one has.
    pub(in crate::guided_review) fn next(
        self,
        forward: bool,
        answers: bool,
        opens: bool,
        stacks: bool,
    ) -> Self {
        const ORDER: [FinishReviewChoice; 8] = [
            FinishReviewChoice::Accept,
            FinishReviewChoice::AcceptAndCommit,
            FinishReviewChoice::RequestFixes,
            FinishReviewChoice::CommentOnly,
            FinishReviewChoice::AnswerOnGitHub,
            FinishReviewChoice::OpenPullRequest,
            FinishReviewChoice::OpenStack,
            FinishReviewChoice::Cancel,
        ];
        let order: Vec<FinishReviewChoice> = ORDER
            .into_iter()
            .filter(|choice| answers || *choice != FinishReviewChoice::AnswerOnGitHub)
            .filter(|choice| opens || *choice != FinishReviewChoice::OpenPullRequest)
            .filter(|choice| stacks || *choice != FinishReviewChoice::OpenStack)
            .collect();
        let index = order.iter().position(|choice| *choice == self).unwrap_or(0);
        let next = if forward {
            (index + 1) % order.len()
        } else {
            index.checked_sub(1).unwrap_or(order.len() - 1)
        };
        order[next]
    }
}
