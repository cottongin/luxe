//! The mirror of `Action`'s known variants that serde derives a decoder from.

use super::super::{Action, ModelRef};
use crate::guided_review::{
    ReviewId, ReviewLoadRequestId, ReviewRevisionId, ReviewSessionSaveRequestId, ReviewTarget,
};
use crate::permission::PermissionRequestId;
use serde::Deserialize;

/// [`Action`]'s known variants, field for field, for serde to derive a decoder from.
///
/// `remote` makes the derive build `Action` itself, so a field this mirror names wrongly does not
/// compile. A variant left out of it decodes as `Other`, which the samples test catches.
#[derive(Deserialize)]
#[serde(remote = "Action", tag = "kind", content = "content", rename_all = "snake_case")]
pub(super) enum KnownAction {
    SubmitUserMessage {
        text: String,
        #[serde(default)]
        images: Vec<crate::engine::ImageAttachment>,
    },
    SteerTurn {
        text: String,
    },
    WithdrawSteering,
    Cancel,
    EditResend {
        text: String,
    },
    Retry,
    Regenerate,
    RespondPermission {
        id: PermissionRequestId,
        reply: crate::permission::PermissionReply,
    },
    AnswerQuestion {
        id: crate::question::QuestionRequestId,
        reply: crate::question::QuestionReply,
    },
    SetPermissionAutoApprove(bool),
    SelectModel(ModelRef),
    SetThinkingLevel(crate::provider::ThinkingLevel),
    SetMode(String),
    RequestReview {
        focus: Option<String>,
    },
    NewSession,
    LoadSession(String),
    RefreshSessions,
    RefreshWorkspace,
    RequestContextComposition,
    CompactNow {
        #[serde(default)]
        focus: Option<String>,
    },
    LoadGuidedReview {
        request_id: ReviewLoadRequestId,
        target: ReviewTarget,
    },
    EditReview {
        request_id: ReviewSessionSaveRequestId,
        review_id: ReviewId,
        #[serde(flatten)]
        edit: crate::engine::ReviewEdit,
    },
    RecordReviewProgress {
        review_id: ReviewId,
        walked_chapter_ids: Vec<crate::guided_review::ChapterId>,
    },
    RecordReviewSeen {
        review_id: ReviewId,
    },
    RefreshGuidedReview {
        request_id: ReviewLoadRequestId,
        review_id: ReviewId,
        revision_id: ReviewRevisionId,
    },
    CommitReviewChapters {
        request_id: ReviewSessionSaveRequestId,
        review_id: ReviewId,
        revision_id: ReviewRevisionId,
        #[serde(default)]
        chapter_ids: Vec<crate::guided_review::ChapterId>,
        #[serde(default)]
        squash: bool,
    },
    PreparePullRequest {
        request_id: crate::guided_review::PullRequestOpenId,
        review_id: ReviewId,
    },
    OpenPullRequest {
        request_id: crate::guided_review::PullRequestOpenId,
        title: String,
        body: String,
        #[serde(default)]
        draft: bool,
    },
    PrepareStack {
        request_id: crate::guided_review::PullRequestOpenId,
        review_id: ReviewId,
    },
    OpenStack {
        request_id: crate::guided_review::PullRequestOpenId,
        #[serde(default)]
        draft: bool,
    },
    PreparePullRequestPost {
        request_id: crate::guided_review::PullRequestPostId,
        review_id: ReviewId,
    },
    PostPullRequestReview {
        request_id: crate::guided_review::PullRequestPostId,
        verdict: crate::github::ReviewEvent,
        session_revision: u64,
    },
    PrepareThreadAnswers {
        request_id: crate::guided_review::ThreadAnswersId,
        review_id: ReviewId,
    },
    AnswerThreads {
        request_id: crate::guided_review::ThreadAnswersId,
        review_id: ReviewId,
        answers: Vec<crate::guided_review::ThreadReply>,
    },
    ListGithub {
        #[serde(default)]
        sections: Option<Vec<crate::config::GithubSection>>,
    },
    SearchGithub {
        request: u64,
        section: crate::config::GithubSection,
        #[serde(default)]
        after: Option<String>,
    },
    ReadGithubOverview {
        #[serde(default)]
        repository: Option<String>,
        number: u64,
        pull_request: bool,
    },
    ReadGithubConversation {
        #[serde(default)]
        repository: Option<String>,
        number: u64,
        pull_request: bool,
    },
    DraftGithubReply(crate::engine::DraftRequest),
    CheckPullRequestFix {
        number: u64,
        #[serde(default)]
        repository: Option<String>,
    },
    PlanRestack {
        request_id: crate::restack::RestackId,
    },
    CheckWorktreeRemoval,
    LoadFileDiff {
        request_id: crate::file_diff::FileDiffRequestId,
        path: String,
    },
    ListDirectory {
        request_id: crate::project_files::ListingRequestId,
        #[serde(default)]
        path: String,
        #[serde(default)]
        include_ignored: bool,
    },
    ReadProjectFile {
        request_id: crate::project_files::FileReadRequestId,
        path: String,
    },
    SetFocus {
        focused: bool,
    },
    LinkPullRequest {
        number: u64,
        url: String,
    },
    PostGithubComment(crate::engine::GithubPost),
    ChangePullRequest {
        #[serde(default)]
        repository: Option<String>,
        number: u64,
        write: crate::engine::PullRequestWrite,
    },
    Restack {
        request_id: crate::restack::RestackId,
    },
    Close,
}
