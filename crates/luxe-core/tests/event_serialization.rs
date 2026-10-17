//! Event serialization — exhaustive wire-contract regression coverage.
//!
//! Two tiers, because they catch different things. The frames a front-end must decode are pinned
//! to literal bytes; everything else is round-tripped, which proves a payload survives but not that
//! it kept its name. `crates/luxe-core/src/event.rs` pins the inbound half the same way.

use luxe_core::event::{Action, GuidedReviewFailure, GuidedReviewFailureKind};
use luxe_core::guided_review::{
    CheckSummary, GuidedReview, ReviewBase, ReviewContext, ReviewId, ReviewRevisionId,
    ReviewSession, ReviewSessionSaveRequestId, ReviewSource, ReviewStats, ReviewTarget,
};
use luxe_core::{
    EngineState, Event, ProjectIdentity, ReviewLoadRequestId, StopReason, ToolActivity,
    WorkspaceScopeId, WorkspaceSnapshot,
};
use serde::{Deserialize, Serialize};

fn roundtrip<T>(value: T)
where
    T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug,
{
    let encoded = serde_json::to_string(&value).unwrap();
    let decoded: T = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, value);
}

fn empty_review_session() -> ReviewSession {
    let review = empty_guided_review();
    ReviewSession::new(review.id, review.target, review.source, 0)
}

fn empty_guided_review() -> GuidedReview {
    let revision_id = ReviewRevisionId::from_canonical_bytes(b"");
    GuidedReview {
        conversation: None,
        id: ReviewId::new(),
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

/// A `ready` from a build that predates `github_writes` reads as a session that refuses writes.
///
/// The safe default for a client replaying a capture, or talking to an older Luxe: assuming writes
/// were allowed would offer buttons that can only be refused.
#[test]
fn a_ready_without_github_writes_reads_as_refusing_them() {
    let older = r#"{"kind":"ready","content":{"protocol_version":1,"luxe_version":"0.1.0","session_id":"s","cwd":"/w"}}"#;
    let Ok(Event::Ready { github_writes, .. }) = serde_json::from_str::<Event>(older) else {
        panic!("an older ready still parses");
    };
    assert!(!github_writes);
}

/// The frames a front-end cannot be written without, pinned to their exact published bytes.
///
/// The round-trips below cover the other fifty-odd events — enough to catch a payload that stopped
/// surviving the wire, but blind to a rename, because serializing and reading straight back
/// exercises both halves of the same code. These nine are the ones a client must decode to be a
/// client at all: the handshake, the two prompts it has to answer, the streaming text, the tool
/// lifecycle, and the end of the turn. A rename in any of them breaks every client silently, so
/// they are pinned to literals copied from real output rather than round-tripped.
#[test]
fn the_frames_a_client_cannot_work_without_are_pinned() {
    let pinned: Vec<(Event, &str)> = vec![
        (
            Event::Ready {
                protocol_version: 1,
                luxe_version: "0.1.0".into(),
                session_id: "session-1".into(),
                cwd: "/work/luxe".into(),
                github_writes: false,
            },
            r#"{"kind":"ready","content":{"protocol_version":1,"luxe_version":"0.1.0","session_id":"session-1","cwd":"/work/luxe","github_writes":false}}"#,
        ),
        (
            Event::ActionRejected {
                kind: Some("steer_turn".into()),
                reason: "steer_turn: missing field `text`".into(),
            },
            r#"{"kind":"action_rejected","content":{"kind":"steer_turn","reason":"steer_turn: missing field `text`"}}"#,
        ),
        (
            Event::ActionRejected { kind: None, reason: "line is not JSON".into() },
            r#"{"kind":"action_rejected","content":{"reason":"line is not JSON"}}"#,
        ),
        (
            Event::PermissionRequested {
                id: request_id("perm:1"),
                session_id: "session-1".into(),
                tool: "bash".into(),
                permission: "bash".into(),
                summary: "Run: rm -rf build".into(),
                always_patterns: vec!["rm *".into()],
            },
            r#"{"kind":"permission_requested","content":{"id":"perm:1","session_id":"session-1","tool":"bash","permission":"bash","summary":"Run: rm -rf build","always_patterns":["rm *"]}}"#,
        ),
        (
            Event::QuestionAsked {
                id: request_id("ask:1"),
                session_id: "session-1".into(),
                prompt: "Which database?".into(),
                options: vec![luxe_core::question::QuestionOption {
                    label: "postgres".into(),
                    description: Some("Relational, boring, correct".into()),
                    recommended: true,
                }],
                multi: false,
            },
            r#"{"kind":"question_asked","content":{"id":"ask:1","session_id":"session-1","prompt":"Which database?","options":[{"label":"postgres","description":"Relational, boring, correct","recommended":true}],"multi":false}}"#,
        ),
        (
            Event::TokenDelta { block: 0, delta: "hello".into() },
            r#"{"kind":"token_delta","content":{"block":0,"delta":"hello"}}"#,
        ),
        (
            Event::ToolStarted { call_id: "call-1".into(), name: "read".into() },
            r#"{"kind":"tool_started","content":{"call_id":"call-1","name":"read"}}"#,
        ),
        (
            Event::ToolEnded { call_id: "call-1".into(), is_error: false },
            r#"{"kind":"tool_ended","content":{"call_id":"call-1","is_error":false}}"#,
        ),
        (
            Event::TurnEnded { reason: StopReason::EndTurn },
            r#"{"kind":"turn_ended","content":{"reason":"end_turn"}}"#,
        ),
        (
            Event::StateChanged(luxe_core::event::EngineState {
                model: luxe_core::event::ModelRef::new("anthropic", "claude-sonnet-5"),
                streaming: false,
                status: "mode: plan".into(),
                mode: "plan".into(),
                thinking: luxe_core::provider::ThinkingLevel::High,
            }),
            r#"{"kind":"state_changed","content":{"model":{"provider":"anthropic","model":"claude-sonnet-5"},"streaming":false,"status":"mode: plan","mode":"plan","thinking":"high"}}"#,
        ),
        (
            Event::PlanWritten { path: "PLAN.md".into() },
            r#"{"kind":"plan_written","content":{"path":"PLAN.md"}}"#,
        ),
        (
            Event::ReviewStarted { model: "openai/gpt-5.6".into() },
            r#"{"kind":"review_started","content":{"model":"openai/gpt-5.6"}}"#,
        ),
        (
            Event::ReviewFinished {
                verdict: luxe_core::event::RequestedReviewVerdict::Fix,
                findings: vec!["src/a.rs:3 unwraps a fallible read".into()],
            },
            r#"{"kind":"review_finished","content":{"verdict":"fix","findings":["src/a.rs:3 unwraps a fallible read"]}}"#,
        ),
        (
            Event::PullRequestPostPrepared {
                request_id: request_id("pr-post:1"),
                patches: vec![luxe_core::github::FilePatch {
                    path: "src/router.rs".into(),
                    previous_path: None,
                    commentable: luxe_core::github::CommentableLines {
                        left: [10].into(),
                        right: [10, 11].into(),
                    },
                }],
                head_moved: false,
                own_pull_request: true,
            },
            r#"{"kind":"pull_request_post_prepared","content":{"request_id":"pr-post:1","patches":[{"path":"src/router.rs","previous_path":null,"commentable":{"left":[10],"right":[10,11]}}],"head_moved":false,"own_pull_request":true}}"#,
        ),
        (
            Event::PullRequestReviewPosted {
                request_id: request_id("pr-post:1"),
                github_review_id: 5307695620,
                url: "https://github.com/acme/app/pull/7#pullrequestreview-5307695620".into(),
                finding_ids: vec![request_id("finding:1")],
            },
            r#"{"kind":"pull_request_review_posted","content":{"request_id":"pr-post:1","github_review_id":5307695620,"url":"https://github.com/acme/app/pull/7#pullrequestreview-5307695620","finding_ids":["finding:1"]}}"#,
        ),
        (
            Event::ThreadAnswersPrepared {
                request_id: request_id("thread-answers:1"),
                pushed: false,
                uncommitted: true,
            },
            r#"{"kind":"thread_answers_prepared","content":{"request_id":"thread-answers:1","pushed":false,"uncommitted":true}}"#,
        ),
        (
            Event::ThreadsAnswered {
                request_id: request_id("thread-answers:1"),
                results: vec![luxe_core::guided_review::ThreadAnswerResult {
                    finding_id: request_id("finding:1"),
                    reply_url: Some("https://github.com/acme/app/pull/146#discussion_r1".into()),
                    resolved: true,
                    error: None,
                }],
            },
            r#"{"kind":"threads_answered","content":{"request_id":"thread-answers:1","results":[{"finding_id":"finding:1","reply_url":"https://github.com/acme/app/pull/146#discussion_r1","resolved":true,"error":null}]}}"#,
        ),
        (
            Event::ThreadAnswersFailed {
                request_id: request_id("thread-answers:1"),
                message: "The GitHub CLI is not signed in.".into(),
            },
            r#"{"kind":"thread_answers_failed","content":{"request_id":"thread-answers:1","message":"The GitHub CLI is not signed in."}}"#,
        ),
        (
            Event::GithubListed {
                repository: Some("acme/app".into()),
                sections: vec![luxe_core::github::ListedSection {
                    name: "Mine".into(),
                    items: vec![luxe_core::github::ListItem {
                        pull_request: true,
                        number: 164,
                        title: "Open a branch's review as a stack".into(),
                        url: "https://github.com/acme/app/pull/164".into(),
                        repository: "acme/app".into(),
                        author: "mara".into(),
                        draft: false,
                        updated_at: "2026-09-25T17:00:00Z".into(),
                        state: luxe_core::github::PullRequestState::Open,
                        facts: None,
                    }],
                    error: None,
                    total: None,
                    next: None,
                }],
            },
            r#"{"kind":"github_listed","content":{"repository":"acme/app","sections":[{"name":"Mine","items":[{"pull_request":true,"number":164,"title":"Open a branch's review as a stack","url":"https://github.com/acme/app/pull/164","repository":"acme/app","author":"mara","draft":false,"updated_at":"2026-09-25T17:00:00Z","state":"open"}],"error":null}]}}"#,
        ),
        (
            Event::GithubSearched {
                request: 7,
                repository: Some("acme/app".into()),
                section: luxe_core::github::ListedSection {
                    name: "Pull requests".into(),
                    items: vec![luxe_core::github::ListItem {
                        pull_request: true,
                        number: 131,
                        title: "Retry a webhook once".into(),
                        url: "https://github.com/acme/app/pull/131".into(),
                        repository: "acme/app".into(),
                        author: "lena".into(),
                        draft: false,
                        updated_at: "2026-09-20T09:00:00Z".into(),
                        state: luxe_core::github::PullRequestState::Merged,
                        facts: None,
                    }],
                    error: None,
                    total: Some(214),
                    next: Some("Y3Vyc29yOjMw".into()),
                },
            },
            r#"{"kind":"github_searched","content":{"request":7,"repository":"acme/app","section":{"name":"Pull requests","items":[{"pull_request":true,"number":131,"title":"Retry a webhook once","url":"https://github.com/acme/app/pull/131","repository":"acme/app","author":"lena","draft":false,"updated_at":"2026-09-20T09:00:00Z","state":"merged"}],"error":null,"total":214,"next":"Y3Vyc29yOjMw"}}}"#,
        ),
        (
            Event::GithubOverviewRead {
                repository: None,
                number: 142,
                overview: Some(luxe_core::github::overview::Overview::PullRequest(Box::new(
                    pinned_overview(),
                ))),
                error: None,
                local: None,
            },
            r#"{"kind":"github_overview_read","content":{"repository":null,"number":142,"overview":{"kind":"pull_request","overview":{"number":142,"node_id":"","title":"Route digests by category","body":"Routes each digest.","url":"https://github.com/acme/app/pull/142","repository":"acme/app","author":{"login":"mara","bot":false},"state":"open","draft":false,"base_ref":"main","head_ref":"digest-routing","head_oid":"4c1e2a0f9b","cross_repository":false,"additions":96,"deletions":31,"changed_files":7,"commits":6,"updated_at":1790265600,"labels":[{"name":"backend","color":"1d76db"}],"merge_status":"blocked","review_decision":"changes_requested","reviewers":[{"login":"jon","kind":"user","verdict":"changes_requested","at":1790254800}],"checks":{"passed":11,"failed":1,"pending":0,"skipped":0},"failing":[{"name":"test (ubuntu-latest)","required":true}],"unresolved_threads":2,"stack":null,"viewer":{"author":true,"can_push":true,"can_merge":true},"merge_methods":["merge","squash"],"merge_settings":{"last_method":null,"deletes_branch":false,"can_auto_merge":false,"can_cancel_auto_merge":false,"auto_merge":null},"conversation_count":7}},"error":null}}"#,
        ),
        (
            Event::GithubOverviewRead {
                repository: Some("other/lib".into()),
                number: 9,
                overview: None,
                error: Some("GitHub has no such pull request that this account can see.".into()),
                local: None,
            },
            r#"{"kind":"github_overview_read","content":{"repository":"other/lib","number":9,"overview":null,"error":"GitHub has no such pull request that this account can see."}}"#,
        ),
        (
            Event::ReviewQuestionsAnswered {
                submission_id: serde_json::from_str(
                    "\"submission:c32d1080-1072-49c5-82c9-62aec2e5fcf4\"",
                )
                .expect("a submission id"),
                answer: "Short answer: it's a `trim_end` artifact.".into(),
            },
            r#"{"kind":"review_questions_answered","content":{"submission_id":"submission:c32d1080-1072-49c5-82c9-62aec2e5fcf4","answer":"Short answer: it's a `trim_end` artifact."}}"#,
        ),
        (
            Event::GithubReplyDrafted {
                request_id: "draft:142:1".into(),
                body: Some("Fixed in `4c1e2a0`.".into()),
                sources: vec!["this session".into(), "the thread".into()],
                error: None,
                note: None,
            },
            r#"{"kind":"github_reply_drafted","content":{"request_id":"draft:142:1","body":"Fixed in `4c1e2a0`.","sources":["this session","the thread"],"error":null}}"#,
        ),
        (
            Event::GithubReplyDrafted {
                request_id: "draft:review:4:1".into(),
                body: Some("Approving.".into()),
                sources: vec!["this session".into()],
                error: None,
                note: Some("I haven't read the diff myself this turn.".into()),
            },
            r#"{"kind":"github_reply_drafted","content":{"request_id":"draft:review:4:1","body":"Approving.","sources":["this session"],"error":null,"note":"I haven't read the diff myself this turn."}}"#,
        ),
        (
            Event::PullRequestChanged {
                repository: None,
                number: 1,
                change: Some(luxe_core::engine::PullRequestChange::AutoMergeEnabled {
                    method: luxe_core::github::overview::MergeMethod::Squash,
                }),
                error: None,
            },
            r#"{"kind":"pull_request_changed","content":{"repository":null,"number":1,"change":{"change":"auto_merge_enabled","method":"squash"},"error":null}}"#,
        ),
        (
            Event::PullRequestChanged {
                repository: None,
                number: 142,
                change: Some(luxe_core::engine::PullRequestChange::Merged {
                    sha: "abce6b1396d97556c3a8f0beafbb681b5963d714".into(),
                    branch_deleted: true,
                }),
                error: None,
            },
            r#"{"kind":"pull_request_changed","content":{"repository":null,"number":142,"change":{"change":"merged","sha":"abce6b1396d97556c3a8f0beafbb681b5963d714","branch_deleted":true},"error":null}}"#,
        ),
        (
            Event::GithubCommentPosted {
                repository: None,
                number: 142,
                thread: Some("PRRT_1".into()),
                url: Some("https://github.com/acme/app/pull/142#discussion_r2".into()),
                error: None,
            },
            r#"{"kind":"github_comment_posted","content":{"repository":null,"number":142,"thread":"PRRT_1","url":"https://github.com/acme/app/pull/142#discussion_r2","error":null}}"#,
        ),
        (
            Event::GithubConversationRead {
                repository: None,
                number: 142,
                entries: Some(pinned_conversation()),
                error: None,
            },
            r#"{"kind":"github_conversation_read","content":{"repository":null,"number":142,"entries":[{"kind":"opening","author":{"login":"nora","bot":false},"body":"Routes each digest.","at":1790260000},{"kind":"comment","id":"mara-1","author":"mara","bot":false,"mine":false,"body":"Reads well.","created_at":"2026-09-24T12:00:00Z","url":"https://github.com/acme/app/pull/142#issuecomment-1"},{"kind":"review","comment":{"id":"jon-1","author":"jon","bot":false,"mine":false,"body":"Two things first.","created_at":"2026-09-24T12:00:00Z","url":"https://github.com/acme/app/pull/142#issuecomment-1"},"verdict":"changes_requested","line_comments":2},{"kind":"thread","id":"PRRT_1","path":"src/dispatch/router.rs","line":88,"start_line":null,"side":"right","outdated":false,"resolved":false,"file_level":false,"comments":[{"author":"jon","body":"This decides the fallback again.","created_at":"2026-09-24T12:05:00Z","url":"https://github.com/acme/app/pull/142#discussion_r1"}],"total_comments":1,"anchor_text":"let schedule = fallback();","can_reply":true,"can_resolve":true}],"error":null}}"#,
        ),
        (
            Event::StackPrepared {
                request_id: request_id("pr-open:2"),
                layers: Some(vec![luxe_core::github::NewPullRequest {
                    head: "fix-part-1".into(),
                    base: "main".into(),
                    title: "Sum every element".into(),
                    body: "The loop started at 1.\n\nPart 1 of 2.".into(),
                    draft: false,
                }]),
                refusal: None,
            },
            r#"{"kind":"stack_prepared","content":{"request_id":"pr-open:2","layers":[{"head":"fix-part-1","base":"main","title":"Sum every element","body":"The loop started at 1.\n\nPart 1 of 2.","draft":false}],"refusal":null}}"#,
        ),
        (
            Event::StackOpened {
                request_id: request_id("pr-open:2"),
                numbers: vec![170, 171],
                urls: vec![
                    "https://github.com/acme/app/pull/170".into(),
                    "https://github.com/acme/app/pull/171".into(),
                ],
                failed: None,
            },
            r#"{"kind":"stack_opened","content":{"request_id":"pr-open:2","numbers":[170,171],"urls":["https://github.com/acme/app/pull/170","https://github.com/acme/app/pull/171"],"failed":null}}"#,
        ),
        (
            Event::RestackPlanned {
                request_id: request_id("restack:1"),
                plan: Some(luxe_core::restack::RestackPlan {
                    remote: "origin".into(),
                    steps: vec![luxe_core::restack::RestackStep {
                        number: 140,
                        head_ref: "feat/threads".into(),
                        old: "a".repeat(40),
                        new: "b".repeat(40),
                        commits: 2,
                        onto: "feat/review".into(),
                        retarget: None,
                        checked_out: None,
                        local: true,
                    }],
                }),
                merged: None,
                refusal: None,
            },
            r#"{"kind":"restack_planned","content":{"request_id":"restack:1","plan":{"remote":"origin","steps":[{"number":140,"head_ref":"feat/threads","old":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","new":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","commits":2,"onto":"feat/review","retarget":null,"checked_out":null,"local":true}]},"merged":null,"refusal":null}}"#,
        ),
        (
            Event::Restacked {
                request_id: request_id("restack:1"),
                moved: vec![140, 143],
                retargeted: vec![140],
                retarget_failed: None,
            },
            r#"{"kind":"restacked","content":{"request_id":"restack:1","moved":[140,143],"retargeted":[140],"retarget_failed":null}}"#,
        ),
        (
            Event::RestackFailed {
                request_id: request_id("restack:1"),
                message: "Git could not push the stack: rejected.".into(),
            },
            r#"{"kind":"restack_failed","content":{"request_id":"restack:1","message":"Git could not push the stack: rejected."}}"#,
        ),
        (
            Event::PullRequestFixChecked {
                number: 142,
                branch: Some("mara/main".into()),
                refusal: None,
            },
            r#"{"kind":"pull_request_fix_checked","content":{"number":142,"branch":"mara/main","refusal":null}}"#,
        ),
        (
            Event::PullRequestPrepared {
                request_id: request_id("pr-open:1"),
                pull_request: luxe_core::github::NewPullRequest {
                    head: "123-total-skips".into(),
                    base: "main".into(),
                    title: "Count the first number".into(),
                    body: "Closes #123".into(),
                    draft: false,
                },
                publication: luxe_core::github::Publication::Ahead { commits: 2 },
                uncommitted: 1,
                commits: vec!["Sum every element".into()],
            },
            r#"{"kind":"pull_request_prepared","content":{"request_id":"pr-open:1","pull_request":{"head":"123-total-skips","base":"main","title":"Count the first number","body":"Closes #123","draft":false},"publication":{"state":"ahead","commits":2},"uncommitted":1,"commits":["Sum every element"]}}"#,
        ),
        (
            Event::PullRequestOpened {
                request_id: request_id("pr-open:1"),
                number: 151,
                url: "https://github.com/acme/app/pull/151".into(),
                committed: vec!["0123abcd".into()],
            },
            r#"{"kind":"pull_request_opened","content":{"request_id":"pr-open:1","number":151,"url":"https://github.com/acme/app/pull/151","committed":["0123abcd"]}}"#,
        ),
        (
            Event::PullRequestOpenFailed {
                request_id: request_id("pr-open:1"),
                message: "GitHub refused it: No commits between main and main".into(),
            },
            r#"{"kind":"pull_request_open_failed","content":{"request_id":"pr-open:1","message":"GitHub refused it: No commits between main and main"}}"#,
        ),
        (
            Event::WorktreeRemovalChecked { uncommitted: vec!["stray.txt".into()] },
            r#"{"kind":"worktree_removal_checked","content":{"uncommitted":["stray.txt"]}}"#,
        ),
        (
            Event::TerminalFocusChanged { focused: false },
            r#"{"kind":"terminal_focus_changed","content":{"focused":false}}"#,
        ),
        (
            Event::PullRequestPostFailed {
                request_id: request_id("pr-post:1"),
                message: "GitHub refused the review: Line could not be resolved.".into(),
            },
            r#"{"kind":"pull_request_post_failed","content":{"request_id":"pr-post:1","message":"GitHub refused the review: Line could not be resolved."}}"#,
        ),
    ];
    for (event, wire) in pinned {
        assert_eq!(
            serde_json::to_string(&event).unwrap().as_str(),
            wire,
            "a published frame changed shape"
        );
        assert_eq!(
            serde_json::from_str::<Event>(wire).unwrap(),
            event,
            "a published frame does not read back the form it writes"
        );
    }
}

/// A request id parsed from its wire form, so a pinned literal stays deterministic.
fn request_id<T: for<'de> Deserialize<'de>>(literal: &str) -> T {
    serde_json::from_value(serde_json::json!(literal)).expect("request ids are wire strings")
}

#[test]
fn every_known_event_roundtrips() {
    let workspace = WorkspaceSnapshot::empty(
        WorkspaceScopeId::new("scope-1"),
        ProjectIdentity::non_git("/work/luxe".into(), "luxe", "~/work/luxe"),
    );
    let activity = ToolActivity::running("session-1", "call-1", "read", "Read Cargo.toml", 42);

    for event in vec![
        Event::TokenDelta { block: 0, delta: "hello".into() },
        Event::ThinkingDelta { block: 1, delta: "considering".into() },
        Event::ToolCallComposing {
            call_id: "call-1".into(),
            name: "write".into(),
            subject: None,
            body: None,
        },
        Event::ToolCallComposing {
            call_id: "call-1".into(),
            name: "write".into(),
            subject: Some("src/main.rs".into()),
            body: luxe_core::tool_card_body(
                "write",
                &serde_json::json!({"path": "src/main.rs", "content": "fn main() {}\n"}),
                "/project",
            ),
        },
        Event::ToolStarted { call_id: "call-1".into(), name: "read".into() },
        Event::ToolProgress { call_id: "call-1".into(), chunk: "halfway".into() },
        Event::ToolEnded { call_id: "call-1".into(), is_error: false },
        Event::MessageCommitted {
            session_id: "session-1".into(),
            entry_id: luxe_core::session::EntryId("entry-1".into()),
            message: luxe_core::Message::assistant("done"),
        },
        Event::SessionLoaded {
            session_id: "session-1".into(),
            messages: vec![luxe_core::event::SessionMessage {
                entry_id: luxe_core::session::EntryId("entry-1".into()),
                message: luxe_core::Message::user("hello"),
            }],
        },
        Event::StateChanged(EngineState::default()),
        Event::TurnEnded { reason: StopReason::EndTurn },
        Event::LastTurnAvailable { session_id: "session-1".into(), turn_id: "turn-1".into() },
        Event::CompactionDone { tokens_before: 2_000, tokens_after: 1_000 },
        Event::ContextUsage { used_tokens: 1_000, context_window: 8_000 },
        Event::TurnUsage {
            prompt_tokens: 800,
            completion_tokens: 200,
            cost_usd: Some(0.02),
            cache_read_tokens: 512,
            cache_write_tokens: 64,
            reasoning_tokens: 40,
            source: luxe_core::usage::UsageSource::Review,
        },
        Event::SessionUsage(luxe_core::usage::SessionUsage {
            total: luxe_core::usage::UsageTotals {
                calls: 3,
                prompt_tokens: 2_400,
                completion_tokens: 600,
                cache_read_tokens: 1_536,
                cache_write_tokens: 192,
                reasoning_tokens: 120,
                cost_usd: 0.06,
                unpriced_calls: 1,
                plan_usd: 0.01,
                plan_calls: 1,
            },
            by_source: vec![luxe_core::usage::SourceUsage {
                source: luxe_core::usage::UsageSource::Compaction,
                totals: luxe_core::usage::UsageTotals { calls: 1, ..Default::default() },
            }],
        }),
        Event::PlanUsage(luxe_core::provider::PlanUsage {
            plan: Some("plus".into()),
            primary: Some(luxe_core::provider::PlanWindow {
                used_percent: 12.5,
                window_minutes: 10_080,
                resets_in_secs: Some(603_553),
            }),
            secondary: None,
            credits_balance: Some(4.25),
        }),
        Event::ImageAttached { mime: "image/png".into(), width: 16, height: 9 },
        Event::Error("connection lost".into()),
        Event::GuidedReviewLoading {
            request_id: ReviewLoadRequestId::new(),
            target: ReviewTarget::Base { reference: "origin/main".into() },
        },
        Event::GuidedReviewLoaded {
            request_id: ReviewLoadRequestId::new(),
            review: Box::new(empty_guided_review()),
        },
        Event::GuidedReviewFailed {
            request_id: ReviewLoadRequestId::new(),
            target: ReviewTarget::LastTurn {
                session_id: "session-1".into(),
                turn_id: "turn-1".into(),
            },
            error: GuidedReviewFailure {
                kind: GuidedReviewFailureKind::Unavailable,
                message: "no complete baseline is retained".into(),
            },
        },
        Event::ReviewSessionSaved {
            request_id: ReviewSessionSaveRequestId::new(),
            session: Box::new(empty_review_session()),
        },
        Event::ReviewSessionSaveFailed {
            request_id: ReviewSessionSaveRequestId::new(),
            review_id: ReviewId::new(),
            message: "Review feedback was not saved. Retry the save.".into(),
        },
        Event::SessionsRefreshed(vec!["session-1".into(), "session-2".into()]),
        Event::WorkspaceRefreshRequested,
        Event::WorkspaceUpdated(Box::new(workspace)),
        Event::ToolActivityUpdated(Box::new(activity)),
    ] {
        roundtrip(event);
    }
}

#[test]
fn unknown_variants_preserve_the_original_payload() {
    let action_wire = serde_json::json!({
        "kind": "future_action",
        "content": {"nested": ["keep", 1]},
        "request_id": "request-1"
    });
    let action: Action = serde_json::from_value(action_wire.clone()).unwrap();
    match action {
        Action::Other { kind, content } => {
            assert_eq!(kind, "future_action");
            assert_eq!(content, action_wire);
        }
        other => panic!("expected Action::Other, got {other:?}"),
    }

    let event_wire = serde_json::json!({
        "kind": "future_event",
        "content": {"nested": ["keep", 1]},
        "request_id": "request-1"
    });
    let event: Event = serde_json::from_value(event_wire.clone()).unwrap();
    match event {
        Event::Other { kind, content } => {
            assert_eq!(kind, "future_event");
            assert_eq!(content, event_wire);
        }
        other => panic!("expected Event::Other, got {other:?}"),
    }
}

/// The overview the pinned `github_overview_read` frame carries.
fn pinned_overview() -> luxe_core::github::overview::PullRequestOverview {
    use luxe_core::github::overview::*;
    use luxe_core::github::{Author, CheckRollup, PullRequestState, ReviewDecision};
    PullRequestOverview {
        node_id: String::new(),
        number: 142,
        title: "Route digests by category".into(),
        body: "Routes each digest.".into(),
        url: "https://github.com/acme/app/pull/142".into(),
        repository: "acme/app".into(),
        author: Author { login: "mara".into(), bot: false },
        state: PullRequestState::Open,
        draft: false,
        base_ref: "main".into(),
        head_ref: "digest-routing".into(),
        head_oid: "4c1e2a0f9b".into(),
        cross_repository: false,
        additions: 96,
        deletions: 31,
        changed_files: 7,
        commits: 6,
        updated_at: 1_790_265_600,
        labels: vec![Label { name: "backend".into(), color: "1d76db".into() }],
        merge_status: MergeStatus::Blocked,
        review_decision: Some(ReviewDecision::ChangesRequested),
        reviewers: vec![Reviewer {
            login: "jon".into(),
            kind: ReviewerKind::User,
            verdict: Verdict::ChangesRequested,
            at: Some(1_790_254_800),
        }],
        checks: CheckRollup { passed: 11, failed: 1, pending: 0, skipped: 0 },
        failing: vec![FailingCheckName { name: "test (ubuntu-latest)".into(), required: true }],
        unresolved_threads: 2,
        stack: None,
        viewer: Viewer { author: true, can_push: true, can_merge: true },
        merge_methods: vec![MergeMethod::Merge, MergeMethod::Squash],
        merge_settings: Default::default(),
        conversation_count: 7,
    }
}

/// One of each kind of conversation entry, as the pinned `github_conversation_read` frame carries.
fn pinned_conversation() -> Vec<luxe_core::github::overview::ConversationEntry> {
    use luxe_core::github::overview::{ConversationEntry, Verdict};
    use luxe_core::github::{Author, ConversationComment, ReviewThread, ThreadComment, ThreadSide};
    let comment = |author: &str, body: &str| ConversationComment {
        id: format!("{author}-1"),
        author: author.into(),
        bot: false,
        mine: false,
        body: body.into(),
        created_at: "2026-09-24T12:00:00Z".into(),
        url: "https://github.com/acme/app/pull/142#issuecomment-1".into(),
    };
    vec![
        ConversationEntry::Opening {
            author: Author { login: "nora".into(), bot: false },
            body: "Routes each digest.".into(),
            at: 1_790_260_000,
        },
        ConversationEntry::Comment(comment("mara", "Reads well.")),
        ConversationEntry::Review {
            comment: comment("jon", "Two things first."),
            verdict: Verdict::ChangesRequested,
            line_comments: 2,
        },
        ConversationEntry::Thread(ReviewThread {
            id: "PRRT_1".into(),
            path: "src/dispatch/router.rs".into(),
            line: Some(88),
            start_line: None,
            side: ThreadSide::Right,
            outdated: false,
            resolved: false,
            file_level: false,
            comments: vec![ThreadComment {
                author: "jon".into(),
                body: "This decides the fallback again.".into(),
                created_at: "2026-09-24T12:05:00Z".into(),
                url: "https://github.com/acme/app/pull/142#discussion_r1".into(),
            }],
            total_comments: 1,
            anchor_text: Some("let schedule = fallback();".into()),
            can_reply: true,
            can_resolve: true,
        }),
    ]
}
