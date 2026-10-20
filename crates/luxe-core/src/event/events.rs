//! Event — the core→UI half of the protocol.

use super::{EngineState, GuidedReviewFailure, RequestedReviewVerdict, SessionMessage, StopReason};
use crate::activity::ToolActivity;
use crate::guided_review::{
    GuideProvenance, GuidedReview, RemediationReport, ReviewChapter, ReviewId, ReviewLoadRequestId,
    ReviewRevisionId, ReviewSession, ReviewSessionSaveRequestId, ReviewTarget, SubmissionId,
};
use crate::message::Message;
use crate::permission::PermissionRequestId;
use crate::session::EntryId;
use crate::workspace::WorkspaceSnapshot;
use serde::Serialize;

/// core→UI stream. Unknown variants deserialize into [`Event::Other`] and are preserved verbatim.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", content = "content", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Event {
    /// A token fragment of the next assistant text block (block index is for ordering).
    TokenDelta { block: usize, delta: String },
    /// A reasoning/thinking token fragment.
    ThinkingDelta { block: usize, delta: String },
    /// The model has begun writing a tool call, well before it can be dispatched.
    ///
    /// A `write`'s content *is* its argument, so the model spends far longer composing that call
    /// than the tool spends running it. Without this the transcript has nothing to show for that
    /// stretch, and the card lands at the same moment the work finishes — which reads as a tool
    /// that never ran. [`Event::ToolStarted`] follows with the same `call_id` once it is
    /// dispatched, and every call that composes also ends, so a front-end may open a row here and
    /// settle it there.
    ///
    /// `subject` is best effort: absent on the first frame, then sent once more as soon as the
    /// arguments that have landed name what the call is about (`describe_partial_call`). A call
    /// whose bulky argument arrives first is never named and simply stays a spinner.
    ///
    /// `body` is the payload as far as it has been written, for the tools whose payload is the
    /// point (`partial_card_body`), and is resent as it grows — rate-limited, not per fragment.
    /// Its `total_lines` counts what has landed, so a card's `+N more lines` climbs while the file
    /// is written. Every field is a snapshot rather than a delta, so a client may drop any frame.
    ToolCallComposing {
        call_id: String,
        name: String,
        subject: Option<String>,
        body: Option<crate::activity::ToolCardBody>,
    },
    /// Tool invocation started (name + id known before execution).
    ToolStarted { call_id: String, name: String },
    /// Streaming progress from a running tool.
    ToolProgress { call_id: String, chunk: String },
    /// A call finished running, with what it returned, before its message's results commit.
    ///
    /// A call can run while the model is still writing the next one, so its result exists well
    /// before the batch's results are committed and `ToolEnded` follows. This is what lets a card
    /// settle the moment its own call does instead of when the last of the batch does. Optional for
    /// a client: one that ignores it settles the card on `ToolEnded`. `output` is the result's
    /// text, already redacted. Args: `{call_id, is_error, output}`.
    ToolCompleted { call_id: String, is_error: bool, output: String },
    /// Tool finished (success or error).
    ToolEnded { call_id: String, is_error: bool },
    /// A full message, from any role, was committed to the session.
    MessageCommitted { session_id: String, entry_id: EntryId, message: Message },
    /// A validated remediation report was extracted after its assistant transcript entry committed.
    RemediationReported {
        /// Persisted assistant entry that carried the named report block.
        entry_id: EntryId,
        /// Source-pinned result data safe to apply to matching review feedback.
        report: RemediationReport,
    },
    /// A queued feedback item crossed the engine boundary and can no longer be cancelled.
    ReviewFeedbackTurnDispatched { submission_id: SubmissionId },
    /// A feedback turn failed before the engine could produce a terminal agent response.
    ReviewFeedbackTurnFailed {
        /// Persisted feedback submission whose turn failed.
        submission_id: SubmissionId,
        /// Display-safe, bounded failure detail.
        message: String,
    },
    /// A question's turn answered, just before it completes: the answer belongs on the
    /// questions it was asked about, not only in the transcript.
    ReviewQuestionsAnswered {
        /// Persisted feedback submission that asked
        submission_id: SubmissionId,
        /// The turn's final reply
        answer: String,
    },
    /// A feedback turn reached its terminal assistant response and may be marked complete.
    ReviewFeedbackTurnCompleted { submission_id: SubmissionId },
    /// A resumed session was loaded with its active-path transcript.
    SessionLoaded {
        session_id: String,
        /// Active-path messages paired with their persisted entry IDs.
        messages: Vec<SessionMessage>,
    },
    /// Engine state snapshot changed.
    StateChanged(EngineState),
    /// A turn began, whichever client asked for it. Sent before anything else of the turn, the
    /// user's message included, and always followed by `TurnEnded`, so a client that didn't start
    /// the turn needn't guess from its first sign.
    TurnStarted,
    /// The in-flight turn ended.
    TurnEnded { reason: StopReason },
    /// The models this client can switch to, configured ones first, each with the thinking levels
    /// it honors, answering the `list_models` with this `request_id`.
    ModelsListed { request_id: String, models: Vec<super::ModelChoice> },
    /// A complete retained baseline is available for the latest agent turn.
    LastTurnAvailable {
        /// Session which owns the persisted baseline.
        session_id: String,
        /// Opaque turn key accepted by `ReviewTarget::LastTurn`.
        turn_id: String,
    },
    /// A summary began, which can take a minute; `CompactionDone` or `CompactionStopped` ends it.
    CompactionStarted { tokens_before: u64 },
    /// Compaction completed.
    CompactionDone { tokens_before: u64, tokens_after: u64 },
    /// A started compaction failed or was cancelled; a failure's reason comes as a `Notice` or `Error`.
    CompactionStopped,
    /// Token usage after a provider call — lets UIs show context-window fullness.
    ContextUsage { used_tokens: u32, context_window: u32 },
    /// Token counts + cost for **one provider call**, as it completes.
    ///
    /// Per call, not per turn: a turn that calls tools makes many, each reporting its own usage.
    /// For a running total read [`Event::SessionUsage`], which the engine already summed.
    TurnUsage {
        prompt_tokens: u64,
        completion_tokens: u64,
        cost_usd: Option<f64>,
        /// Cached input tokens read (subset of `prompt_tokens`).
        #[serde(default)]
        cache_read_tokens: u64,
        /// Input tokens written to cache (subset of `prompt_tokens`).
        #[serde(default)]
        cache_write_tokens: u64,
        /// Reasoning tokens (subset of `completion_tokens`).
        #[serde(default)]
        reasoning_tokens: u64,
        /// Which part of the harness made the call.
        #[serde(default)]
        source: crate::usage::UsageSource,
    },
    /// The session's running spend after the call [`Event::TurnUsage`] just reported.
    ///
    /// The authoritative figure: totals plus the per-source breakdown behind them. UIs render it
    /// and perform no accounting of their own.
    SessionUsage(crate::usage::SessionUsage),
    /// How much of a subscription's allowance is left, as the provider reported it on the last
    /// call. Absent for API-key providers, which meter money rather than a plan.
    PlanUsage(crate::provider::PlanUsage),
    /// What the next request would carry, broken into parts — the answer to `/context`.
    ///
    /// Boxed: it dwarfs every other variant, and this enum is moved through a channel on every
    /// streamed token.
    ContextComposition(Box<crate::context_report::ContextComposition>),
    /// An image was attached to the current message (clipboard paste or file drop).
    ImageAttached { mime: String, width: u32, height: u32 },
    /// A non-recoverable error occurred.
    Error(String),
    /// An informational message for the user. Unlike [`Event::Error`] it does not end the turn.
    Notice(String),
    /// Who's joined this session now, by name, for the terminal to show; sent as one comes or goes.
    GuestsChanged(Vec<String>),
    /// Plan mode wrote one of its plan files. Args: `path`, project-relative.
    ///
    /// What a front-end offers the plan's next step on, once the turn that wrote it ends.
    PlanWritten { path: String },
    /// A requested review started, on `model`, so a front-end can say one is running.
    ReviewStarted { model: String },
    /// A requested review came back. Args: `verdict`, and `findings` — each one a reviewer would
    /// fix, or the reason none came back.
    ReviewFinished { verdict: RequestedReviewVerdict, findings: Vec<String> },
    /// Steering messages buffered during the turn were delivered to the model at a checkpoint.
    /// Args: `count` — how many, in admission order, so a UI can clear their pending markers.
    SteeringDelivered { count: usize },
    /// The provider could not answer just now, before saying anything, and the same request goes
    /// out again after a pause. Args: `attempt` of `attempts`, and `reason`, the provider's words.
    ProviderRetrying { attempt: u32, attempts: u32, reason: String },
    /// A requested immutable Guided Review source started loading.
    GuidedReviewLoading {
        /// Correlates this state with the request that caused it.
        request_id: ReviewLoadRequestId,
        /// Target currently being captured.
        target: ReviewTarget,
    },
    /// A requested immutable Guided Review source was captured and assembled.
    GuidedReviewLoaded {
        /// Correlates this review with the request that produced it.
        request_id: ReviewLoadRequestId,
        /// Complete review derived only from its pinned source artifact.
        review: Box<GuidedReview>,
    },
    /// A richer Guide replaced the structural chapters a review opened with.
    ///
    /// Chaptering is advisory and may take several model passes, so the review opens at once with
    /// deterministic sections and upgrades in place only while its pinned revision is on screen.
    GuidedReviewGuideUpdated {
        /// Correlates this Guide with the request whose review it describes.
        request_id: ReviewLoadRequestId,
        /// Logical review the chapters belong to.
        review_id: ReviewId,
        /// Exact pinned revision the chapters were computed against.
        revision_id: ReviewRevisionId,
        /// Replacement chapters, already validated to cover every source item once.
        chapters: Vec<ReviewChapter>,
        /// How those chapters were produced.
        provenance: GuideProvenance,
        /// The reviewer-facing summary from the same pass, when it produced one.
        #[serde(default)]
        brief: Option<crate::guided_review::ReviewBrief>,
    },
    /// What a pull request's review needs before it is posted.
    PullRequestPostPrepared {
        /// The attempt this answers
        request_id: crate::guided_review::PullRequestPostId,
        /// GitHub's patches, which decide where each comment can land
        patches: Vec<crate::github::FilePatch>,
        /// Whether the author pushed after the review was pinned
        head_moved: bool,
        /// Whether the reader opened the pull request, which rules out approving it
        own_pull_request: bool,
    },
    /// A pull request's review landed on GitHub.
    PullRequestReviewPosted {
        /// The attempt this answers
        request_id: crate::guided_review::PullRequestPostId,
        /// GitHub's id for the review
        github_review_id: u64,
        /// Where it can be read
        url: String,
        /// The comments it carried
        finding_ids: Vec<crate::guided_review::FindingId>,
    },
    /// Preparing or posting a pull request's review failed; nothing was posted.
    PullRequestPostFailed {
        /// The attempt this answers
        request_id: crate::guided_review::PullRequestPostId,
        /// What went wrong, as a sentence
        message: String,
    },
    /// Whether the fixes behind a thread answer are on GitHub yet.
    ThreadAnswersPrepared {
        /// The attempt this answers
        request_id: crate::guided_review::ThreadAnswersId,
        /// Whether the checkout's `HEAD` is the pull request's head on GitHub
        pushed: bool,
        /// Whether the working tree has changes not committed
        uncommitted: bool,
    },
    /// What answering each thread did.
    ThreadsAnswered {
        /// The attempt this answers
        request_id: crate::guided_review::ThreadAnswersId,
        /// One per thread asked about, in order
        results: Vec<crate::guided_review::ThreadAnswerResult>,
    },
    /// Preparing or sending thread answers failed before any thread was touched.
    ThreadAnswersFailed {
        /// The attempt this answers
        request_id: crate::guided_review::ThreadAnswersId,
        /// What went wrong, as a sentence
        message: String,
    },
    /// What `/github`'s sections found.
    GithubListed {
        /// The checkout's repository, `owner/name`, which a row elsewhere is told apart from
        repository: Option<String>,
        /// Each section, in the order asked, with its rows or its reason
        sections: Vec<crate::github::ListedSection>,
    },
    /// One search `/github` ran on its own: a tab's edited query, or the next page of one.
    GithubSearched {
        /// The front-end's id for the search, as asked
        request: u64,
        /// The checkout's repository, `owner/name`, which a row elsewhere is told apart from
        repository: Option<String>,
        /// What it found, or why nothing
        section: crate::github::ListedSection,
    },
    /// One pull request's or issue's overview, or why it could not be read.
    GithubOverviewRead {
        /// `owner/name` as asked, `None` for the checkout's repository
        repository: Option<String>,
        /// The number asked about
        number: u64,
        /// What GitHub said about it
        overview: Option<crate::github::overview::Overview>,
        /// Why it could not be read, as a sentence
        error: Option<String>,
        /// Your latest other session on it, for an issue on the checkout's repository that has one
        #[serde(default, skip_serializing_if = "Option::is_none")]
        local: Option<crate::github::overview::LocalSession>,
    },
    /// A reply the agent drafted for the reader to read, change and send, or why there is none.
    GithubReplyDrafted {
        /// The request it answers
        request_id: String,
        /// The draft, markdown
        body: Option<String>,
        /// What it was drafted from, as a person says it: `this session`, `the thread`
        sources: Vec<String>,
        /// Why there is no draft, as a sentence
        error: Option<String>,
        /// What the agent said beside the draft — a caveat to read before sending it
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    /// A change the reader made to a pull request landed, or why it did not.
    PullRequestChanged {
        /// `owner/name` as asked, `None` for the checkout's repository
        repository: Option<String>,
        /// The pull request changed
        number: u64,
        /// What changed
        change: Option<crate::engine::PullRequestChange>,
        /// Why nothing did, as a sentence
        error: Option<String>,
    },
    /// A reply or comment the reader posted landed, or why it did not.
    GithubCommentPosted {
        /// `owner/name` as asked, `None` for the checkout's repository
        repository: Option<String>,
        /// The pull request or issue it was on
        number: u64,
        /// The thread it replied in, when it answered one
        thread: Option<String>,
        /// Where it can be read, once it landed
        url: Option<String>,
        /// Why it did not land, as a sentence
        error: Option<String>,
    },
    /// Everything said about a pull request or an issue, or why it could not be read.
    GithubConversationRead {
        /// `owner/name` as asked, `None` for the checkout's repository
        repository: Option<String>,
        /// The number asked about
        number: u64,
        /// Its conversation, oldest first
        entries: Option<Vec<crate::github::overview::ConversationEntry>>,
        /// Why it could not be read, as a sentence
        error: Option<String>,
    },
    /// A branch's review drafted as a stack.
    StackPrepared {
        /// The attempt this answers
        request_id: crate::guided_review::PullRequestOpenId,
        /// The pull requests, bottom first, each on the branch of the one below
        layers: Option<Vec<crate::github::NewPullRequest>>,
        /// Why it cannot be a stack, as a sentence
        refusal: Option<String>,
    },
    /// A stack opened from a review, all of it or as far as it got.
    StackOpened {
        /// The attempt this answers
        request_id: crate::guided_review::PullRequestOpenId,
        /// The pull requests that opened, bottom first
        numbers: Vec<u64>,
        /// Where each can be read, in the same order
        urls: Vec<String>,
        /// What stopped the rest, as a sentence
        failed: Option<String>,
    },
    /// What restacking the checked-out branch's stack would do.
    RestackPlanned {
        /// Names the plan, for the `restack` that confirms it
        request_id: crate::restack::RestackId,
        /// The plan; its steps are empty when the stack already sits right
        plan: Option<crate::restack::RestackPlan>,
        /// The pull request that merged out from under the stack's bottom, when one did
        merged: Option<crate::restack::MergedBelow>,
        /// Why there is no plan, as a sentence
        refusal: Option<String>,
    },
    /// A restack was pushed.
    Restacked {
        /// The plan that was pushed
        request_id: crate::restack::RestackId,
        /// Pull requests whose branch moved
        moved: Vec<u64>,
        /// Pull requests moved to a new base
        retargeted: Vec<u64>,
        /// A base that could not be moved, after the push, as a sentence
        retarget_failed: Option<String>,
    },
    /// A restack failed before anything was pushed.
    RestackFailed {
        /// The plan the push named
        request_id: crate::restack::RestackId,
        /// What went wrong, as a sentence
        message: String,
    },
    /// Whether a pull request can be checked out here to fix.
    PullRequestFixChecked {
        /// Number within its repository
        number: u64,
        /// The local branch it would be checked out on, when it can be
        branch: Option<String>,
        /// Why it cannot, as a sentence
        refusal: Option<String>,
    },
    /// The pull request a branch's review would open, drafted for the reader to edit.
    PullRequestPrepared {
        /// The attempt this answers
        request_id: crate::guided_review::PullRequestOpenId,
        /// The draft: head, base, title and description
        pull_request: crate::github::NewPullRequest,
        /// Whether the branch is on GitHub yet; opening pushes it when it is not
        publication: crate::github::Publication,
        /// Paths with changes not committed
        uncommitted: usize,
        /// Subjects of the commits opening writes first, when the reviewed work is not committed
        commits: Vec<String>,
    },
    /// A branch's pull request opened on GitHub.
    PullRequestOpened {
        /// The attempt this answers
        request_id: crate::guided_review::PullRequestOpenId,
        /// Its number
        number: u64,
        /// Where it can be read
        url: String,
        /// Commits written before it opened
        committed: Vec<String>,
    },
    /// Drafting or opening a pull request failed; nothing was opened.
    PullRequestOpenFailed {
        /// The attempt this answers
        request_id: crate::guided_review::PullRequestOpenId,
        /// What went wrong, as a sentence
        message: String,
    },
    /// What in the session's worktree is not committed, asked just before removing it.
    WorktreeRemovalChecked {
        /// Paths with changes not committed, as git names them; empty when removing loses nothing
        uncommitted: Vec<String>,
    },
    /// The reviewed change reached the log.
    ReviewCommitted {
        /// Correlates this outcome with the request that asked for it.
        request_id: ReviewSessionSaveRequestId,
        /// New commit object IDs, oldest first.
        commits: Vec<String>,
        /// Reviewed items no patch could carry, named so nothing vanishes silently.
        skipped: Vec<String>,
    },
    /// The reviewed change could not be committed, and nothing was changed.
    ReviewCommitFailed {
        /// Correlates this failure with the request that asked for it.
        request_id: ReviewSessionSaveRequestId,
        /// Reviewer-facing explanation.
        message: String,
    },
    /// A requested immutable Guided Review source could not be loaded.
    GuidedReviewFailed {
        /// Correlates this failure with the request that produced it.
        request_id: ReviewLoadRequestId,
        /// Target whose immutable source was unavailable.
        target: ReviewTarget,
        /// Typed, bounded failure safe to render or serialize.
        error: GuidedReviewFailure,
    },
    /// One file's change was captured for a glance, outside any review.
    FileDiffLoaded {
        /// Correlates this capture with the request that asked for it.
        request_id: crate::file_diff::FileDiffRequestId,
        /// The file's change, or an explicit absence when it matches its base.
        diff: Box<crate::file_diff::FileDiff>,
    },
    /// One file's change could not be captured.
    FileDiffFailed {
        /// Correlates this failure with the request that asked for it.
        request_id: crate::file_diff::FileDiffRequestId,
        /// Path whose change was asked for.
        path: String,
        /// Typed, bounded failure safe to render or serialize.
        error: crate::file_diff::FileDiffFailure,
    },
    /// One project folder was listed.
    DirectoryListed {
        /// Correlates this listing with the request that asked for it.
        request_id: crate::project_files::ListingRequestId,
        /// The folder's entries.
        listing: crate::project_files::DirectoryListing,
    },
    /// One project folder could not be listed.
    DirectoryListFailed {
        /// Correlates this failure with the request that asked for it.
        request_id: crate::project_files::ListingRequestId,
        /// Folder that was asked for.
        path: String,
        /// Why not.
        error: crate::project_files::ProjectFileFailure,
    },
    /// One project file was read for a preview.
    ProjectFileRead {
        /// Correlates this read with the request that asked for it.
        request_id: crate::project_files::FileReadRequestId,
        /// The file's text, or the fact that it has none.
        file: Box<crate::project_files::ProjectFile>,
    },
    /// One project file could not be read.
    ProjectFileReadFailed {
        /// Correlates this failure with the request that asked for it.
        request_id: crate::project_files::FileReadRequestId,
        /// File that was asked for.
        path: String,
        /// Why not.
        error: crate::project_files::ProjectFileFailure,
    },
    /// The session bound to the review just loaded, always after [`Event::GuidedReviewLoaded`]:
    /// carried over a refresh onto its new source, restored after a restart, or new. Also sent when
    /// another writer's copy replaced this one.
    ReviewSessionRestored {
        /// Complete authoritative state, as the review desk holds it.
        session: Box<ReviewSession>,
    },
    /// The answer to `show_open_review`. Boxed: it carries a whole review.
    OpenReviewShown(Box<super::types::OpenReview>),
    /// The open review's feedback session changed and was stored. Every change arrives this way,
    /// whoever made it: an `EditReview`, answered under its `request_id`, or something Luxe did
    /// itself (a turn dispatched, a report recorded, a review posted), under a fresh one.
    ReviewSessionSaved {
        /// The edit this answers, or a fresh id for a change Luxe made itself.
        request_id: ReviewSessionSaveRequestId,
        /// Complete authoritative state returned by the expected-revision store.
        session: Box<ReviewSession>,
    },
    /// A change to the feedback session was refused or could not be stored, and nothing changed.
    ReviewSessionSaveFailed {
        /// The edit this answers, or a fresh id for a change Luxe made itself.
        request_id: ReviewSessionSaveRequestId,
        /// Logical review whose mutable state was not persisted.
        review_id: ReviewId,
        /// Concise recovery guidance safe to show in the UI.
        message: String,
    },
    /// Session list was refreshed.
    SessionsRefreshed(Vec<String>),
    /// Project-local workspace refresh was explicitly requested.
    WorkspaceRefreshRequested,
    /// The terminal gained or lost focus.
    ///
    /// The workspace runtime paces its background GitHub refreshes on it: a terminal nobody has
    /// looked at for a while is checked less often, and one coming back is checked at once.
    TerminalFocusChanged { focused: bool },
    /// Project-local workspace summary changed.
    WorkspaceUpdated(Box<WorkspaceSnapshot>),
    /// Structured display state for a tool call changed.
    ///
    /// Boxed like [`Event::WorkspaceUpdated`]: it is far and away the largest payload here, and every
    /// event on the channel — a token delta, a spinner tick — pays for the biggest variant.
    ToolActivityUpdated(Box<ToolActivity>),
    /// A tool call needs the user's approval (permission `ask`). The front-end shows a prompt and
    /// replies with `Action::RespondPermission` carrying the same `id`.
    PermissionRequested {
        /// Correlates this prompt with its `RespondPermission` reply.
        id: PermissionRequestId,
        /// Session that owns the awaiting turn.
        session_id: String,
        /// Tool being invoked (e.g. `bash`, `edit`).
        tool: String,
        /// Permission key the decision was keyed on (e.g. `bash`, `external_directory`).
        permission: String,
        /// One-line, human-readable description of the action (e.g. `Run: rm -rf build`).
        summary: String,
        /// Patterns an "always" reply would approve for the session (for display).
        always_patterns: Vec<String>,
    },
    /// The interactive `question` tool needs the user's answer. The front-end shows a modal and
    /// replies with `Action::AnswerQuestion` carrying the same `id`.
    QuestionAsked {
        /// Correlates this prompt with its `AnswerQuestion` reply.
        id: crate::question::QuestionRequestId,
        /// Session that owns the awaiting turn.
        session_id: String,
        /// The question to put to the user.
        prompt: String,
        /// Selectable options with their rationale (may be empty for a free-text-only question).
        options: Vec<crate::question::QuestionOption>,
        /// Whether multiple options may be selected.
        multi: bool,
    },
    /// A prompt stopped waiting, answered by any front-end or cancelled; the others take it down.
    PromptClosed { id: String },
    /// First frame of an RPC stream, from `luxe --rpc` or a joined session: protocol and session.
    /// Additive changes do not move `protocol_version`; removing or retagging anything does.
    Ready {
        /// Wire protocol version — [`crate::PROTOCOL_VERSION`].
        protocol_version: u32,
        /// The Luxe build serving this session.
        luxe_version: String,
        /// Session the stream is attached to.
        session_id: String,
        /// Project root the agent was opened in.
        cwd: String,
        /// Whether this session accepts actions that publish to GitHub (`--github-writes`), so a
        /// client can hide what would only be refused.
        #[serde(default)]
        github_writes: bool,
    },
    /// An inbound frame was refused and had no effect.
    ///
    /// Recoverable by construction, unlike [`Event::Error`] — one malformed line costs a client
    /// that line, not its session.
    ActionRejected {
        /// The refused `kind`, or absent when the line never parsed far enough to have one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<String>,
        /// What was wrong with it.
        reason: String,
    },
    /// Catch-all for future variants the builder didn't know about.
    Other { kind: String, content: serde_json::Value },
}
