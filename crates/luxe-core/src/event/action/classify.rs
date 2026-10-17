//! What an `Action` is called on the wire, and which side of the write lines it falls on.

use super::Action;

impl Action {
    /// The `kind` this action serializes as.
    ///
    /// A `match` rather than a lookup, so the compiler keeps it complete: adding a variant stops
    /// this compiling until its kind is written down. `#[non_exhaustive]` binds other crates, not
    /// this one, which is why the list is published from here — a translator in `luxe-cli` cannot
    /// match this enum exhaustively even when it covers every variant.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::SubmitUserMessage { .. } => "submit_user_message",
            Self::SteerTurn { .. } => "steer_turn",
            Self::WithdrawSteering => "withdraw_steering",
            Self::Cancel => "cancel",
            Self::EditResend { .. } => "edit_resend",
            Self::Retry => "retry",
            Self::Regenerate => "regenerate",
            Self::RespondPermission { .. } => "respond_permission",
            Self::AnswerQuestion { .. } => "answer_question",
            Self::SetPermissionAutoApprove(_) => "set_permission_auto_approve",
            Self::SelectModel(_) => "select_model",
            Self::SetThinkingLevel(_) => "set_thinking_level",
            Self::SetMode(_) => "set_mode",
            Self::RequestReview { .. } => "request_review",
            Self::NewSession => "new_session",
            Self::LoadSession(_) => "load_session",
            Self::RefreshSessions => "refresh_sessions",
            Self::RefreshWorkspace => "refresh_workspace",
            Self::RequestContextComposition => "request_context_composition",
            Self::CompactNow { .. } => "compact_now",
            Self::LoadGuidedReview { .. } => "load_guided_review",
            Self::EditReview { .. } => "edit_review",
            Self::RecordReviewProgress { .. } => "record_review_progress",
            Self::RecordReviewSeen { .. } => "record_review_seen",
            Self::RefreshGuidedReview { .. } => "refresh_guided_review",
            Self::CommitReviewChapters { .. } => "commit_review_chapters",
            Self::PreparePullRequest { .. } => "prepare_pull_request",
            Self::OpenPullRequest { .. } => "open_pull_request",
            Self::PrepareStack { .. } => "prepare_stack",
            Self::OpenStack { .. } => "open_stack",
            Self::PreparePullRequestPost { .. } => "prepare_pull_request_post",
            Self::PostPullRequestReview { .. } => "post_pull_request_review",
            Self::PrepareThreadAnswers { .. } => "prepare_thread_answers",
            Self::AnswerThreads { .. } => "answer_threads",
            Self::ListGithub { .. } => "list_github",
            Self::SearchGithub { .. } => "search_github",
            Self::ReadGithubOverview { .. } => "read_github_overview",
            Self::ReadGithubConversation { .. } => "read_github_conversation",
            Self::DraftGithubReply(_) => "draft_github_reply",
            Self::CheckPullRequestFix { .. } => "check_pull_request_fix",
            Self::PlanRestack { .. } => "plan_restack",
            Self::CheckWorktreeRemoval => "check_worktree_removal",
            Self::LoadFileDiff { .. } => "load_file_diff",
            Self::ListDirectory { .. } => "list_directory",
            Self::ReadProjectFile { .. } => "read_project_file",
            Self::SetFocus { .. } => "set_focus",
            Self::LinkPullRequest { .. } => "link_pull_request",
            Self::PostGithubComment(_) => "post_github_comment",
            Self::ChangePullRequest { .. } => "change_pull_request",
            Self::Restack { .. } => "restack",
            Self::Close => "close",
            // The one variant whose tag is not known until runtime, so it reports the category.
            // A caller that needs the client's actual string reads it off the variant.
            Self::Other { .. } => "other",
        }
    }

    /// Every `kind` a client may send, `other` excluded.
    ///
    /// Published so a front-end can advertise what it supports and a translator in another crate
    /// can be held to the whole list — `#[non_exhaustive]` makes an exhaustive match impossible
    /// there. `the_published_kind_list_matches_the_variants` keeps it honest against [`Self::kind`].
    pub const KINDS: &'static [&'static str] = &[
        "submit_user_message",
        "steer_turn",
        "withdraw_steering",
        "cancel",
        "edit_resend",
        "retry",
        "regenerate",
        "respond_permission",
        "answer_question",
        "set_permission_auto_approve",
        "select_model",
        "set_thinking_level",
        "set_mode",
        "request_review",
        "new_session",
        "load_session",
        "refresh_sessions",
        "refresh_workspace",
        "request_context_composition",
        "compact_now",
        "load_guided_review",
        "edit_review",
        "record_review_progress",
        "record_review_seen",
        "refresh_guided_review",
        "commit_review_chapters",
        "prepare_pull_request",
        "open_pull_request",
        "prepare_stack",
        "open_stack",
        "prepare_pull_request_post",
        "post_pull_request_review",
        "prepare_thread_answers",
        "answer_threads",
        "list_github",
        "search_github",
        "read_github_overview",
        "read_github_conversation",
        "draft_github_reply",
        "check_pull_request_fix",
        "plan_restack",
        "check_worktree_removal",
        "load_file_diff",
        "list_directory",
        "read_project_file",
        "set_focus",
        "link_pull_request",
        "post_github_comment",
        "change_pull_request",
        "restack",
        "close",
    ];

    /// Whether this action writes the repository's own history without publishing anything:
    /// committing a review's chapters.
    ///
    /// A read-only session refuses it, the way the permission engine refuses the agent's own
    /// commits there. Every GitHub write commits or pushes too, and is refused there already.
    pub fn writes_the_repository(&self) -> bool {
        matches!(self, Self::CommitReviewChapters { .. })
    }

    /// Whether this action publishes to GitHub: posts, pushes, merges, or changes a pull request.
    ///
    /// The line `luxe --rpc` draws without `--github-writes`. Every variant is named rather than
    /// matched by a wildcard, so a new action does not compile until someone has decided which side
    /// of that line it is on. Reading GitHub, drafting, and planning are not writes; neither is
    /// recording a pull request on the session, which never leaves the machine.
    pub fn writes_to_github(&self) -> bool {
        match self {
            Self::PostGithubComment(_)
            | Self::ChangePullRequest { .. }
            | Self::Restack { .. }
            | Self::OpenPullRequest { .. }
            | Self::OpenStack { .. }
            | Self::PostPullRequestReview { .. }
            | Self::AnswerThreads { .. } => true,
            Self::SubmitUserMessage { .. }
            | Self::SteerTurn { .. }
            | Self::WithdrawSteering
            | Self::Cancel
            | Self::EditResend { .. }
            | Self::Retry
            | Self::Regenerate
            | Self::RespondPermission { .. }
            | Self::AnswerQuestion { .. }
            | Self::SetPermissionAutoApprove(_)
            | Self::SelectModel(_)
            | Self::SetThinkingLevel(_)
            | Self::SetMode(_)
            | Self::RequestReview { .. }
            | Self::NewSession
            | Self::LoadSession(_)
            | Self::RefreshSessions
            | Self::RefreshWorkspace
            | Self::RequestContextComposition
            | Self::CompactNow { .. }
            | Self::LoadGuidedReview { .. }
            | Self::EditReview { .. }
            | Self::RecordReviewProgress { .. }
            | Self::RecordReviewSeen { .. }
            | Self::RefreshGuidedReview { .. }
            | Self::CommitReviewChapters { .. }
            | Self::PreparePullRequest { .. }
            | Self::PrepareStack { .. }
            | Self::PreparePullRequestPost { .. }
            | Self::PrepareThreadAnswers { .. }
            | Self::ListGithub { .. }
            | Self::SearchGithub { .. }
            | Self::ReadGithubOverview { .. }
            | Self::ReadGithubConversation { .. }
            | Self::DraftGithubReply(_)
            | Self::CheckPullRequestFix { .. }
            | Self::PlanRestack { .. }
            | Self::CheckWorktreeRemoval
            | Self::LoadFileDiff { .. }
            | Self::ListDirectory { .. }
            | Self::ReadProjectFile { .. }
            | Self::SetFocus { .. }
            | Self::LinkPullRequest { .. }
            | Self::Close
            | Self::Other { .. } => false,
        }
    }
}
