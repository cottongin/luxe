//! Action — the UI→core half of the protocol.

use super::ModelRef;
use crate::guided_review::{
    ReviewId, ReviewLoadRequestId, ReviewRevisionId, ReviewSessionSaveRequestId, ReviewTarget,
};
use crate::permission::PermissionRequestId;
use serde::Serialize;

mod classify;

/// UI→core intents — the published wire vocabulary an RPC front-end speaks.
///
/// Deliberately narrower than [`crate::engine::Action`]. That enum is an implementation detail: it
/// carries internal choreography a client must never drive, and payloads (a whole pinned
/// `GuidedReview`) no client can hold. This one is a contract, so it moves slowly and every variant
/// has to survive being written down in `docs/guide/rpc.md`.
///
/// `#[non_exhaustive]` plus the [`Action::Other`] fallback make additions non-breaking in both
/// directions — an unrecognized variant arrives with its payload preserved verbatim rather than
/// failing the parse.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "content", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Action {
    // === Conversation ===
    /// Submit a user message, with any images the front-end collected alongside it.
    SubmitUserMessage {
        /// Message text.
        text: String,
        /// Base64-encoded attachments for a vision model.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<crate::engine::ImageAttachment>,
    },
    /// Deliver `text` to the model at the next checkpoint between tool calls, without ending the
    /// turn or starting a new one. With no turn in flight it behaves as a plain submission.
    SteerTurn {
        /// Message text to deliver mid-turn.
        text: String,
    },
    /// Drop every steer admitted but not yet delivered, so the front-end can hand the text back to
    /// the user to edit. A steer the model already has stays delivered.
    WithdrawSteering,
    Cancel,
    /// Replace the last user message and re-send it, branching from that message's parent.
    EditResend {
        /// Replacement text for the last user message.
        text: String,
    },
    Retry,
    Regenerate,

    // === Prompts the agent is blocked on ===
    /// Answer a pending `permission_requested`, matched by `id`.
    RespondPermission {
        /// The prompt being answered.
        id: PermissionRequestId,
        /// The front-end's decision.
        reply: crate::permission::PermissionReply,
    },
    /// Answer a pending `question_asked`, matched by `id`.
    AnswerQuestion {
        /// The question being answered.
        id: crate::question::QuestionRequestId,
        /// Chosen options and free text, or a cancellation.
        reply: crate::question::QuestionReply,
    },
    /// Auto-allow permission `ask`s for the rest of the session. `deny` rules still apply.
    SetPermissionAutoApprove(bool),

    // === Model ===
    /// Switch the active model. The harness resolves the rest of the metadata from its catalog, so
    /// a client names a provider and an id and nothing else.
    SelectModel(ModelRef),
    /// Set the reasoning level; the turn clamps it to what the active model supports.
    SetThinkingLevel(crate::provider::ThinkingLevel),
    /// Switch the session's mode by name (`build`, `plan`). The mode's model and thinking level,
    /// when it has them, come from config.
    SetMode(String),
    /// Ask a reviewer in a fresh context to judge the uncommitted work, answered by
    /// `review_finished`. `focus` says what to look at hardest.
    RequestReview {
        /// What to look at hardest
        focus: Option<String>,
    },

    // === Session and workspace ===
    NewSession,
    /// Load a specific session by id.
    LoadSession(String),
    RefreshSessions,
    RefreshWorkspace,
    /// Measure what the next request would carry, answered by `context_composition`.
    RequestContextComposition,
    /// Summarize the conversation now, whatever the occupancy, answered by `compaction_done`.
    ///
    /// Args: `(focus)` — an optional steer for the summary.
    CompactNow {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        focus: Option<String>,
    },

    // === Guided Review ===
    /// Capture one immutable Guided Review source.
    LoadGuidedReview {
        /// Correlates loading, loaded, and failed events.
        request_id: ReviewLoadRequestId,
        /// Immutable source comparison to capture.
        target: ReviewTarget,
    },

    /// Change the open review's comments, answered by `review_session_saved` or
    /// `review_session_save_failed` under `request_id`. The change is flattened into the payload,
    /// tagged by `edit`.
    EditReview {
        /// Correlates the answer
        request_id: ReviewSessionSaveRequestId,
        /// The open review
        review_id: ReviewId,
        /// What to change
        #[serde(flatten)]
        edit: crate::engine::ReviewEdit,
    },
    /// Record which chapters of the open review have been read. Saved when it changed, never
    /// answered.
    RecordReviewProgress {
        /// The open review
        review_id: ReviewId,
        /// Every chapter walked so far
        walked_chapter_ids: Vec<crate::guided_review::ChapterId>,
    },
    /// Remember what the walked chapters covered, so the next review of the same target leads
    /// with what is new.
    RecordReviewSeen {
        /// The open review
        review_id: ReviewId,
    },
    /// Ask for the review open now, whoever opened it, answered by `open_review_shown` under
    /// `request_id`. How a second client joins a review instead of replacing it: loading one
    /// makes a new review, and the session holds one at a time.
    ShowOpenReview {
        /// Correlates the answer
        request_id: ReviewLoadRequestId,
    },
    /// Read the open review again, keeping its comments, if it is still at `revision_id`.
    RefreshGuidedReview {
        /// Correlates the loading, loaded and failed events
        request_id: ReviewLoadRequestId,
        /// The open review
        review_id: ReviewId,
        /// The revision on screen
        revision_id: ReviewRevisionId,
    },
    /// Commit the open review's change, a commit per chapter or one squashed, answered by
    /// `review_committed` or `review_commit_failed`. Never pushes; refused under read-only.
    CommitReviewChapters {
        /// Correlates the answer
        request_id: ReviewSessionSaveRequestId,
        /// The open review
        review_id: ReviewId,
        /// The revision on screen
        revision_id: ReviewRevisionId,
        /// Chapters to commit; absent or empty means all of them
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        chapter_ids: Vec<crate::guided_review::ChapterId>,
        /// One commit for all of it
        #[serde(default)]
        squash: bool,
    },
    /// Draft the pull request the open review would open, answered by `pull_request_prepared`.
    /// Luxe keeps the draft for `open_pull_request`.
    PreparePullRequest {
        /// Names the draft
        request_id: crate::guided_review::PullRequestOpenId,
        /// The open review
        review_id: ReviewId,
    },
    /// Open the drafted pull request with these words, answered by `pull_request_opened` or
    /// `pull_request_open_failed`. A GitHub write.
    OpenPullRequest {
        /// The draft being opened
        request_id: crate::guided_review::PullRequestOpenId,
        /// Its title
        title: String,
        /// Its description
        body: String,
        /// Open it as a draft
        #[serde(default)]
        draft: bool,
    },
    /// Draft the open review as a stack, one pull request per chapter, answered by
    /// `stack_prepared`. Luxe keeps the layers for `open_stack`.
    PrepareStack {
        /// Names the stack
        request_id: crate::guided_review::PullRequestOpenId,
        /// The open review
        review_id: ReviewId,
    },
    /// Open the drafted stack, answered by `stack_opened`. A GitHub write.
    OpenStack {
        /// The stack being opened
        request_id: crate::guided_review::PullRequestOpenId,
        /// Open every layer as a draft
        #[serde(default)]
        draft: bool,
    },
    /// Read what posting the open pull request review needs, answered by
    /// `pull_request_post_prepared`. Luxe keeps it for `post_pull_request_review`.
    PreparePullRequestPost {
        /// Names the preview
        request_id: crate::guided_review::PullRequestPostId,
        /// The open review
        review_id: ReviewId,
    },
    /// Post the open pull request review with a verdict, answered by `pull_request_review_posted`
    /// or `pull_request_post_failed`. A GitHub write.
    PostPullRequestReview {
        /// The preview being posted
        request_id: crate::guided_review::PullRequestPostId,
        /// `comment`, `approve` or `request_changes`
        verdict: crate::github::ReviewEvent,
        /// The session revision the preview was drawn from; refused if it moved
        session_revision: u64,
    },
    /// Read whether the fixes behind a thread answer are on GitHub yet, answered by
    /// `thread_answers_prepared`.
    PrepareThreadAnswers {
        /// Correlates the answer
        request_id: crate::guided_review::ThreadAnswersId,
        /// The open review of your own pull request
        review_id: ReviewId,
    },
    /// Reply to and resolve threads on your own pull request, answered by `threads_answered`. A
    /// GitHub write.
    AnswerThreads {
        /// Correlates the answer
        request_id: crate::guided_review::ThreadAnswersId,
        /// The open review of your own pull request
        review_id: ReviewId,
        /// One per thread, named by the comment it became
        answers: Vec<crate::guided_review::ThreadReply>,
    },

    // === GitHub ===
    /// Run `/github`'s searches, answered together by `github_listed`.
    ///
    /// Args: `(sections)` — each a name and a GitHub search, `{repo}` standing for the checkout's
    /// repository; absent means the user's configured sections.
    ListGithub {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sections: Option<Vec<crate::config::GithubSection>>,
    },
    /// Run one search on its own, or its next page, answered by `github_searched`.
    SearchGithub {
        /// The client's id for it, echoed in the answer
        request: u64,
        /// The search, `{repo}` still in its query
        section: crate::config::GithubSection,
        /// The cursor the page starts after; absent for the first page
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after: Option<String>,
    },
    /// Read one pull request's or issue's overview, answered by `github_overview_read`.
    ReadGithubOverview {
        /// `owner/name`, when it is not the checkout's repository
        #[serde(default, skip_serializing_if = "Option::is_none")]
        repository: Option<String>,
        /// Its number
        number: u64,
        /// Whether it is a pull request, rather than an issue
        pull_request: bool,
    },
    /// Read everything said on a pull request or issue, answered by `github_conversation_read`.
    ReadGithubConversation {
        /// `owner/name`, when it is not the checkout's repository
        #[serde(default, skip_serializing_if = "Option::is_none")]
        repository: Option<String>,
        /// Its number
        number: u64,
        /// Whether it is a pull request, rather than an issue
        pull_request: bool,
    },
    /// Have the agent draft a reply in a turn that changes nothing, answered by
    /// `github_reply_drafted`.
    DraftGithubReply(crate::engine::DraftRequest),
    /// Ask whether a pull request can be checked out here to fix, answered by
    /// `pull_request_fix_checked`.
    CheckPullRequestFix {
        /// Its number
        number: u64,
        /// `owner/name`, when it is not the checkout's repository
        #[serde(default, skip_serializing_if = "Option::is_none")]
        repository: Option<String>,
    },
    /// Work out what restacking the checked-out branch's stack would do, answered by
    /// `restack_planned`. Luxe keeps the plan for the `restack` that confirms it.
    PlanRestack {
        /// Names the plan
        request_id: crate::restack::RestackId,
    },
    /// Ask what in the session's worktree is not committed, answered by
    /// `worktree_removal_checked`.
    CheckWorktreeRemoval,
    /// Capture one file's change, answered by `file_diff_loaded` or `file_diff_failed`.
    LoadFileDiff {
        /// Correlates the answer
        request_id: crate::file_diff::FileDiffRequestId,
        /// Project-relative, or an absolute path the session touched outside the project
        path: String,
    },
    /// Ask which models this client can switch to, answered by `models_listed` to this client
    /// alone.
    ListModels {
        /// Correlates the answer
        request_id: String,
    },
    /// List one project folder, answered by `directory_listed` or `directory_list_failed`.
    ListDirectory {
        /// Correlates the answer
        request_id: crate::project_files::ListingRequestId,
        /// Project-relative folder; empty for the project root
        path: String,
        /// List what `.gitignore` ignores too, each entry marked; off when absent
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        include_ignored: bool,
    },
    /// Read one project file, answered by `project_file_read` or `project_file_read_failed`.
    ReadProjectFile {
        /// Correlates the answer
        request_id: crate::project_files::FileReadRequestId,
        /// Project-relative file
        path: String,
    },
    /// Whether the client's window has focus, which paces Luxe's background GitHub refreshes.
    SetFocus {
        /// Whether it now has focus
        focused: bool,
    },
    /// Record that this session worked on a pull request.
    LinkPullRequest {
        /// Its number
        number: u64,
        /// Its web address
        url: String,
    },
    /// Post a reply or comment, answered by `github_comment_posted`. A GitHub write.
    PostGithubComment(crate::engine::GithubPost),
    /// Update, mark ready, merge, or set auto-merge on a pull request, answered by
    /// `pull_request_changed`. A GitHub write.
    ChangePullRequest {
        /// `owner/name`, when it is not the checkout's repository
        #[serde(default, skip_serializing_if = "Option::is_none")]
        repository: Option<String>,
        /// Its number
        number: u64,
        /// What to change
        write: crate::engine::PullRequestWrite,
    },
    /// Push the plan `plan_restack` made under `request_id`, answered by `restacked` or
    /// `restack_failed`. A GitHub write.
    Restack {
        /// The plan that was shown
        request_id: crate::restack::RestackId,
    },

    // === Lifecycle ===
    /// A client that joined a running session names itself, so the interface can say who joined
    /// and who answered a prompt. Refused from the client that started the session.
    ///
    /// Args: `name` — how to call it, such as the device the bridge relays for. Cosmetic: any
    /// program running as you could send it. `watch_only` — the client only reads, so the
    /// interface doesn't say it joined, and nothing it sends afterward reaches the engine. Honored
    /// only as the client's first line, so a client can't act and then go quiet.
    Introduce {
        /// How to call the client
        name: String,
        /// Reads without being announced, and can't act
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        watch_only: bool,
        /// Paired by the person at the terminal, who needn't be told their own device joined
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        yours: bool,
    },
    /// Shut the engine down, stopping a turn in flight the way `cancel` does. The event stream
    /// ends after it.
    Close,

    /// Catch-all for variants this build does not know about.
    Other {
        kind: String,
        content: serde_json::Value,
    },
}
