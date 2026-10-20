//! The mirror of `Event`'s known variants that serde derives a decoder from.

use super::super::{
    EngineState, Event, GuidedReviewFailure, RequestedReviewVerdict, SessionMessage, StopReason,
};
use crate::activity::ToolActivity;
use crate::guided_review::{
    GuideProvenance, GuidedReview, RemediationReport, ReviewChapter, ReviewId, ReviewLoadRequestId,
    ReviewRevisionId, ReviewSession, ReviewSessionSaveRequestId, ReviewTarget, SubmissionId,
};
use crate::message::Message;
use crate::permission::PermissionRequestId;
use crate::session::EntryId;
use crate::workspace::WorkspaceSnapshot;
use serde::Deserialize;

/// [`Event`]'s known variants, field for field; see [`KnownAction`] for how the mirror is held.
#[derive(Deserialize)]
#[serde(remote = "Event", tag = "kind", content = "content", rename_all = "snake_case")]
pub(super) enum KnownEvent {
    TokenDelta {
        #[serde(default)]
        block: usize,
        delta: String,
    },
    ThinkingDelta {
        #[serde(default)]
        block: usize,
        delta: String,
    },
    ToolCallComposing {
        call_id: String,
        name: String,
        subject: Option<String>,
        body: Option<crate::activity::ToolCardBody>,
    },
    ToolStarted {
        call_id: String,
        name: String,
    },
    ToolProgress {
        call_id: String,
        chunk: String,
    },
    ToolCompleted {
        call_id: String,
        is_error: bool,
        output: String,
    },
    ToolEnded {
        call_id: String,
        is_error: bool,
    },
    MessageCommitted {
        session_id: String,
        entry_id: EntryId,
        message: Message,
    },
    RemediationReported {
        entry_id: EntryId,
        report: RemediationReport,
    },
    ReviewFeedbackTurnDispatched {
        submission_id: SubmissionId,
    },
    ReviewFeedbackTurnFailed {
        submission_id: SubmissionId,
        message: String,
    },
    ReviewQuestionsAnswered {
        submission_id: SubmissionId,
        answer: String,
    },
    ReviewFeedbackTurnCompleted {
        submission_id: SubmissionId,
    },
    SessionLoaded {
        session_id: String,
        messages: Vec<SessionMessage>,
    },
    StateChanged(EngineState),
    TurnStarted,
    TurnEnded {
        reason: StopReason,
    },
    ModelsListed {
        request_id: String,
        models: Vec<super::super::ModelChoice>,
    },
    LastTurnAvailable {
        session_id: String,
        turn_id: String,
    },
    CompactionStarted {
        tokens_before: u64,
    },
    CompactionDone {
        tokens_before: u64,
        tokens_after: u64,
    },
    CompactionStopped,
    ContextUsage {
        used_tokens: u32,
        context_window: u32,
    },
    TurnUsage {
        prompt_tokens: u64,
        completion_tokens: u64,
        cost_usd: Option<f64>,
        #[serde(default)]
        cache_read_tokens: u64,
        #[serde(default)]
        cache_write_tokens: u64,
        #[serde(default)]
        reasoning_tokens: u64,
        #[serde(default)]
        source: crate::usage::UsageSource,
    },
    SessionUsage(crate::usage::SessionUsage),
    PlanUsage(crate::provider::PlanUsage),
    ContextComposition(Box<crate::context_report::ContextComposition>),
    ImageAttached {
        mime: String,
        width: u32,
        height: u32,
    },
    Error(String),
    Notice(String),
    GuestsChanged(Vec<String>),
    PlanWritten {
        path: String,
    },
    ReviewStarted {
        model: String,
    },
    ReviewFinished {
        verdict: RequestedReviewVerdict,
        #[serde(default)]
        findings: Vec<String>,
    },
    SteeringDelivered {
        count: usize,
    },
    ProviderRetrying {
        attempt: u32,
        attempts: u32,
        reason: String,
    },
    GuidedReviewLoading {
        request_id: ReviewLoadRequestId,
        target: ReviewTarget,
    },
    GuidedReviewLoaded {
        request_id: ReviewLoadRequestId,
        review: Box<GuidedReview>,
    },
    GuidedReviewGuideUpdated {
        request_id: ReviewLoadRequestId,
        review_id: ReviewId,
        revision_id: ReviewRevisionId,
        chapters: Vec<ReviewChapter>,
        provenance: GuideProvenance,
        #[serde(default)]
        brief: Option<crate::guided_review::ReviewBrief>,
    },
    PullRequestPostPrepared {
        request_id: crate::guided_review::PullRequestPostId,
        patches: Vec<crate::github::FilePatch>,
        head_moved: bool,
        own_pull_request: bool,
    },
    PullRequestReviewPosted {
        request_id: crate::guided_review::PullRequestPostId,
        github_review_id: u64,
        url: String,
        finding_ids: Vec<crate::guided_review::FindingId>,
    },
    PullRequestPostFailed {
        request_id: crate::guided_review::PullRequestPostId,
        message: String,
    },
    ThreadAnswersPrepared {
        request_id: crate::guided_review::ThreadAnswersId,
        pushed: bool,
        uncommitted: bool,
    },
    ThreadsAnswered {
        request_id: crate::guided_review::ThreadAnswersId,
        results: Vec<crate::guided_review::ThreadAnswerResult>,
    },
    ThreadAnswersFailed {
        request_id: crate::guided_review::ThreadAnswersId,
        message: String,
    },
    GithubListed {
        repository: Option<String>,
        sections: Vec<crate::github::ListedSection>,
    },
    GithubSearched {
        request: u64,
        repository: Option<String>,
        section: crate::github::ListedSection,
    },
    GithubOverviewRead {
        repository: Option<String>,
        number: u64,
        overview: Option<crate::github::overview::Overview>,
        error: Option<String>,
        #[serde(default)]
        local: Option<crate::github::overview::LocalSession>,
    },
    GithubReplyDrafted {
        request_id: String,
        body: Option<String>,
        #[serde(default)]
        sources: Vec<String>,
        error: Option<String>,
        #[serde(default)]
        note: Option<String>,
    },
    PullRequestChanged {
        repository: Option<String>,
        number: u64,
        change: Option<crate::engine::PullRequestChange>,
        error: Option<String>,
    },
    GithubCommentPosted {
        repository: Option<String>,
        number: u64,
        thread: Option<String>,
        url: Option<String>,
        error: Option<String>,
    },
    GithubConversationRead {
        repository: Option<String>,
        number: u64,
        entries: Option<Vec<crate::github::overview::ConversationEntry>>,
        error: Option<String>,
    },
    StackPrepared {
        request_id: crate::guided_review::PullRequestOpenId,
        layers: Option<Vec<crate::github::NewPullRequest>>,
        refusal: Option<String>,
    },
    StackOpened {
        request_id: crate::guided_review::PullRequestOpenId,
        numbers: Vec<u64>,
        urls: Vec<String>,
        failed: Option<String>,
    },
    RestackPlanned {
        request_id: crate::restack::RestackId,
        plan: Option<crate::restack::RestackPlan>,
        merged: Option<crate::restack::MergedBelow>,
        refusal: Option<String>,
    },
    Restacked {
        request_id: crate::restack::RestackId,
        moved: Vec<u64>,
        retargeted: Vec<u64>,
        retarget_failed: Option<String>,
    },
    RestackFailed {
        request_id: crate::restack::RestackId,
        message: String,
    },
    PullRequestFixChecked {
        number: u64,
        branch: Option<String>,
        refusal: Option<String>,
    },
    PullRequestPrepared {
        request_id: crate::guided_review::PullRequestOpenId,
        pull_request: crate::github::NewPullRequest,
        publication: crate::github::Publication,
        uncommitted: usize,
        commits: Vec<String>,
    },
    PullRequestOpened {
        request_id: crate::guided_review::PullRequestOpenId,
        number: u64,
        url: String,
        committed: Vec<String>,
    },
    PullRequestOpenFailed {
        request_id: crate::guided_review::PullRequestOpenId,
        message: String,
    },
    WorktreeRemovalChecked {
        uncommitted: Vec<String>,
    },
    ReviewCommitted {
        request_id: ReviewSessionSaveRequestId,
        commits: Vec<String>,
        skipped: Vec<String>,
    },
    ReviewCommitFailed {
        request_id: ReviewSessionSaveRequestId,
        message: String,
    },
    GuidedReviewFailed {
        request_id: ReviewLoadRequestId,
        target: ReviewTarget,
        error: GuidedReviewFailure,
    },
    FileDiffLoaded {
        request_id: crate::file_diff::FileDiffRequestId,
        diff: Box<crate::file_diff::FileDiff>,
    },
    FileDiffFailed {
        request_id: crate::file_diff::FileDiffRequestId,
        path: String,
        error: crate::file_diff::FileDiffFailure,
    },
    DirectoryListed {
        request_id: crate::project_files::ListingRequestId,
        listing: crate::project_files::DirectoryListing,
    },
    DirectoryListFailed {
        request_id: crate::project_files::ListingRequestId,
        path: String,
        error: crate::project_files::ProjectFileFailure,
    },
    ProjectFileRead {
        request_id: crate::project_files::FileReadRequestId,
        file: Box<crate::project_files::ProjectFile>,
    },
    ProjectFileReadFailed {
        request_id: crate::project_files::FileReadRequestId,
        path: String,
        error: crate::project_files::ProjectFileFailure,
    },
    ReviewSessionRestored {
        session: Box<ReviewSession>,
    },
    OpenReviewShown(Box<super::super::OpenReview>),
    ReviewSessionSaved {
        request_id: ReviewSessionSaveRequestId,
        session: Box<ReviewSession>,
    },
    ReviewSessionSaveFailed {
        request_id: ReviewSessionSaveRequestId,
        review_id: ReviewId,
        message: String,
    },
    SessionsRefreshed(Vec<String>),
    WorkspaceRefreshRequested,
    TerminalFocusChanged {
        focused: bool,
    },
    WorkspaceUpdated(Box<WorkspaceSnapshot>),
    ToolActivityUpdated(Box<ToolActivity>),
    PermissionRequested {
        id: PermissionRequestId,
        session_id: String,
        tool: String,
        permission: String,
        summary: String,
        #[serde(default)]
        always_patterns: Vec<String>,
    },
    QuestionAsked {
        id: crate::question::QuestionRequestId,
        session_id: String,
        prompt: String,
        #[serde(default)]
        options: Vec<crate::question::QuestionOption>,
        #[serde(default)]
        multi: bool,
    },
    PromptClosed {
        id: String,
    },
    Ready {
        protocol_version: u32,
        luxe_version: String,
        session_id: String,
        cwd: String,
        #[serde(default)]
        github_writes: bool,
    },
    ActionRejected {
        #[serde(default)]
        kind: Option<String>,
        reason: String,
    },
}
