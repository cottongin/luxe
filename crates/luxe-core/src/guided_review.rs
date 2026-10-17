//! Guided Review — immutable, UI-neutral review domain primitives.
//!
//! The model keeps logical review identity separate from pinned source revisions so refreshes
//! cannot mutate the diff a user is reading.

mod attention;
mod budget;
mod chaptering;
mod classification;
mod commit;
mod content;
mod context;
mod conversation;
mod diff_paths;
mod exclude;
mod feedback_message;
mod feedback_session;
mod feedback_store;
mod guide_cache;
mod guide_plan;
mod ids;
mod insight;
mod last_turn_session;
mod loader;
mod manifest;
mod model;
mod parser;
mod posting;
mod provenance;
mod pull_request;
mod pull_request_draft;
mod remediation;
mod review_session;
mod seen_store;
mod source_paths;
mod source_store;
#[cfg(test)]
pub(crate) mod test_support;
mod thread_findings;
mod turn_baseline;
mod untracked;

pub use attention::{review_attention, review_flags, AttentionKind, AttentionSignal};
pub use budget::{
    chapter_needs_review, review_budget, Accounted, ChapterBudget, ItemVerdict, ReviewBudget,
};
pub use chaptering::{
    fallback_chapters, fallback_groups, source_items, validate_chapter_coverage,
    ChapterCoverageError,
};
pub use commit::{
    chapter_commits, squashed_commit, ChapterCommit, ChapterCommitError, MAX_COMMIT_SUBJECT_CHARS,
};
pub use content::{
    LineRange, NoiseKind, NonTextChange, NonTextChangeKind, ReviewFile, ReviewFileStatus,
    ReviewHunk, ReviewLine, ReviewLineKind, SymbolContext,
};
pub use context::{
    ReviewContext, ReviewContextAssemblyError, ReviewContextDraft, ReviewContextSourceError,
    ReviewContextSourceKind, ReviewTarget, MAX_CONTEXT_EXCERPT_CHARS, MAX_CONTEXT_TITLE_CHARS,
    UNTITLED_REVIEW,
};
pub use conversation::{
    earlier_conversation, ConversationEntry, EarlierComment, EarlierConversation, EarlierThread,
    ThreadAnchor, MAX_KEPT_COMMENTS,
};
pub use exclude::{is_luxe_artifact, matches_glob, ReviewExclusions, LUXE_PROJECT_DIR};
pub use feedback_message::format_review_feedback_message;
pub use feedback_session::{
    remap_finding_anchor, title_from_body, DiffSide, FindingAnchor, FindingAnchorRefresh,
    FindingEvidence, FindingMessage, FindingMessageAuthor, FindingSeverity, FindingState,
    GitHubThread, PostedComment, ReviewFeedbackMode, ReviewFinding, ReviewOutcome, ReviewProgress,
    ReviewSession, ReviewSessionMutationError, ReviewSessionValidationError, ReviewSubmission,
    SubmissionDispatch, SubmittedFinding, REVIEW_SESSION_SCHEMA_VERSION,
};
pub use feedback_store::{
    ReviewSessionStore, ReviewSessionStoreError, REVIEW_SESSION_RELATIVE_DIR,
};
pub use guide_cache::{
    CachedGuide, GuideCache, GUIDE_CACHE_RELATIVE_PATH, GUIDE_CACHE_SCHEMA_VERSION,
};
pub use guide_plan::{
    guide_batches, guide_max_tokens, guide_retry_max_tokens, reduce_groups, GuideBatch,
    GuideFallback, GuideGroup, GuideProvenance, MAX_GUIDE_BATCH_CHARS, MAX_HUNK_SOURCE_CHARS,
};
pub use ids::{
    AgentTurnId, ChapterId, FileId, FindingId, HunkId, PullRequestOpenId, PullRequestPostId,
    ReviewId, ReviewLoadRequestId, ReviewRevisionId, ReviewSessionSaveRequestId, SubmissionId,
    ThreadAnswersId, TurnBaselineId,
};
pub use insight::{FileInsight, RelevantFile, ReviewInsight, ReviewInsightProvider};
pub use last_turn_session::{
    LastTurnRecord, LastTurnRecordError, LastTurnRecordStore, LastTurnRecordStoreError,
    LAST_TURN_RECORD_RELATIVE_PATH, LAST_TURN_RECORD_SCHEMA_VERSION,
};
pub use loader::{
    GuidedReviewError, LocalGit, PullRequestPin, ReviewSourceUnavailable, UntrackedFileKind,
    UntrackedSourcePolicy, WorkingTreeCapture, WorktreeProvenance,
};
pub use manifest::{
    GuideManifest, GuideManifestBrief, GuideManifestChapter, GuideManifestError, GuideManifestKind,
    GuideManifestNote, MAX_GUIDE_ACCOUNT_CHARS, MAX_GUIDE_ACCOUNT_ITEMS, MAX_GUIDE_CHAPTERS,
    MAX_GUIDE_ITEM_ID_CHARS, MAX_GUIDE_OUTPUT_BYTES, MAX_GUIDE_TEXT_CHARS, MAX_GUIDE_TITLE_CHARS,
};
pub use model::{
    ChapterKind, CheckSummary, GuidedReview, GuidedReviewBuildError, ItemNote, ReviewBase,
    ReviewBrief, ReviewChapter, ReviewCheck, ReviewCheckStatus, ReviewItemRef, ReviewSource,
    ReviewStats,
};
pub use parser::{parse_unified_diff, GuidedReviewParseError, ParsedDiff};
pub use posting::{
    plan_post, postable, BodyReason, PlacedComment, Placement, PostPlan, PostPlanError,
};
pub use provenance::{CommandRun, TurnProvenance, MAX_ACCOUNT_CHARS};
pub use pull_request::{
    pull_request_label, resolve_pull_request, LinkedIssue, PullRequestChecks, PullRequestContext,
    PullRequestResolveError, PullRequestStatus, PullRequestVerdict, ResolvedPullRequest,
    MAX_LINKED_ISSUES,
};
pub use pull_request_draft::{
    pull_request_draft, stack_draft, ClosedIssue, PullRequestDraft, StackLayerDraft,
};
pub(crate) use remediation::without_fence as remediation_without_fence;
pub use remediation::{
    extract_remediation_report, remediation_report_template, without_remediation_report,
    RemediationReport, RemediationReportError, RemediationResult, RemediationStatus,
    MAX_REMEDIATION_NOTE_BYTES, MAX_REMEDIATION_REPORT_BYTES, MAX_REMEDIATION_REPORT_ITEMS,
};
pub use review_session::{
    ActiveReviewRecord, ActiveReviewRecordError, ActiveReviewRecordStore,
    ActiveReviewRecordStoreError, ActiveReviewRestoreError, ACTIVE_REVIEW_RECORD_RELATIVE_PATH,
    ACTIVE_REVIEW_RECORD_SCHEMA_VERSION,
};
pub use seen_store::{
    SeenReview, SeenReviewStore, SEEN_REVIEW_RELATIVE_PATH, SEEN_REVIEW_SCHEMA_VERSION,
};
pub use source_store::{
    ReviewSourceArtifact, SourceArtifactManifest, SourceRetentionPolicy, SourceStoreError,
    SOURCE_ARTIFACT_SCHEMA_VERSION, SOURCE_CACHE_RELATIVE_DIR, SOURCE_MANIFEST_FILE,
    SOURCE_PAYLOAD_FILE,
};
pub use thread_findings::{thread_findings, ThreadAnswer, ThreadAnswerResult, ThreadReply};
pub use turn_baseline::{
    TurnBaselineArtifact, TurnBaselineExclusion, TurnBaselineFile, TurnBaselineManifest,
    TurnBaselineManifestError, TurnBaselineStoreError, TURN_BASELINE_CACHE_RELATIVE_DIR,
    TURN_BASELINE_SCHEMA_VERSION,
};
pub use untracked::{render_untracked_addition, render_whole_file_deletion};
