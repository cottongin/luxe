//! Samples — one of every `Event` variant, and the table of tags they publish.

use super::*;
use crate::guided_review::{
    CheckSummary, ReviewBase, ReviewContext, ReviewSessionSaveRequestId, ReviewSource, ReviewStats,
    SubmissionId,
};

/// One table makes both [`tag_of`] and [`EVENT_TAGS`], so the row the compiler demands for a new
/// variant publishes its tag too, and the completeness test then asks for its sample and its arm.
macro_rules! published {
    ($($variant:ident => $tag:literal,)*) => {
        /// The tag every `Event` variant serializes as. A `match`, so it is the compiler that keeps
        /// it complete: `#[non_exhaustive]` binds other crates, not this one.
        pub(super) fn tag_of(event: &Event) -> &'static str {
            match event {
                $(Event::$variant { .. } => $tag,)*
                Event::Other { .. } => "other",
            }
        }

        /// The tags a client may receive, one per variant but `Other`.
        pub(super) const EVENT_TAGS: &[&str] = &[$($tag,)*];
    };
}

published! {
    TokenDelta => "token_delta",
    ThinkingDelta => "thinking_delta",
    ToolCallComposing => "tool_call_composing",
    ToolStarted => "tool_started",
    ToolProgress => "tool_progress",
    ToolCompleted => "tool_completed",
    ToolEnded => "tool_ended",
    MessageCommitted => "message_committed",
    RemediationReported => "remediation_reported",
    ReviewFeedbackTurnDispatched => "review_feedback_turn_dispatched",
    ReviewFeedbackTurnFailed => "review_feedback_turn_failed",
    ReviewQuestionsAnswered => "review_questions_answered",
    ReviewFeedbackTurnCompleted => "review_feedback_turn_completed",
    SessionLoaded => "session_loaded",
    StateChanged => "state_changed",
    TurnEnded => "turn_ended",
    LastTurnAvailable => "last_turn_available",
    CompactionStarted => "compaction_started",
    CompactionDone => "compaction_done",
    CompactionStopped => "compaction_stopped",
    ContextUsage => "context_usage",
    TurnUsage => "turn_usage",
    SessionUsage => "session_usage",
    PlanUsage => "plan_usage",
    ContextComposition => "context_composition",
    ImageAttached => "image_attached",
    Error => "error",
    Notice => "notice",
    PlanWritten => "plan_written",
    ReviewStarted => "review_started",
    ReviewFinished => "review_finished",
    SteeringDelivered => "steering_delivered",
    ProviderRetrying => "provider_retrying",
    GuidedReviewLoading => "guided_review_loading",
    GuidedReviewLoaded => "guided_review_loaded",
    GuidedReviewGuideUpdated => "guided_review_guide_updated",
    PullRequestPostPrepared => "pull_request_post_prepared",
    PullRequestReviewPosted => "pull_request_review_posted",
    PullRequestPostFailed => "pull_request_post_failed",
    ThreadAnswersPrepared => "thread_answers_prepared",
    ThreadsAnswered => "threads_answered",
    ThreadAnswersFailed => "thread_answers_failed",
    GithubListed => "github_listed",
    GithubSearched => "github_searched",
    GithubOverviewRead => "github_overview_read",
    GithubConversationRead => "github_conversation_read",
    GithubCommentPosted => "github_comment_posted",
    PullRequestChanged => "pull_request_changed",
    GithubReplyDrafted => "github_reply_drafted",
    StackPrepared => "stack_prepared",
    StackOpened => "stack_opened",
    RestackPlanned => "restack_planned",
    Restacked => "restacked",
    RestackFailed => "restack_failed",
    PullRequestFixChecked => "pull_request_fix_checked",
    PullRequestPrepared => "pull_request_prepared",
    PullRequestOpened => "pull_request_opened",
    PullRequestOpenFailed => "pull_request_open_failed",
    WorktreeRemovalChecked => "worktree_removal_checked",
    ReviewCommitted => "review_committed",
    ReviewCommitFailed => "review_commit_failed",
    GuidedReviewFailed => "guided_review_failed",
    FileDiffLoaded => "file_diff_loaded",
    FileDiffFailed => "file_diff_failed",
    DirectoryListed => "directory_listed",
    DirectoryListFailed => "directory_list_failed",
    ProjectFileRead => "project_file_read",
    ProjectFileReadFailed => "project_file_read_failed",
    ReviewSessionRestored => "review_session_restored",
    ReviewSessionSaved => "review_session_saved",
    ReviewSessionSaveFailed => "review_session_save_failed",
    SessionsRefreshed => "sessions_refreshed",
    WorkspaceRefreshRequested => "workspace_refresh_requested",
    WorkspaceUpdated => "workspace_updated",
    TerminalFocusChanged => "terminal_focus_changed",
    ToolActivityUpdated => "tool_activity_updated",
    PermissionRequested => "permission_requested",
    QuestionAsked => "question_asked",
    Ready => "ready",
    ActionRejected => "action_rejected",
}

/// An id read from its wire form, so a sample needs no constructor.
fn id<T: for<'de> Deserialize<'de>>(literal: &str) -> T {
    serde_json::from_value(serde_json::json!(literal)).expect("ids are wire strings")
}

fn review() -> GuidedReview {
    let revision_id = ReviewRevisionId::from_canonical_bytes(b"");
    GuidedReview {
        conversation: None,
        id: id("review:1"),
        revision_id: revision_id.clone(),
        target: ReviewTarget::WorkingTree,
        source: ReviewSource {
            base: ReviewBase::EmptyTree,
            content_digest: revision_id,
            includes_staged: false,
            includes_unstaged: false,
            included_untracked_files: 0,
            captured_at_ms: 0,
            uncovered_paths: Vec::new(),
        },
        context: ReviewContext {
            title: "No changes".into(),
            goal: None,
            phase_index: None,
            phase_title: None,
            planning_excerpt: None,
            acceptance_excerpt: None,
            source_intent: None,
            source_commits: 0,
            pull_request: None,
        },
        files: Vec::new(),
        chapters: Vec::new(),
        checks: CheckSummary::default(),
        stats: ReviewStats::default(),
        generated_at_ms: 0,
        insight: None,
        provenance: None,
        seen: None,
        brief: None,
    }
}

fn session() -> Box<ReviewSession> {
    let review = review();
    Box::new(ReviewSession::new(review.id, review.target, review.source, 0))
}

fn pull_request() -> crate::github::NewPullRequest {
    crate::github::NewPullRequest {
        head: "fix".into(),
        base: "main".into(),
        title: "Fix it".into(),
        body: String::new(),
        draft: false,
    }
}

fn section() -> crate::github::ListedSection {
    crate::github::ListedSection {
        name: "Mine".into(),
        items: Vec::new(),
        error: None,
        total: None,
        next: None,
    }
}

/// Every variant once, with a payload that has something in it. A variant missing here fails
/// `every_event_round_trips_under_a_published_tag`, since its tag is published without a sample.
pub(super) fn every_event() -> Vec<Event> {
    let s = |text: &str| text.to_string();
    let workspace = WorkspaceSnapshot::empty(
        crate::workspace::WorkspaceScopeId::new("scope-1"),
        crate::ProjectIdentity::non_git("/work".into(), "work", "~/work"),
    );
    let activity = ToolActivity::running("session-1", "call-1", "read", "Read Cargo.toml", 42);
    let composition = crate::context_report::ContextComposition {
        model: s("m"),
        context_window: 8,
        used_tokens: 1,
        calibration: crate::context_report::Calibration::Estimated,
        slices: Vec::new(),
        findings: Vec::new(),
        cache_hit_pct: None,
        cache_write_pct: None,
        mcp: None,
    };
    let review_failure = |message: &str| GuidedReviewFailure {
        kind: GuidedReviewFailureKind::Unavailable,
        message: s(message),
    };
    vec![
        Event::TokenDelta { block: 0, delta: s("hi") },
        Event::ThinkingDelta { block: 1, delta: s("hm") },
        Event::ToolCallComposing { call_id: s("c1"), name: s("write"), subject: None, body: None },
        Event::ToolStarted { call_id: s("c1"), name: s("read") },
        Event::ToolProgress { call_id: s("c1"), chunk: s("half") },
        Event::ToolCompleted { call_id: s("c1"), is_error: true, output: s("no") },
        Event::ToolEnded { call_id: s("c1"), is_error: false },
        Event::MessageCommitted {
            session_id: s("s1"),
            entry_id: crate::session::EntryId(s("e1")),
            message: crate::Message::assistant("done"),
        },
        Event::RemediationReported {
            entry_id: crate::session::EntryId(s("e1")),
            report: crate::guided_review::RemediationReport {
                review_id: id("review:1"),
                revision_id: ReviewRevisionId::from_canonical_bytes(b""),
                findings: Vec::new(),
            },
        },
        Event::ReviewFeedbackTurnDispatched { submission_id: SubmissionId::new() },
        Event::ReviewFeedbackTurnFailed { submission_id: SubmissionId::new(), message: s("x") },
        Event::ReviewQuestionsAnswered { submission_id: SubmissionId::new(), answer: s("yes") },
        Event::ReviewFeedbackTurnCompleted { submission_id: SubmissionId::new() },
        Event::SessionLoaded { session_id: s("s1"), messages: Vec::new() },
        Event::StateChanged(EngineState::default()),
        Event::TurnEnded { reason: StopReason::EndTurn },
        Event::LastTurnAvailable { session_id: s("s1"), turn_id: s("t1") },
        Event::CompactionStarted { tokens_before: 2 },
        Event::CompactionDone { tokens_before: 2, tokens_after: 1 },
        Event::CompactionStopped,
        Event::ContextUsage { used_tokens: 1, context_window: 8 },
        Event::TurnUsage {
            prompt_tokens: 8,
            completion_tokens: 2,
            cost_usd: Some(0.5),
            cache_read_tokens: 4,
            cache_write_tokens: 1,
            reasoning_tokens: 1,
            source: crate::usage::UsageSource::Review,
        },
        Event::SessionUsage(crate::usage::SessionUsage::default()),
        Event::PlanUsage(crate::provider::PlanUsage::default()),
        Event::ContextComposition(Box::new(composition)),
        Event::ImageAttached { mime: s("image/png"), width: 16, height: 9 },
        Event::Error(s("lost")),
        Event::Notice(s("heads up")),
        Event::PlanWritten { path: s("PLAN.md") },
        Event::ReviewStarted { model: s("m") },
        Event::ReviewFinished { verdict: RequestedReviewVerdict::Fix, findings: vec![s("f")] },
        Event::SteeringDelivered { count: 3 },
        Event::ProviderRetrying { attempt: 1, attempts: 3, reason: s("busy") },
        Event::GuidedReviewLoading {
            request_id: ReviewLoadRequestId::new(),
            target: ReviewTarget::WorkingTree,
        },
        Event::GuidedReviewLoaded {
            request_id: ReviewLoadRequestId::new(),
            review: Box::new(review()),
        },
        Event::GuidedReviewGuideUpdated {
            request_id: ReviewLoadRequestId::new(),
            review_id: id("review:1"),
            revision_id: ReviewRevisionId::from_canonical_bytes(b""),
            chapters: Vec::new(),
            provenance: crate::guided_review::GuideProvenance::Model { batches: 2 },
            brief: None,
        },
        Event::PullRequestPostPrepared {
            request_id: id("pr-post:1"),
            patches: Vec::new(),
            head_moved: true,
            own_pull_request: false,
        },
        Event::PullRequestReviewPosted {
            request_id: id("pr-post:1"),
            github_review_id: 9,
            url: s("u"),
            finding_ids: Vec::new(),
        },
        Event::PullRequestPostFailed { request_id: id("pr-post:1"), message: s("x") },
        Event::ThreadAnswersPrepared {
            request_id: id("thread-answers:1"),
            pushed: true,
            uncommitted: false,
        },
        Event::ThreadsAnswered { request_id: id("thread-answers:1"), results: Vec::new() },
        Event::ThreadAnswersFailed { request_id: id("thread-answers:1"), message: s("x") },
        Event::GithubListed { repository: Some(s("a/b")), sections: vec![section()] },
        Event::GithubSearched { request: 7, repository: None, section: section() },
        Event::GithubOverviewRead {
            repository: None,
            number: 1,
            overview: None,
            error: Some(s("x")),
            local: None,
        },
        Event::GithubConversationRead { repository: None, number: 1, entries: None, error: None },
        Event::GithubCommentPosted {
            repository: None,
            number: 1,
            thread: None,
            url: Some(s("u")),
            error: None,
        },
        Event::PullRequestChanged {
            repository: None,
            number: 1,
            change: None,
            error: Some(s("x")),
        },
        Event::GithubReplyDrafted {
            request_id: s("r1"),
            body: Some(s("b")),
            sources: vec![s("src")],
            error: None,
            note: None,
        },
        Event::StackPrepared {
            request_id: id("pr-open:1"),
            layers: Some(vec![pull_request()]),
            refusal: None,
        },
        Event::StackOpened {
            request_id: id("pr-open:1"),
            numbers: vec![1],
            urls: vec![s("u")],
            failed: None,
        },
        Event::RestackPlanned {
            request_id: id("restack:1"),
            plan: None,
            merged: None,
            refusal: Some(s("x")),
        },
        Event::Restacked {
            request_id: id("restack:1"),
            moved: vec![1],
            retargeted: Vec::new(),
            retarget_failed: None,
        },
        Event::RestackFailed { request_id: id("restack:1"), message: s("x") },
        Event::PullRequestFixChecked { number: 1, branch: Some(s("b")), refusal: None },
        Event::PullRequestPrepared {
            request_id: id("pr-open:1"),
            pull_request: pull_request(),
            publication: crate::github::Publication::Ahead { commits: 2 },
            uncommitted: 1,
            commits: vec![s("c")],
        },
        Event::PullRequestOpened {
            request_id: id("pr-open:1"),
            number: 2,
            url: s("u"),
            committed: Vec::new(),
        },
        Event::PullRequestOpenFailed { request_id: id("pr-open:1"), message: s("x") },
        Event::WorktreeRemovalChecked { uncommitted: vec![s("a.rs")] },
        Event::ReviewCommitted {
            request_id: ReviewSessionSaveRequestId::new(),
            commits: vec![s("c")],
            skipped: Vec::new(),
        },
        Event::ReviewCommitFailed {
            request_id: ReviewSessionSaveRequestId::new(),
            message: s("x"),
        },
        Event::GuidedReviewFailed {
            request_id: ReviewLoadRequestId::new(),
            target: ReviewTarget::WorkingTree,
            error: review_failure("gone"),
        },
        Event::FileDiffLoaded {
            request_id: crate::file_diff::FileDiffRequestId::new(),
            diff: Box::new(crate::file_diff::FileDiff {
                path: s("a"),
                file: None,
                untracked: true,
            }),
        },
        Event::FileDiffFailed {
            request_id: crate::file_diff::FileDiffRequestId::new(),
            path: s("a"),
            error: crate::file_diff::FileDiffFailure::Git(s("x")),
        },
        Event::DirectoryListed {
            request_id: crate::project_files::ListingRequestId::new(),
            listing: crate::project_files::DirectoryListing {
                path: s("src"),
                entries: Vec::new(),
                omitted: 2,
            },
        },
        Event::DirectoryListFailed {
            request_id: crate::project_files::ListingRequestId::new(),
            path: s("src"),
            error: crate::project_files::ProjectFileFailure::Unreadable(s("x")),
        },
        Event::ProjectFileRead {
            request_id: crate::project_files::FileReadRequestId::new(),
            file: Box::new(crate::project_files::ProjectFile {
                path: s("a"),
                text: s("t"),
                bytes: 1,
                truncated: true,
                binary: false,
            }),
        },
        Event::ProjectFileReadFailed {
            request_id: crate::project_files::FileReadRequestId::new(),
            path: s("a"),
            error: crate::project_files::ProjectFileFailure::NotFound,
        },
        Event::ReviewSessionRestored { session: session() },
        Event::ReviewSessionSaved {
            request_id: ReviewSessionSaveRequestId::new(),
            session: session(),
        },
        Event::ReviewSessionSaveFailed {
            request_id: ReviewSessionSaveRequestId::new(),
            review_id: id("review:1"),
            message: s("x"),
        },
        Event::SessionsRefreshed(vec![s("s1")]),
        Event::WorkspaceRefreshRequested,
        Event::WorkspaceUpdated(Box::new(workspace)),
        Event::TerminalFocusChanged { focused: true },
        Event::ToolActivityUpdated(Box::new(activity)),
        Event::PermissionRequested {
            id: PermissionRequestId::new(),
            session_id: s("s1"),
            tool: s("bash"),
            permission: s("bash"),
            summary: s("Run"),
            always_patterns: vec![s("rm *")],
        },
        Event::QuestionAsked {
            id: crate::question::QuestionRequestId::new(),
            session_id: s("s1"),
            prompt: s("Which?"),
            options: Vec::new(),
            multi: true,
        },
        Event::Ready {
            protocol_version: 1,
            luxe_version: s("0.1.0"),
            session_id: s("s1"),
            cwd: s("/work"),
            github_writes: true,
        },
        Event::ActionRejected { kind: Some(s("x")), reason: s("y") },
    ]
}
