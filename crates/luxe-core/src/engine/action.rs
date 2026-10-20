//! Action — everything a front end or the engine itself can ask the engine to do.

use super::*;

/// Immutable evidence that binds an internal agent turn to a submitted review snapshot.
#[derive(Debug, Clone)]
pub struct RemediationTurnContext {
    /// Persisted feedback submission that started this turn, when the turn came from review UI.
    pub submission_id: Option<crate::guided_review::SubmissionId>,
    /// Logical review that owns the submitted findings.
    pub review_id: crate::guided_review::ReviewId,
    /// Immutable source revision the agent was asked to address.
    pub revision_id: crate::guided_review::ReviewRevisionId,
    /// Finding IDs present in the immutable submitted snapshot, in the order the agent read them.
    pub finding_ids: Vec<crate::guided_review::FindingId>,
    /// Whether the submission only asks questions, which holds the turn to reading.
    pub answers_only: bool,
    /// Whether the agent was asked to report on each comment, which only a fix request does.
    pub reports: bool,
}

/// Actions that can be sent to the engine.
#[derive(Debug, Clone)]
pub enum Action {
    /// Submit a user message with optional attached images and internal turn evidence.
    SubmitUserMessage {
        content: String,
        /// Base64-encoded image data with MIME type.
        images: Vec<ImageAttachment>,
        /// Submitted review evidence eligible for a matching agent remediation report.
        remediation: Option<RemediationTurnContext>,
    },
    /// Steer the in-flight turn: deliver `content` to the model as a user message at the next
    /// checkpoint between tool iterations, without ending the turn or starting a new one.
    ///
    /// With no turn running it behaves as a plain submission, so a front-end can send it without
    /// racing the turn's lifecycle.
    SteerTurn { content: String },
    /// Drop every steering message admitted but not yet delivered, so a front-end can take back
    /// what the user typed and let them edit it instead.
    ///
    /// A steer reaches the engine the moment it is submitted — that is what makes it arrive at the
    /// next checkpoint rather than after the turn — so a UI cannot recall one on its own. Nothing is
    /// reported back: the front-end holds the text (and any images) the user actually typed, which
    /// the engine never sees, so it restores from its own copy. A steer already handed to the model
    /// stays delivered; the buffer is simply empty by then.
    WithdrawSteering,
    /// Cancel the current turn.
    Cancel,
    /// Answer a pending permission prompt (matches an `Event::PermissionRequested` by `id`).
    RespondPermission {
        /// The prompt being answered.
        id: crate::permission::PermissionRequestId,
        /// The user's choice.
        reply: crate::permission::PermissionReply,
    },
    /// Answer a pending interactive question (matches an `Event::QuestionAsked` by `id`).
    AnswerQuestion {
        /// The question being answered.
        id: crate::question::QuestionRequestId,
        /// The user's answer (chosen options / free text) or a cancellation.
        reply: crate::question::QuestionReply,
    },
    /// Toggle auto-approve: when on, permission `ask`s are auto-allowed (deny still enforced),
    /// so the loop runs unattended. Off restores prompting. No effect on a front-end that can't
    /// prompt (headless/loop already auto-allow).
    SetPermissionAutoApprove(bool),
    /// Close the engine (shutdown). A turn in flight stops first, the way `Cancel` stops it.
    Close,
    /// Switch the session's mode, by name (`build`, `plan`).
    SetMode(String),
    /// Ask a reviewer in a fresh context to judge the uncommitted work, with what to look at
    /// hardest. Findings it would fix go back to the agent as a turn.
    RequestReview {
        /// What to look at hardest, when the user said
        focus: Option<String>,
    },
    /// A review asked for with [`Action::RequestReview`] came back. Internal: the engine sends it
    /// to itself from the task the review ran on.
    ReviewReturned(crate::phase_review::PhaseReviewOutcome),
    /// Load a specific session by ID.
    LoadSession(String),
    /// Refresh the session list (rescan filesystem).
    RefreshSessions,
    /// Request a fresh project-local workspace snapshot from the runtime collector.
    RefreshWorkspace,
    /// Context the next user message carries, once, appended after what the user wrote: an issue
    /// taken up as the session's brief. In the transcript rather than the system prompt, so the
    /// cached prefix holds and a resume keeps it.
    SetSessionBrief(String),
    /// Asks what in the working tree is not committed, before its worktree is removed.
    CheckWorktreeRemoval,
    /// Records that this session worked on a pull request, once.
    LinkPullRequest {
        /// Number within its repository
        number: u64,
        /// Its web address
        url: String,
    },
    /// The terminal gained or lost focus, which paces background refreshes.
    FocusChanged {
        /// Whether it now has focus
        focused: bool,
    },
    /// Create a new session.
    NewSession,
    /// Switch the active model in-session (id/provider/context/thinking come from `ModelInfo`).
    SetModel(crate::provider::ModelInfo),
    /// Set the active thinking level; the turn clamps it to what the active model supports.
    SetThinkingLevel(crate::provider::ThinkingLevel),
    /// Runs `/github`'s sections, each a GitHub search, and answers them together.
    ListGithub {
        /// The sections, with `{repo}` still in their queries
        sections: Vec<crate::config::GithubSection>,
    },
    /// Runs one of `/github`'s searches on its own: a tab's edited query, or the next page of one.
    SearchGithub {
        /// The front-end's id for it, answered in `GithubSearched`
        request: u64,
        /// The section, with `{repo}` still in its query
        section: crate::config::GithubSection,
        /// The cursor the page starts after; `None` for the first page
        after: Option<String>,
    },
    /// Reads one pull request's or issue's overview for `/pr` or `/github`'s detail.
    ReadGithubOverview {
        /// `owner/name` when it is not the checkout's repository
        repository: Option<String>,
        /// Its number
        number: u64,
        /// Whether it is a pull request, rather than an issue
        pull_request: bool,
    },
    /// Posts a reply or comment the reader wrote and confirmed.
    PostGithubComment(super::GithubPost),
    /// Has the agent draft a reply for the reader to read, change and send, in a turn that changes
    /// nothing.
    DraftGithubReply(super::DraftRequest),
    /// Changes a pull request on GitHub as the reader asked from its Status: brings its branch up
    /// to date, marks it ready, or merges it.
    ChangePullRequest {
        /// `owner/name` when it is not the checkout's repository
        repository: Option<String>,
        /// Its number
        number: u64,
        /// What to change
        write: super::PullRequestWrite,
    },
    /// Reads everything said about a pull request or an issue, for its Conversation tab.
    ReadGithubConversation {
        /// `owner/name` when it is not the checkout's repository
        repository: Option<String>,
        /// Its number
        number: u64,
        /// Whether it is a pull request, rather than an issue
        pull_request: bool,
    },
    /// Drafts the open review as a stack of pull requests, one per chapter, for the preview. The
    /// engine keeps the layers for the `OpenStack` that confirms them.
    PrepareStack {
        /// Names the preview, for the confirm
        request_id: crate::guided_review::PullRequestOpenId,
        /// The review whose chapters are the layers
        review_id: ReviewId,
    },
    /// Commits the reviewed chapters, branches and pushes the layers, and opens their pull
    /// requests from the bottom, exactly as `PrepareStack` drafted them.
    OpenStack {
        /// The preview being confirmed
        request_id: crate::guided_review::PullRequestOpenId,
        /// Open every layer as a draft
        draft: bool,
    },
    /// Works out what restacking the checked-out branch's stack would do, making nothing visible.
    ///
    /// The engine keeps the plan under `request_id` for the [`Action::Restack`] that confirms it. A
    /// newer plan supersedes it.
    PlanRestack {
        /// Names the plan, for the push that confirms it
        request_id: crate::restack::RestackId,
    },
    /// Pushes the plan made under `request_id` exactly as planned, then retargets the pull requests
    /// whose base moved. Refused when a newer plan superseded it, or it was already pushed.
    Restack {
        /// The plan the reader was shown
        request_id: crate::restack::RestackId,
    },
    /// Asks whether pull request `number` can be checked out here to fix: open, pushable, on this
    /// checkout's repository, and not checked out already.
    CheckPullRequestFix {
        /// Number within its repository
        number: u64,
        /// `owner/name` when the review read it from a repository other than the checkout's own
        repository: Option<String>,
    },
    /// Drafts the pull request the open review would open, and reads whether the branch is pushed
    /// and what is not committed, for the preview to warn about. The engine keeps the draft for the
    /// `OpenPullRequest` that confirms it.
    PreparePullRequest {
        /// Names the preview, for the confirm
        request_id: crate::guided_review::PullRequestOpenId,
        /// The review the description is drafted from
        review_id: ReviewId,
    },
    /// Commits the reviewed work when the preview said it would, pushes the branch when it has to,
    /// then opens the drafted pull request with the reader's title, description and draft choice.
    OpenPullRequest {
        /// The preview being confirmed
        request_id: crate::guided_review::PullRequestOpenId,
        /// Its title, as the reader left it
        title: String,
        /// Its description, as the reader left it
        body: String,
        /// Open it as a draft
        draft: bool,
    },
    /// Reads what posting the open pull request review needs: GitHub's patches, whether the head
    /// has moved, and whether the reader is the author. The engine keeps them for the
    /// `PostPullRequestReview` that confirms it.
    PreparePullRequestPost {
        /// Names the preview, for the confirm
        request_id: crate::guided_review::PullRequestPostId,
        /// The pull request's review
        review_id: ReviewId,
    },
    /// Reads whether the fixes behind a thread answer are on GitHub yet: pushed, and committed.
    PrepareThreadAnswers {
        /// Correlates the answer with the preview that asked
        request_id: crate::guided_review::ThreadAnswersId,
        /// Your own pull request's review
        review_id: ReviewId,
    },
    /// Replies to and resolves the threads the reader confirmed, exactly as confirmed. Which thread
    /// each comment is, and where it was said, is read from the session, not taken from the caller.
    AnswerThreads {
        /// Correlates the answer with the preview that asked
        request_id: crate::guided_review::ThreadAnswersId,
        /// Your own pull request's review
        review_id: ReviewId,
        /// One per thread, in the order shown
        answers: Vec<crate::guided_review::ThreadReply>,
    },
    /// Posts the open pull request review with the reader's verdict, planned from what
    /// `PreparePullRequestPost` read and the comments as the reader last saw them.
    ///
    /// Refused when the session moved past `session_revision`: what posts is what was shown.
    PostPullRequestReview {
        /// The preview being confirmed
        request_id: crate::guided_review::PullRequestPostId,
        /// Comment, approve, or request changes
        verdict: crate::github::ReviewEvent,
        /// The session revision the preview was drawn from
        session_revision: u64,
    },
    /// Request a fresh immutable Guided Review source capture.
    LoadGuidedReview {
        /// Correlates this request with its loading and result events.
        request_id: ReviewLoadRequestId,
        /// Target to capture from the local project.
        target: ReviewTarget,
    },
    /// Captures one file's change for a glance, outside any review.
    ///
    /// Deliberately not a review: nothing is pinned, persisted or chaptered, and no model is
    /// asked anything. A project-relative `path` is measured against the project's `HEAD`; an
    /// absolute one is a file the session touched somewhere else, measured against whatever
    /// witness that place has.
    LoadFileDiff {
        /// Correlates this request with its result event.
        request_id: crate::file_diff::FileDiffRequestId,
        /// Project-relative path, or an absolute path outside the project.
        path: String,
    },
    /// Lists one project folder for the FILES tree.
    ListDirectory {
        /// Correlates this request with its result event.
        request_id: crate::project_files::ListingRequestId,
        /// Project-relative folder; empty for the project root.
        path: String,
        /// List what `.gitignore` ignores too, marked as ignored.
        include_ignored: bool,
    },
    /// Reads one project file for the preview.
    ReadProjectFile {
        /// Correlates this request with its result event.
        request_id: crate::project_files::FileReadRequestId,
        /// Project-relative file.
        path: String,
    },
    /// Refreshes the open review only if it is still at `expected_revision`, keeping its identity
    /// and its comments.
    RefreshGuidedReview {
        /// Correlates this request with its loading and result events.
        request_id: ReviewLoadRequestId,
        /// The open review
        review_id: ReviewId,
        /// Optimistic guard for the visible source revision.
        expected_revision: ReviewRevisionId,
    },
    /// Changes the open review's feedback session: a comment, a reply, a send, and the rest.
    ///
    /// Applied by the review desk in the order it arrives, mid-turn included, and answered once:
    /// `ReviewSessionSaved` under `request_id` with the stored session (even when nothing changed),
    /// or `ReviewSessionSaveFailed` saying why it was refused. An edit for a review that is not the
    /// open one is refused.
    EditReview {
        /// Correlates the answer with the edit
        request_id: crate::guided_review::ReviewSessionSaveRequestId,
        /// The review it is for
        review_id: ReviewId,
        /// What to change
        edit: super::ReviewEdit,
    },
    /// Records which chapters of the open review have been walked.
    ///
    /// Saved only when it changed, and never answered: it follows the reader through the review,
    /// and a reply per step would be noise.
    RecordReviewProgress {
        /// The review it is for
        review_id: ReviewId,
        /// The walked chapters
        progress: crate::guided_review::ReviewProgress,
    },
    /// Remembers what the walked chapters of the open review covered, so the next review of its
    /// target leads with what is new.
    ///
    /// Fire-and-forget: the record is an optimisation for the reviewer, so a failure to write it is
    /// logged and never surfaced as a review error.
    RecordReviewSeen {
        /// The review it is for
        review_id: ReviewId,
    },
    /// Answers with the open review and its session, whoever opened it, as `OpenReviewShown`.
    ///
    /// How a second front end joins the review on screen rather than loading its own, which would
    /// replace it: a load always makes a new review.
    ShowOpenReview {
        /// Correlates the answer
        request_id: ReviewLoadRequestId,
    },
    /// Holds one stored review-feedback submission for its turn.
    ///
    /// Sent by the review desk once the submission is stored, never by a front end, which sends
    /// `EditReview` with a send instead.
    SubmitReviewFeedback {
        /// Unique persisted submission identity for idempotent dispatch.
        submission_id: crate::guided_review::SubmissionId,
        /// Logical review that owns the feedback.
        review_id: crate::guided_review::ReviewId,
        /// Pinned source the reviewer inspected.
        source: crate::guided_review::ReviewSource,
        /// Requested agent behavior.
        mode: crate::guided_review::ReviewFeedbackMode,
        /// Immutable finding snapshots to report.
        findings: Vec<crate::guided_review::SubmittedFinding>,
    },
    /// Drops a submission that has not crossed its turn boundary.
    ///
    /// Sent by the review desk once the cancellation is stored, never by a front end.
    CancelQueuedReviewFeedback {
        /// Immutable feedback identity to remove from the engine-owned queue.
        submission_id: crate::guided_review::SubmissionId,
    },
    /// Dispatches a previously accepted feedback submission at its turn boundary.
    ///
    /// This is internal engine choreography: callers submit or cancel feedback, but never send
    /// this action directly.
    DispatchQueuedReviewFeedback {
        /// Immutable feedback identity to dispatch if it remains queued.
        submission_id: crate::guided_review::SubmissionId,
    },
    /// Commits the open review's change, one commit per chapter or one squashed commit.
    ///
    /// Never pushes and never amends; refuses outright when the index is dirty, the worktree has
    /// moved past the reviewed revision, or the review was refreshed since it was shown.
    CommitReviewChapters {
        /// Correlates the outcome with the request that asked for it.
        request_id: crate::guided_review::ReviewSessionSaveRequestId,
        /// The open review
        review_id: ReviewId,
        /// The revision the reader saw
        expected_revision: ReviewRevisionId,
        /// Chapters to commit, or empty for all of them in Guide order.
        chapter_ids: Vec<crate::guided_review::ChapterId>,
        /// Collapse them into one commit instead of one per chapter.
        squash: bool,
    },
    /// Measure what the next request would carry, answering `/context`.
    ///
    /// Read-only: assembles the prompt and walks the transcript without sending anything, and
    /// without spending the one-time resume briefing.
    RequestContextComposition,
    /// Summarize the conversation now, whatever the occupancy — `/compact`.
    ///
    /// Args: `(focus)` — an optional steer for the summary, passed to the summarizer.
    CompactNow { focus: Option<String> },
    /// Resume the most recent session,
    ResumeLatest,
    /// Open a sub-picker (find/recent/grep sessions).
    OpenPicker(crate::dashboard::PickerKind),
    /// Edit the last user message and re-send (branches from that message's parent).
    EditResend { content: String },
    /// Re-send the last user message unchanged as a new branch.
    Retry,
    /// Re-run the assistant turn for the current last user message (new branch).
    Regenerate,
}

impl Action {
    /// Whether the review desk takes this as it comes, mid-turn or not: a send that never waits.
    pub(crate) fn goes_to_review_desk(&self) -> bool {
        matches!(
            self,
            Self::EditReview { .. }
                | Self::RecordReviewProgress { .. }
                | Self::RecordReviewSeen { .. }
                | Self::ShowOpenReview { .. }
        )
    }
}
