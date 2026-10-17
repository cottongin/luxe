use super::*;
use crate::activity::ToolActivity;
use crate::guided_review::{
    GuidedReview, RemediationReport, ReviewId, ReviewLoadRequestId, ReviewRevisionId,
    ReviewSession, ReviewTarget, SubmissionId,
};
use crate::message::Message;
use crate::permission::PermissionRequestId;
use crate::session::EntryId;
use crate::workspace::WorkspaceSnapshot;
use serde::{Deserialize, Serialize};

mod decoding;
mod samples;
use samples::{tag_of, EVENT_TAGS};

fn roundtrip<T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug>(
    name: &str,
    value: &T,
) {
    let json = serde_json::to_string(value).expect("serialize");
    let decoded: T = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(&decoded, value, "{} round-trip mismatch", name);
}

#[test]
fn model_ref_roundtrip() {
    let m = ModelRef::new("openai", "gpt-4o");
    roundtrip("model ref", &m);
}

#[test]
fn model_ref_with_base_url_roundtrip() {
    let m = ModelRef::new("anthropic", "claude-sonnet-4-20250514")
        .with_base_url("http://localhost:8080");
    roundtrip("model ref (base_url)", &m);
}

/// A request id parsed from its wire form, so a pinned literal stays deterministic.
///
/// Doubles as an assertion that these ids really are plain strings on the wire: if one grew a
/// struct shape, every call here would stop deserializing.
fn id<T: for<'de> Deserialize<'de>>(literal: &str) -> T {
    serde_json::from_value(serde_json::json!(literal)).expect("request ids are wire strings")
}

/// One representative value per wire [`Action`] variant, `Other` excluded.
///
/// `every_action_variant_is_sampled` holds it complete against [`action_tag_of`], which the
/// compiler holds complete against the enum.
fn sample_actions() -> Vec<Action> {
    vec![
        Action::SubmitUserMessage {
            text: "summarize the diff".into(),
            images: vec![crate::engine::ImageAttachment {
                data: "aGk=".into(),
                mime: "image/png".into(),
                name: Some("shot.png".into()),
            }],
        },
        Action::SteerTurn { text: "check the windows path too".into() },
        Action::WithdrawSteering,
        Action::Cancel,
        Action::EditResend { text: "summarize the diff, briefly".into() },
        Action::Retry,
        Action::Regenerate,
        Action::RespondPermission {
            id: id("perm:1"),
            reply: crate::permission::PermissionReply::AlwaysPersist,
        },
        Action::AnswerQuestion {
            id: id("ask:1"),
            reply: crate::question::QuestionReply::Answered(vec!["postgres".into()]),
        },
        Action::SetPermissionAutoApprove(true),
        Action::SelectModel(ModelRef::new("anthropic", "claude-sonnet-4-20250514")),
        Action::SetThinkingLevel(crate::provider::ThinkingLevel::High),
        Action::SetMode("plan".into()),
        Action::RequestReview { focus: Some("error handling".into()) },
        Action::NewSession,
        Action::LoadSession("session-1".into()),
        Action::RefreshSessions,
        Action::RefreshWorkspace,
        Action::RequestContextComposition,
        Action::CompactNow { focus: Some("the API decisions".into()) },
        Action::LoadGuidedReview {
            request_id: id("review-load:1"),
            target: ReviewTarget::WorkingTree,
        },
        Action::EditReview {
            request_id: id("review-save:1"),
            review_id: id("review:1"),
            edit: crate::engine::ReviewEdit::Comment {
                draft: crate::engine::CommentDraft {
                    anchor: crate::guided_review::FindingAnchor::Line {
                        file_id: id("file:1"),
                        hunk_id: id("hunk:1"),
                        side: crate::guided_review::DiffSide::New,
                        line: 12,
                        context_hash: String::new(),
                    },
                    severity: crate::guided_review::FindingSeverity::Question,
                    title: String::new(),
                    body: "Why does this retry twice?".into(),
                    proposed_fix: None,
                },
                send: true,
            },
        },
        Action::RecordReviewProgress {
            review_id: id("review:1"),
            walked_chapter_ids: vec![id("chapter:1")],
        },
        Action::RecordReviewSeen { review_id: id("review:1") },
        Action::RefreshGuidedReview {
            request_id: id("review-load:2"),
            review_id: id("review:1"),
            revision_id: id("revision:1"),
        },
        Action::CommitReviewChapters {
            request_id: id("review-save:2"),
            review_id: id("review:1"),
            revision_id: id("revision:1"),
            chapter_ids: Vec::new(),
            squash: true,
        },
        Action::PreparePullRequest { request_id: id("pr-open:1"), review_id: id("review:1") },
        Action::OpenPullRequest {
            request_id: id("pr-open:1"),
            title: "Retry once".into(),
            body: "Why.".into(),
            draft: false,
        },
        Action::PrepareStack { request_id: id("pr-open:2"), review_id: id("review:1") },
        Action::OpenStack { request_id: id("pr-open:2"), draft: true },
        Action::PreparePullRequestPost { request_id: id("pr-post:1"), review_id: id("review:1") },
        Action::PostPullRequestReview {
            request_id: id("pr-post:1"),
            verdict: crate::github::ReviewEvent::RequestChanges,
            session_revision: 4,
        },
        Action::PrepareThreadAnswers {
            request_id: id("thread-answers:1"),
            review_id: id("review:1"),
        },
        Action::AnswerThreads {
            request_id: id("thread-answers:1"),
            review_id: id("review:1"),
            answers: vec![crate::guided_review::ThreadReply {
                finding_id: id("finding:1"),
                reply: Some("Fixed in the last push.".into()),
                resolve: true,
            }],
        },
        Action::ListGithub { sections: Some(vec![section()]) },
        Action::SearchGithub { request: 7, section: section(), after: Some("Y3Vyc29yOjMw".into()) },
        Action::ReadGithubOverview { repository: None, number: 142, pull_request: true },
        Action::ReadGithubConversation {
            repository: Some("acme/app".into()),
            number: 9,
            pull_request: false,
        },
        Action::DraftGithubReply(crate::engine::DraftRequest {
            request_id: "draft:1".into(),
            repository: None,
            number: 142,
            pull_request: true,
            target: crate::engine::DraftTarget::Comment(crate::github::ConversationComment {
                id: "IC_1".into(),
                author: "mara".into(),
                bot: false,
                mine: false,
                body: "Does this handle Windows paths?".into(),
                created_at: "2026-08-20T10:00:00Z".into(),
                url: "https://github.com/acme/app/pull/142#issuecomment-1".into(),
            }),
            conversation: Vec::new(),
            finding: None,
            note: None,
            current: None,
        }),
        Action::CheckPullRequestFix { number: 142, repository: None },
        Action::PlanRestack { request_id: id("restack:1") },
        Action::CheckWorktreeRemoval,
        Action::LoadFileDiff { request_id: id("file-diff:1"), path: "src/main.rs".into() },
        Action::ListDirectory {
            request_id: id("listing:1"),
            path: "src".into(),
            include_ignored: false,
        },
        Action::ReadProjectFile { request_id: id("file-read:1"), path: "src/main.rs".into() },
        Action::SetFocus { focused: false },
        Action::LinkPullRequest { number: 142, url: "https://github.com/acme/app/pull/142".into() },
        Action::PostGithubComment(crate::engine::GithubPost {
            repository: None,
            number: 142,
            thread: Some("PRRT_1".into()),
            answering: None,
            body: "Fixed in the latest push.".into(),
        }),
        Action::ChangePullRequest {
            repository: None,
            number: 142,
            write: crate::engine::PullRequestWrite::Merge(crate::github::overview::MergeRequest {
                method: crate::github::overview::MergeMethod::Squash,
                head_oid: "a".repeat(40),
                delete_branch: Some("feat/threads".into()),
            }),
        },
        Action::Restack { request_id: id("restack:1") },
        Action::Close,
    ]
}

fn section() -> crate::config::GithubSection {
    crate::config::GithubSection {
        name: "Mine".into(),
        query: "is:open is:pr author:@me repo:{repo}".into(),
    }
}

/// Every wire `Action` pinned to the exact bytes a client writes.
///
/// The literals were copied from real serializer output, not written from what the types look
/// like they ought to emit. That is the whole point: a round-trip exercises both halves of the
/// current code, so it passes happily through a renamed field or a retagged variant — which on
/// a published protocol is a silent break in every client at once. Changing a string here is
/// changing the contract, and `PROTOCOL_VERSION` has to move with it.
#[test]
fn the_wire_action_vocabulary_is_pinned() {
    const PINNED: &[&str] = &[
        r#"{"kind":"submit_user_message","content":{"text":"summarize the diff","images":[{"data":"aGk=","mime":"image/png","name":"shot.png"}]}}"#,
        r#"{"kind":"steer_turn","content":{"text":"check the windows path too"}}"#,
        r#"{"kind":"withdraw_steering"}"#,
        r#"{"kind":"cancel"}"#,
        r#"{"kind":"edit_resend","content":{"text":"summarize the diff, briefly"}}"#,
        r#"{"kind":"retry"}"#,
        r#"{"kind":"regenerate"}"#,
        r#"{"kind":"respond_permission","content":{"id":"perm:1","reply":"always_persist"}}"#,
        r#"{"kind":"answer_question","content":{"id":"ask:1","reply":{"answered":["postgres"]}}}"#,
        r#"{"kind":"set_permission_auto_approve","content":true}"#,
        r#"{"kind":"select_model","content":{"provider":"anthropic","model":"claude-sonnet-4-20250514"}}"#,
        r#"{"kind":"set_thinking_level","content":"high"}"#,
        r#"{"kind":"set_mode","content":"plan"}"#,
        r#"{"kind":"request_review","content":{"focus":"error handling"}}"#,
        r#"{"kind":"new_session"}"#,
        r#"{"kind":"load_session","content":"session-1"}"#,
        r#"{"kind":"refresh_sessions"}"#,
        r#"{"kind":"refresh_workspace"}"#,
        r#"{"kind":"request_context_composition"}"#,
        r#"{"kind":"compact_now","content":{"focus":"the API decisions"}}"#,
        r#"{"kind":"load_guided_review","content":{"request_id":"review-load:1","target":{"kind":"working_tree"}}}"#,
        r#"{"kind":"edit_review","content":{"request_id":"review-save:1","review_id":"review:1","edit":"comment","anchor":{"kind":"line","file_id":"file:1","hunk_id":"hunk:1","side":"new","line":12,"context_hash":""},"severity":"question","title":"","body":"Why does this retry twice?","proposed_fix":null,"send":true}}"#,
        r#"{"kind":"record_review_progress","content":{"review_id":"review:1","walked_chapter_ids":["chapter:1"]}}"#,
        r#"{"kind":"record_review_seen","content":{"review_id":"review:1"}}"#,
        r#"{"kind":"refresh_guided_review","content":{"request_id":"review-load:2","review_id":"review:1","revision_id":"revision:1"}}"#,
        r#"{"kind":"commit_review_chapters","content":{"request_id":"review-save:2","review_id":"review:1","revision_id":"revision:1","squash":true}}"#,
        r#"{"kind":"prepare_pull_request","content":{"request_id":"pr-open:1","review_id":"review:1"}}"#,
        r#"{"kind":"open_pull_request","content":{"request_id":"pr-open:1","title":"Retry once","body":"Why.","draft":false}}"#,
        r#"{"kind":"prepare_stack","content":{"request_id":"pr-open:2","review_id":"review:1"}}"#,
        r#"{"kind":"open_stack","content":{"request_id":"pr-open:2","draft":true}}"#,
        r#"{"kind":"prepare_pull_request_post","content":{"request_id":"pr-post:1","review_id":"review:1"}}"#,
        r#"{"kind":"post_pull_request_review","content":{"request_id":"pr-post:1","verdict":"request_changes","session_revision":4}}"#,
        r#"{"kind":"prepare_thread_answers","content":{"request_id":"thread-answers:1","review_id":"review:1"}}"#,
        r#"{"kind":"answer_threads","content":{"request_id":"thread-answers:1","review_id":"review:1","answers":[{"finding_id":"finding:1","reply":"Fixed in the last push.","resolve":true}]}}"#,
        r#"{"kind":"list_github","content":{"sections":[{"name":"Mine","query":"is:open is:pr author:@me repo:{repo}"}]}}"#,
        r#"{"kind":"search_github","content":{"request":7,"section":{"name":"Mine","query":"is:open is:pr author:@me repo:{repo}"},"after":"Y3Vyc29yOjMw"}}"#,
        r#"{"kind":"read_github_overview","content":{"number":142,"pull_request":true}}"#,
        r#"{"kind":"read_github_conversation","content":{"repository":"acme/app","number":9,"pull_request":false}}"#,
        r#"{"kind":"draft_github_reply","content":{"request_id":"draft:1","repository":null,"number":142,"pull_request":true,"target":{"kind":"comment","id":"IC_1","author":"mara","bot":false,"mine":false,"body":"Does this handle Windows paths?","created_at":"2026-08-20T10:00:00Z","url":"https://github.com/acme/app/pull/142#issuecomment-1"},"conversation":[],"finding":null,"note":null,"current":null}}"#,
        r#"{"kind":"check_pull_request_fix","content":{"number":142}}"#,
        r#"{"kind":"plan_restack","content":{"request_id":"restack:1"}}"#,
        r#"{"kind":"check_worktree_removal"}"#,
        r#"{"kind":"load_file_diff","content":{"request_id":"file-diff:1","path":"src/main.rs"}}"#,
        r#"{"kind":"list_directory","content":{"request_id":"listing:1","path":"src"}}"#,
        r#"{"kind":"read_project_file","content":{"request_id":"file-read:1","path":"src/main.rs"}}"#,
        r#"{"kind":"set_focus","content":{"focused":false}}"#,
        r#"{"kind":"link_pull_request","content":{"number":142,"url":"https://github.com/acme/app/pull/142"}}"#,
        r#"{"kind":"post_github_comment","content":{"repository":null,"number":142,"thread":"PRRT_1","answering":null,"body":"Fixed in the latest push."}}"#,
        r#"{"kind":"change_pull_request","content":{"number":142,"write":{"change":"merge","method":"squash","head_oid":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","delete_branch":"feat/threads"}}}"#,
        r#"{"kind":"restack","content":{"request_id":"restack:1"}}"#,
        r#"{"kind":"close"}"#,
    ];
    let samples = sample_actions();
    assert_eq!(
        samples.len(),
        PINNED.len(),
        "a wire action has no pinned literal — add one copied from real output"
    );
    for (action, pinned) in samples.iter().zip(PINNED) {
        let tag = action.kind();
        assert_eq!(
            serde_json::to_string(action).unwrap().as_str(),
            *pinned,
            "{tag} no longer serializes as its published form"
        );
        assert_eq!(
            &serde_json::from_str::<Action>(pinned).unwrap(),
            action,
            "{tag} does not read back the form it writes"
        );
    }
}

/// `/compact` with no steer is the common case, so a client may omit the payload entirely.
#[test]
fn compact_now_accepts_a_frame_with_no_focus() {
    for frame in [
        r#"{"kind":"compact_now"}"#,
        r#"{"kind":"compact_now","content":null}"#,
        r#"{"kind":"compact_now","content":{}}"#,
    ] {
        assert_eq!(
            serde_json::from_str::<Action>(frame).expect(frame),
            Action::CompactNow { focus: None },
            "{frame}"
        );
    }
}

/// A bare `list_github` asks for the configured sections, the way the dock's `/github` does.
#[test]
fn list_github_accepts_a_frame_with_no_sections() {
    for frame in [
        r#"{"kind":"list_github"}"#,
        r#"{"kind":"list_github","content":null}"#,
        r#"{"kind":"list_github","content":{}}"#,
    ] {
        assert_eq!(
            serde_json::from_str::<Action>(frame).expect(frame),
            Action::ListGithub { sections: None },
            "{frame}"
        );
    }
}

/// Exactly the actions that publish to GitHub are writes, and committing is the one write that
/// stays local.
///
/// This is the list `--github-writes` gates, so both directions matter: a write classified as
/// a read gets through a session that never opted in, and a read classified as a write is
/// refused for no reason. Pinned by kind so the answer cannot drift with the samples.
#[test]
fn only_publishing_to_github_is_a_github_write() {
    let writes: Vec<&str> = sample_actions()
        .iter()
        .filter(|action| action.writes_to_github())
        .map(Action::kind)
        .collect();
    assert_eq!(
        writes,
        [
            "open_pull_request",
            "open_stack",
            "post_pull_request_review",
            "answer_threads",
            "post_github_comment",
            "change_pull_request",
            "restack",
        ]
    );
    let local: Vec<&str> = sample_actions()
        .iter()
        .filter(|action| action.writes_the_repository())
        .map(Action::kind)
        .collect();
    assert_eq!(local, ["commit_review_chapters"], "the one local write read-only refuses");
    assert!(!Action::Other { kind: "merge".into(), content: serde_json::Value::Null }
        .writes_to_github());
}

/// What a hand-written client leaves out still parses: a comment needs a review, a place and
/// words, and every edit kind reads with only what it needs.
///
/// The break this catches: a field that serializes and so round-trips in the pinned test, but
/// that a client never sends, made required by accident.
#[test]
fn an_edit_needs_only_what_it_is_about() {
    use crate::engine::ReviewEdit;
    let parse = |content: serde_json::Value| -> Action {
        serde_json::from_value(serde_json::json!({ "kind": "edit_review", "content": content }))
            .unwrap_or_else(|error| panic!("{error}"))
    };
    let Action::EditReview { edit: ReviewEdit::Comment { draft, send }, .. } =
        parse(serde_json::json!({
            "request_id": "review-save:1", "review_id": "review:1", "edit": "comment",
            "anchor": { "kind": "line", "file_id": "file:1", "hunk_id": "hunk:1",
                        "side": "new", "line": 3 },
            "body": "Keep the old value."
        }))
    else {
        panic!("a comment");
    };
    assert_eq!(draft.severity, crate::guided_review::FindingSeverity::Issue, "an issue by default");
    assert!(draft.title.is_empty() && draft.proposed_fix.is_none() && !send);
    for (edit, extra) in [
        ("reply", serde_json::json!({ "finding_id": "finding:1", "body": "Still wrong." })),
        ("ask_for_fix", serde_json::json!({ "finding_id": "finding:1", "body": "Fix it." })),
        ("resolve", serde_json::json!({ "finding_id": "finding:1" })),
        ("reopen", serde_json::json!({ "finding_id": "finding:1" })),
        ("delete_comment", serde_json::json!({ "finding_id": "finding:1" })),
        ("set_outcome", serde_json::json!({ "outcome": "accepted" })),
        ("send", serde_json::json!({ "finding_ids": ["finding:1"] })),
        ("cancel_send", serde_json::json!({ "submission_id": "submission:1" })),
        ("retry_send", serde_json::json!({ "submission_id": "submission:1" })),
    ] {
        let mut content = serde_json::json!({
            "request_id": "review-save:1", "review_id": "review:1", "edit": edit
        });
        content.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        assert!(matches!(parse(content), Action::EditReview { .. }), "{edit}");
    }
    // Handing a review's comments to a new session is the interface's, not a client's
    let adopt = serde_json::json!({ "kind": "edit_review", "content": {
        "request_id": "review-save:1", "review_id": "review:1", "edit": "adopt_carried",
        "findings": [], "send": false
    }});
    assert!(serde_json::from_value::<Action>(adopt).is_err());
}

/// The published kind list is exactly the variants a client can send.
///
/// [`Action::kind`] is the part the compiler holds; this is what holds [`Action::KINDS`] and
/// [`sample_actions`] to it. Without it a variant could be added, given a `kind`, and never
/// reach either the published list or the pinned literals — and `luxe-cli`'s translator, which
/// cannot match this enum exhaustively across the crate boundary, would never learn of it.
/// `other` is excluded on purpose: it is the fallback a client never writes deliberately, and
/// `unknown_variants_preserve_the_original_payload` covers it.
#[test]
fn the_published_kind_list_matches_the_variants() {
    let sampled: Vec<&str> = sample_actions().iter().map(Action::kind).collect();
    assert_eq!(sampled, Action::KINDS, "a variant is missing from KINDS or from the samples");
    assert!(!Action::KINDS.contains(&"other"), "`other` is a fallback, not a client intent");
    assert_eq!(
        Action::KINDS.len(),
        std::collections::BTreeSet::from_iter(Action::KINDS.iter()).len(),
        "a kind is listed twice"
    );
    assert_eq!(
        Action::kind(&Action::Other { kind: "future".into(), content: serde_json::Value::Null }),
        "other",
        "an unknown frame reports the category, not a tag this build invented"
    );
}

/// A model's display name is what a front-end shows, and falls back to the raw id.
///
/// Every `state_changed` carries one, so this is a published string, not a UI detail. The break
/// it catches: returning the id when an override was set (the picker's chosen label silently
/// reverts to `claude-sonnet-4-20250514`), or returning an override that is not there.
#[test]
fn a_model_shows_its_override_and_falls_back_to_its_id() {
    let plain = ModelRef::new("anthropic", "claude-sonnet-4-20250514");
    assert_eq!(plain.display(), "claude-sonnet-4-20250514");

    let labeled = plain.clone().with_display(Some("Claude Sonnet 4".into()));
    assert_eq!(labeled.display(), "Claude Sonnet 4");
    assert_eq!(labeled.model, "claude-sonnet-4-20250514", "the label never replaces the id");

    assert_eq!(
        plain.with_display(None).display(),
        "claude-sonnet-4-20250514",
        "clearing the label returns the id, not an empty string"
    );
}

/// A review failure is classified into the kind a client recovers by, one case per branch.
///
/// `guided_review_failed` carries this `kind`, and the three mean different things to whoever
/// receives them: `unavailable` says retrying will not help, `invalid_target` says the ref was
/// wrong, `capture` says the attempt failed and may be worth repeating. Collapsing any two of
/// them — which is what deleting either match arm does — tells a client to retry something
/// that can never succeed, and no round-trip test would notice.
#[test]
fn a_review_failure_reports_the_kind_a_client_recovers_by() {
    use crate::guided_review::{GuidedReviewError, ReviewSourceUnavailable};

    let unavailable = GuidedReviewFailure::from_error(&GuidedReviewError::Unavailable(
        ReviewSourceUnavailable::MissingTurnBaseline,
    ));
    assert_eq!(unavailable.kind, GuidedReviewFailureKind::Unavailable);

    let invalid = GuidedReviewFailure::from_error(&GuidedReviewError::InvalidReference {
        reference: "origin/nope".into(),
    });
    assert_eq!(invalid.kind, GuidedReviewFailureKind::InvalidTarget);
    assert!(invalid.message.contains("origin/nope"), "{}", invalid.message);

    let capture = GuidedReviewFailure::from_error(&GuidedReviewError::MalformedGitOutput {
        operation: "diff",
        reason: "unreadable output",
    });
    assert_eq!(capture.kind, GuidedReviewFailureKind::Capture, "anything else is a capture");
}

/// A failure message is bounded and stripped of control characters before it reaches the wire.
///
/// It is built from a subprocess error, so it is the one string here that an attacker-adjacent
/// source contributes to. Newlines and tabs survive because a reader needs them; a terminal
/// escape does not.
#[test]
fn a_review_failure_message_cannot_carry_control_characters_or_run_long() {
    use crate::guided_review::GuidedReviewError;

    let failure = GuidedReviewFailure::from_error(&GuidedReviewError::InvalidReference {
        reference: format!("\u{1b}[31mred\u{7}\ttab\nline{}", "x".repeat(1_000)),
    });
    assert!(!failure.message.contains('\u{1b}'), "an escape sequence reached the wire");
    assert!(!failure.message.contains('\u{7}'), "a bell reached the wire");
    assert!(failure.message.contains('\t') && failure.message.contains('\n'));
    assert_eq!(failure.message.chars().count(), 512, "the message is capped");
}

#[test]
fn stop_reason_roundtrip() {
    for reason in [StopReason::EndTurn, StopReason::ToolCall, StopReason::Other("timeout".into())] {
        roundtrip("stop reason", &reason);
    }
}

#[test]
fn engine_state_roundtrip() {
    let s = EngineState {
        model: ModelRef::new("openai", "gpt-4o"),
        streaming: true,
        status: "running…".into(),
        mode: crate::mode::PLAN.into(),
        thinking: crate::provider::ThinkingLevel::High,
    };
    roundtrip("engine state", &s);
}

#[test]
fn engine_state_default_roundtrip() {
    let s = EngineState::default();
    roundtrip("engine state default", &s);
}

#[test]
fn engine_state_without_thinking_defaults_off() {
    let mut value = serde_json::to_value(EngineState::default()).unwrap();
    value.as_object_mut().unwrap().remove("thinking");
    let state: EngineState = serde_json::from_value(value).unwrap();
    assert_eq!(state.thinking, crate::provider::ThinkingLevel::Off);
}

#[test]
fn event_token_delta_roundtrip() {
    let e = Event::TokenDelta { block: 0, delta: "Hello".into() };
    roundtrip("event token delta", &e);
}

#[test]
fn event_tool_started_roundtrip() {
    let e = Event::ToolStarted { call_id: "c1".into(), name: "bash".into() };
    roundtrip("event tool started", &e);
}

#[test]
fn event_message_committed_roundtrip() {
    let message = Message::assistant("I'll help.".to_string());
    let e = Event::MessageCommitted {
        session_id: "session-1".into(),
        entry_id: EntryId("entry-1".into()),
        message,
    };
    roundtrip("event message committed", &e);
}

#[test]
fn event_remediation_reported_roundtrip() {
    let report = RemediationReport {
        review_id: ReviewId::new(),
        revision_id: ReviewRevisionId::from_canonical_bytes(b"review"),
        findings: vec![crate::guided_review::RemediationResult {
            finding_id: crate::guided_review::FindingId::new(),
            status: crate::guided_review::RemediationStatus::Fixed,
            note: Some("Added a bound.".into()),
        }],
    };
    roundtrip(
        "event remediation reported",
        &Event::RemediationReported { entry_id: EntryId("entry-1".into()), report },
    );
}

#[test]
fn event_review_feedback_turn_dispatched_roundtrip() {
    roundtrip(
        "event review feedback turn dispatched",
        &Event::ReviewFeedbackTurnDispatched { submission_id: SubmissionId::new() },
    );
}

#[test]
fn event_review_feedback_turn_failed_roundtrip() {
    roundtrip(
        "event review feedback turn failed",
        &Event::ReviewFeedbackTurnFailed {
            submission_id: SubmissionId::new(),
            message: "provider connection lost".into(),
        },
    );
}

#[test]
fn event_review_feedback_turn_completed_roundtrip() {
    roundtrip(
        "event review feedback turn completed",
        &Event::ReviewFeedbackTurnCompleted { submission_id: SubmissionId::new() },
    );
}

#[test]
fn event_session_loaded_roundtrip() {
    let e = Event::SessionLoaded {
        session_id: "session-1".into(),
        messages: vec![
            SessionMessage { entry_id: EntryId("entry-1".into()), message: Message::user("hello") },
            SessionMessage {
                entry_id: EntryId("entry-2".into()),
                message: Message::assistant("hi"),
            },
        ],
    };
    roundtrip("event session loaded", &e);
}

#[test]
fn event_turn_ended_roundtrip() {
    let e = Event::TurnEnded { reason: StopReason::EndTurn };
    roundtrip("event turn ended", &e);
}

#[test]
fn event_compaction_done_roundtrip() {
    let e = Event::CompactionDone { tokens_before: 4096, tokens_after: 1024 };
    roundtrip("event compaction done", &e);
}

#[test]
fn event_workspace_updated_roundtrip() {
    let snapshot = WorkspaceSnapshot::empty(
        crate::workspace::WorkspaceScopeId::new("scope-1"),
        crate::workspace::ProjectIdentity::non_git("/work/luxe".into(), "luxe", "~/work/luxe"),
    );
    roundtrip("event workspace updated", &Event::WorkspaceUpdated(Box::new(snapshot)));
}

#[test]
fn event_tool_activity_updated_roundtrip() {
    let activity = ToolActivity::running("session-1", "call-1", "read", "Read Cargo.toml", 42);
    roundtrip("event tool activity updated", &Event::ToolActivityUpdated(Box::new(activity)));
}

#[test]
fn event_error_roundtrip() {
    let e = Event::Error("connection lost".into());
    roundtrip("event error", &e);
}

#[test]
fn event_context_usage_roundtrip() {
    let e = Event::ContextUsage { used_tokens: 12_000, context_window: 128_000 };
    roundtrip("event context usage", &e);
}

/// The four arms this audit added, round-tripped for payload rather than tag.
///
/// `every_event_tag_has_a_deserializer_arm` proves a tag is *recognized*; it cannot prove the
/// arm carries anything. These two events are the ones a person has to act on — a front-end
/// that received them stripped would show an approval prompt with no command in it.
#[test]
fn the_interactive_events_carry_what_the_user_must_act_on() {
    let asked = Event::PermissionRequested {
        id: PermissionRequestId::new(),
        session_id: "s1".into(),
        tool: "bash".into(),
        permission: "bash".into(),
        summary: "Run: rm -rf build".into(),
        always_patterns: vec!["rm *".into()],
    };
    let decoded: Event = serde_json::from_str(&serde_json::to_string(&asked).unwrap()).unwrap();
    assert_eq!(decoded, asked, "the approval prompt must survive the wire intact");

    let question = Event::QuestionAsked {
        id: crate::question::QuestionRequestId::new(),
        session_id: "s1".into(),
        prompt: "Which database?".into(),
        options: Vec::new(),
        multi: true,
    };
    let decoded: Event = serde_json::from_str(&serde_json::to_string(&question).unwrap()).unwrap();
    assert_eq!(decoded, question);

    // A client reads this one by its fields, so the shape is pinned, not just round-tripped
    let retrying = Event::ProviderRetrying { attempt: 1, attempts: 3, reason: "busy".into() };
    assert_eq!(
        serde_json::to_string(&retrying).unwrap(),
        r#"{"kind":"provider_retrying","content":{"attempt":1,"attempts":3,"reason":"busy"}}"#
    );
    for event in [Event::Notice("heads up".into()), Event::SteeringDelivered { count: 3 }, retrying]
    {
        let decoded: Event = serde_json::from_str(&serde_json::to_string(&event).unwrap()).unwrap();
        assert_eq!(decoded, event);
    }
}

/// Every variant, sampled, serializes under a tag on the published list and decodes back to the
/// same payload; and every published tag has a sample.
///
/// The break this catches: the list was checked against a hard-coded length and `tag_of` fed it
/// one variant, so `provider_retrying` sat in neither and nothing noticed. Now a sampled variant
/// missing from the list fails here, and so does an arm that drops its payload.
#[test]
fn every_event_round_trips_under_a_published_tag() {
    let samples = samples::every_event();
    let mut tags: Vec<&str> = samples.iter().map(tag_of).collect();
    for sample in &samples {
        let wire = serde_json::to_string(sample).expect("serializes");
        let decoded = serde_json::from_str::<Event>(&wire);
        assert_eq!(decoded.as_ref().ok(), Some(sample), "{} lost its payload", tag_of(sample));
    }
    tags.sort_unstable();
    let count = tags.len();
    tags.dedup();
    assert_eq!(tags.len(), count, "one sample per variant");
    let mut published = EVENT_TAGS.to_vec();
    published.sort_unstable();
    assert_eq!(tags, published, "the published tags and the sampled variants differ");
}
