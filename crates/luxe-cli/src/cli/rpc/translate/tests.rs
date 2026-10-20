use super::*;
use luxe_core::provider::{ModelInfo, ThinkingLevel};

/// A catalog that knows one model and falls back for everything else, like the real one.
struct OneModel;

impl ModelResolver for OneModel {
    fn resolve(&self, provider: &str, id: &str) -> ModelInfo {
        let known = provider == "anthropic" && id == "claude-sonnet-4-20250514";
        ModelInfo {
            r#ref: luxe_core::provider::ModelRef::new(provider, id),
            context_window: if known { 200_000 } else { 128_000 },
            ..Default::default()
        }
    }

    fn choices(&self) -> Vec<luxe_core::event::ModelChoice> {
        Vec::new()
    }
}

fn policy(github_writes: GithubWrites) -> Policy {
    Policy {
        github_writes,
        github_sections: vec![luxe_core::config::GithubSection {
            name: "Mine".into(),
            query: "is:open is:pr author:@me repo:{repo}".into(),
        }],
        joined: false,
    }
}

fn translate(wire: WireAction) -> Result<Action, Rejected> {
    to_engine_action(wire, &OneModel, &policy(GithubWrites::Allowed))
}

/// A client that joined a terminal's session can't switch it, and one that started its own can.
///
/// The break this catches: dropping the joined check, which lets a phone swap out the
/// conversation the person at the terminal is reading.
#[test]
fn a_joined_client_cannot_switch_the_session() {
    let joined = Policy { joined: true, ..policy(GithubWrites::Allowed) };
    for wire in [WireAction::NewSession, WireAction::LoadSession("other".into())] {
        let refused = to_engine_action(wire.clone(), &OneModel, &joined);
        assert!(refused.is_err(), "a joined client ran {}", wire.kind());
        assert!(translate(wire).is_ok());
    }
}

/// A steer for the summary has to survive the crossing; without it `/compact keep the API
/// decisions` is an ordinary compaction that silently ignored what it was told.
#[test]
fn compact_now_carries_its_focus() {
    assert!(matches!(
        translate(WireAction::CompactNow { focus: Some("the API decisions".into()) }),
        Ok(Action::CompactNow { focus: Some(focus) }) if focus == "the API decisions"
    ));
    assert!(matches!(
        translate(WireAction::CompactNow { focus: None }),
        Ok(Action::CompactNow { focus: None })
    ));
}

/// Every wire action reaches the engine, and reaches it carrying its payload.
///
/// The break this catches: an arm wired to the wrong engine variant, or one that drops a field
/// on the way. `matches!` on the variant alone would pass through a `steer_turn` translated as
/// an empty submission — which is a new turn instead of a mid-turn nudge, the one distinction
/// the action exists to make.
#[test]
fn each_wire_action_arrives_as_the_engine_action_it_names() {
    assert!(matches!(
        translate(WireAction::SubmitUserMessage {
            text: "hello".into(),
            images: vec![luxe_core::engine::ImageAttachment {
                data: "aGk=".into(),
                mime: "image/png".into(),
                name: None,
            }],
        }),
        Ok(Action::SubmitUserMessage { content, images, remediation: None })
            if content == "hello" && images.len() == 1
    ));
    assert!(matches!(
        translate(WireAction::SteerTurn { text: "also windows".into() }),
        Ok(Action::SteerTurn { content }) if content == "also windows"
    ));
    assert!(matches!(translate(WireAction::Cancel), Ok(Action::Cancel)));
    assert!(matches!(
        translate(WireAction::EditResend { text: "shorter".into() }),
        Ok(Action::EditResend { content }) if content == "shorter"
    ));
    assert!(matches!(translate(WireAction::Retry), Ok(Action::Retry)));
    assert!(matches!(translate(WireAction::Regenerate), Ok(Action::Regenerate)));
    assert!(matches!(
        translate(WireAction::SetPermissionAutoApprove(true)),
        Ok(Action::SetPermissionAutoApprove(true))
    ));
    assert!(matches!(
        translate(WireAction::SetThinkingLevel(ThinkingLevel::High)),
        Ok(Action::SetThinkingLevel(ThinkingLevel::High))
    ));
    assert!(matches!(translate(WireAction::NewSession), Ok(Action::NewSession)));
    assert!(matches!(
        translate(WireAction::LoadSession("session-1".into())),
        Ok(Action::LoadSession(id)) if id == "session-1"
    ));
    assert!(matches!(translate(WireAction::RefreshSessions), Ok(Action::RefreshSessions)));
    assert!(matches!(translate(WireAction::RefreshWorkspace), Ok(Action::RefreshWorkspace)));
    assert!(matches!(
        translate(WireAction::RequestContextComposition),
        Ok(Action::RequestContextComposition)
    ));
    assert!(matches!(translate(WireAction::Close), Ok(Action::Close)));
    assert!(matches!(
        translate(WireAction::SetMode("plan".into())),
        Ok(Action::SetMode(mode)) if mode == "plan"
    ));
    assert!(matches!(
        translate(WireAction::RequestReview { focus: Some("the parser".into()) }),
        Ok(Action::RequestReview { focus: Some(focus) }) if focus == "the parser"
    ));
}

/// The client that started a session is refused when it introduces itself, by name.
///
/// The break this catches: the arm falling to the catch-all, which tells a client that a published
/// kind is unknown.
#[test]
fn the_starting_client_cannot_introduce_itself() {
    let introduce = WireAction::Introduce { name: "iPhone".into(), watch_only: false, yours: true };
    let refused = translate(introduce).unwrap_err();
    assert_eq!(refused.kind.as_deref(), Some("introduce"));
    assert!(refused.reason.contains("joined a running session"), "{}", refused.reason);
}

/// Every GitHub action reaches the engine as the action it names, carrying its payload.
///
/// Same break as above, sharpest here: overview and conversation share a payload, as do
/// `plan_restack` and `restack`, so a crossed arm still type-checks — and `restack` arriving as
/// a plan, or a plan arriving as a push, is the difference between looking and force-pushing.
#[test]
fn each_github_action_arrives_as_the_engine_action_it_names() {
    let restack: luxe_core::restack::RestackId =
        serde_json::from_value(serde_json::json!("restack:1")).unwrap();
    assert!(matches!(
        translate(WireAction::PlanRestack { request_id: restack.clone() }),
        Ok(Action::PlanRestack { request_id }) if request_id == restack
    ));
    assert!(matches!(
        translate(WireAction::Restack { request_id: restack.clone() }),
        Ok(Action::Restack { request_id }) if request_id == restack
    ));
    assert!(matches!(
        translate(WireAction::ReadGithubOverview {
            repository: Some("acme/app".into()),
            number: 9,
            pull_request: false,
        }),
        Ok(Action::ReadGithubOverview { repository: Some(repository), number: 9, pull_request: false })
            if repository == "acme/app"
    ));
    assert!(matches!(
        translate(WireAction::ReadGithubConversation {
            repository: None,
            number: 142,
            pull_request: true,
        }),
        Ok(Action::ReadGithubConversation { repository: None, number: 142, pull_request: true })
    ));
    let section =
        luxe_core::config::GithubSection { name: "Bugs".into(), query: "is:open label:bug".into() };
    assert!(matches!(
        translate(WireAction::SearchGithub {
            request: 7,
            section: section.clone(),
            after: Some("cursor".into()),
        }),
        Ok(Action::SearchGithub { request: 7, section: asked, after: Some(after) })
            if asked == section && after == "cursor"
    ));
    assert!(matches!(
        translate(WireAction::CheckPullRequestFix { number: 142, repository: None }),
        Ok(Action::CheckPullRequestFix { number: 142, repository: None })
    ));
    assert!(matches!(
        translate(WireAction::CheckWorktreeRemoval),
        Ok(Action::CheckWorktreeRemoval)
    ));
    let diff: luxe_core::file_diff::FileDiffRequestId =
        serde_json::from_value(serde_json::json!("file-diff:1")).unwrap();
    assert!(matches!(
        translate(WireAction::LoadFileDiff { request_id: diff.clone(), path: "src/a.rs".into() }),
        Ok(Action::LoadFileDiff { request_id, path }) if request_id == diff && path == "src/a.rs"
    ));
    let listing = luxe_core::project_files::ListingRequestId::new();
    assert!(matches!(
        translate(WireAction::ListDirectory {
            request_id: listing.clone(),
            path: "src".into(),
            include_ignored: true,
        }),
        Ok(Action::ListDirectory { request_id, path, include_ignored: true })
            if request_id == listing && path == "src"
    ));
    let read = luxe_core::project_files::FileReadRequestId::new();
    assert!(matches!(
        translate(WireAction::ReadProjectFile { request_id: read.clone(), path: "a.rs".into() }),
        Ok(Action::ReadProjectFile { request_id, path }) if request_id == read && path == "a.rs"
    ));
    assert!(matches!(
        translate(WireAction::SetFocus { focused: false }),
        Ok(Action::FocusChanged { focused: false })
    ));
    assert!(matches!(
        translate(WireAction::LinkPullRequest { number: 142, url: "u".into() }),
        Ok(Action::LinkPullRequest { number: 142, url }) if url == "u"
    ));
    let post = luxe_core::engine::GithubPost {
        repository: None,
        number: 142,
        thread: Some("PRRT_1".into()),
        answering: None,
        body: "Fixed.".into(),
    };
    assert!(matches!(
        translate(WireAction::PostGithubComment(post.clone())),
        Ok(Action::PostGithubComment(sent)) if sent == post
    ));
    let write = luxe_core::engine::PullRequestWrite::MarkReady;
    assert!(matches!(
        translate(WireAction::ChangePullRequest { repository: None, number: 142, write }),
        Ok(Action::ChangePullRequest {
            repository: None,
            number: 142,
            write: luxe_core::engine::PullRequestWrite::MarkReady,
        })
    ));
}

/// Every review action reaches the engine as the action it names, carrying its payload.
///
/// Same break as above. The pairs that share a payload are the ones to watch: `prepare_*` and
/// its `open_*`, and the revision a refresh or a commit is guarded by, which arrives renamed.
#[test]
fn each_review_action_arrives_as_the_engine_action_it_names() {
    use luxe_core::guided_review::{ReviewId, ReviewRevisionId};
    let id = |raw: &str| -> ReviewId { serde_json::from_value(serde_json::json!(raw)).unwrap() };
    let revision: ReviewRevisionId =
        serde_json::from_value(serde_json::json!("revision:1")).unwrap();
    let open: luxe_core::guided_review::PullRequestOpenId =
        serde_json::from_value(serde_json::json!("pr-open:1")).unwrap();
    let post: luxe_core::guided_review::PullRequestPostId =
        serde_json::from_value(serde_json::json!("pr-post:1")).unwrap();
    let save: luxe_core::guided_review::ReviewSessionSaveRequestId =
        serde_json::from_value(serde_json::json!("review-save:1")).unwrap();
    let load: luxe_core::guided_review::ReviewLoadRequestId =
        serde_json::from_value(serde_json::json!("review-load:1")).unwrap();
    let edit = luxe_core::engine::ReviewEdit::SetOutcome {
        outcome: luxe_core::guided_review::ReviewOutcome::Accepted,
    };
    assert!(matches!(
        translate(WireAction::EditReview { request_id: save.clone(), review_id: id("review:1"), edit: edit.clone() }),
        Ok(Action::EditReview { request_id, review_id, edit: sent })
            if request_id == save && review_id == id("review:1") && sent == edit
    ));
    let chapter: luxe_core::guided_review::ChapterId =
        serde_json::from_value(serde_json::json!("chapter:1")).unwrap();
    assert!(matches!(
        translate(WireAction::RecordReviewProgress {
            review_id: id("review:1"),
            walked_chapter_ids: vec![chapter.clone()],
        }),
        Ok(Action::RecordReviewProgress { progress, .. })
            if progress.walked_chapter_ids == vec![chapter]
    ));
    assert!(matches!(
        translate(WireAction::RecordReviewSeen { review_id: id("review:1") }),
        Ok(Action::RecordReviewSeen { .. })
    ));
    assert!(matches!(
        translate(WireAction::ShowOpenReview { request_id: load.clone() }),
        Ok(Action::ShowOpenReview { request_id }) if request_id == load
    ));
    assert!(matches!(
        translate(WireAction::RefreshGuidedReview {
            request_id: load.clone(),
            review_id: id("review:1"),
            revision_id: revision.clone(),
        }),
        Ok(Action::RefreshGuidedReview { request_id, expected_revision, .. })
            if request_id == load && expected_revision == revision
    ));
    assert!(matches!(
        translate(WireAction::CommitReviewChapters {
            request_id: save.clone(),
            review_id: id("review:1"),
            revision_id: revision.clone(),
            chapter_ids: Vec::new(),
            squash: true,
        }),
        Ok(Action::CommitReviewChapters { expected_revision, squash: true, .. })
            if expected_revision == revision
    ));
    assert!(matches!(
        translate(WireAction::PreparePullRequest { request_id: open.clone(), review_id: id("review:1") }),
        Ok(Action::PreparePullRequest { request_id, .. }) if request_id == open
    ));
    assert!(matches!(
        translate(WireAction::OpenPullRequest {
            request_id: open.clone(),
            title: "T".into(),
            body: "B".into(),
            draft: true,
        }),
        Ok(Action::OpenPullRequest { request_id, title, body, draft: true })
            if request_id == open && title == "T" && body == "B"
    ));
    assert!(matches!(
        translate(WireAction::PrepareStack { request_id: open.clone(), review_id: id("review:1") }),
        Ok(Action::PrepareStack { .. })
    ));
    assert!(matches!(
        translate(WireAction::OpenStack { request_id: open.clone(), draft: true }),
        Ok(Action::OpenStack { draft: true, .. })
    ));
    assert!(matches!(
        translate(WireAction::PreparePullRequestPost {
            request_id: post.clone(),
            review_id: id("review:1")
        }),
        Ok(Action::PreparePullRequestPost { .. })
    ));
    assert!(matches!(
        translate(WireAction::PostPullRequestReview {
            request_id: post.clone(),
            verdict: luxe_core::github::ReviewEvent::Approve,
            session_revision: 3,
        }),
        Ok(Action::PostPullRequestReview {
            verdict: luxe_core::github::ReviewEvent::Approve,
            session_revision: 3,
            ..
        })
    ));
    let threads: luxe_core::guided_review::ThreadAnswersId =
        serde_json::from_value(serde_json::json!("thread-answers:1")).unwrap();
    assert!(matches!(
        translate(WireAction::PrepareThreadAnswers {
            request_id: threads.clone(),
            review_id: id("review:1")
        }),
        Ok(Action::PrepareThreadAnswers { .. })
    ));
    assert!(matches!(
        translate(WireAction::AnswerThreads {
            request_id: threads,
            review_id: id("review:1"),
            answers: Vec::new(),
        }),
        Ok(Action::AnswerThreads { .. })
    ));
}

/// Committing a review's chapters writes history without publishing it: allowed without
/// `--github-writes`, refused in a read-only session.
#[test]
fn a_read_only_session_refuses_to_commit_a_review() {
    let commit = || WireAction::CommitReviewChapters {
        request_id: serde_json::from_value(serde_json::json!("review-save:1")).unwrap(),
        review_id: serde_json::from_value(serde_json::json!("review:1")).unwrap(),
        revision_id: serde_json::from_value(serde_json::json!("revision:1")).unwrap(),
        chapter_ids: Vec::new(),
        squash: false,
    };
    assert!(to_engine_action(commit(), &OneModel, &policy(GithubWrites::NotEnabled)).is_ok());
    let refused = to_engine_action(commit(), &OneModel, &policy(GithubWrites::ReadOnly))
        .expect_err("read-only writes nothing");
    assert_eq!(refused.kind.as_deref(), Some("commit_review_chapters"));
    assert!(refused.reason.contains("read-only"), "{}", refused.reason);
}

/// A bare `list_github` runs the configured sections, and named ones replace them.
#[test]
fn list_github_falls_back_to_the_configured_sections() {
    let Ok(Action::ListGithub { sections }) = translate(WireAction::ListGithub { sections: None })
    else {
        panic!("expected ListGithub");
    };
    assert_eq!(sections, policy(GithubWrites::Allowed).github_sections);

    let asked = vec![luxe_core::config::GithubSection {
        name: "Bugs".into(),
        query: "is:open label:bug".into(),
    }];
    let Ok(Action::ListGithub { sections }) =
        translate(WireAction::ListGithub { sections: Some(asked.clone()) })
    else {
        panic!("expected ListGithub");
    };
    assert_eq!(sections, asked);
}

/// GitHub writes need `--github-writes`, a read-only session refuses them regardless, and
/// reads are never gated.
///
/// Both directions are the point. A write that slipped through publishes on behalf of a client
/// nobody allowed to; a read caught by the gate breaks every dashboard that only looks.
#[test]
fn github_writes_need_the_flag_and_read_only_refuses_them_anyway() {
    let writes = || {
        [
            WireAction::PostGithubComment(luxe_core::engine::GithubPost {
                repository: None,
                number: 1,
                thread: None,
                answering: None,
                body: "hi".into(),
            }),
            WireAction::ChangePullRequest {
                repository: None,
                number: 1,
                write: luxe_core::engine::PullRequestWrite::MarkReady,
            },
            WireAction::Restack { request_id: luxe_core::restack::RestackId::new() },
            WireAction::OpenPullRequest {
                request_id: luxe_core::guided_review::PullRequestOpenId::new(),
                title: "T".into(),
                body: "B".into(),
                draft: false,
            },
            WireAction::OpenStack {
                request_id: luxe_core::guided_review::PullRequestOpenId::new(),
                draft: false,
            },
            WireAction::PostPullRequestReview {
                request_id: luxe_core::guided_review::PullRequestPostId::new(),
                verdict: luxe_core::github::ReviewEvent::Comment,
                session_revision: 0,
            },
            WireAction::AnswerThreads {
                request_id: luxe_core::guided_review::ThreadAnswersId::new(),
                review_id: luxe_core::guided_review::ReviewId::new(),
                answers: Vec::new(),
            },
        ]
    };
    for (gate, needle) in [
        (GithubWrites::NotEnabled, "start Luxe with --github-writes"),
        (GithubWrites::ReadOnly, "read-only"),
    ] {
        for wire in writes() {
            let kind = wire.kind();
            let rejected = to_engine_action(wire, &OneModel, &policy(gate))
                .expect_err("a write is refused without permission");
            assert_eq!(rejected.kind.as_deref(), Some(kind));
            assert!(rejected.reason.contains(needle), "{gate:?}: {}", rejected.reason);
        }
        let read =
            WireAction::ReadGithubOverview { repository: None, number: 1, pull_request: true };
        assert!(to_engine_action(read, &OneModel, &policy(gate)).is_ok(), "{gate:?} gates a read");
    }
    for wire in writes() {
        assert!(to_engine_action(wire, &OneModel, &policy(GithubWrites::Allowed)).is_ok());
    }
}

/// Read-only wins over the flag; otherwise the flag decides.
#[test]
fn a_read_only_launch_refuses_writes_whatever_the_flag_says() {
    use luxe_core::permission::PermissionMode;
    assert_eq!(GithubWrites::for_launch(true, PermissionMode::ReadOnly), GithubWrites::ReadOnly);
    assert_eq!(GithubWrites::for_launch(false, PermissionMode::ReadOnly), GithubWrites::ReadOnly);
    for mode in [PermissionMode::Default, PermissionMode::Strict, PermissionMode::Yolo] {
        assert_eq!(GithubWrites::for_launch(true, mode), GithubWrites::Allowed, "{mode:?}");
        assert_eq!(GithubWrites::for_launch(false, mode), GithubWrites::NotEnabled, "{mode:?}");
    }
    assert!(GithubWrites::Allowed.allowed());
    assert!(!GithubWrites::NotEnabled.allowed());
    assert!(!GithubWrites::ReadOnly.allowed());
}

/// The two prompt replies keep their correlation id and their answer.
///
/// Losing either one deadlocks the turn they were sent to unblock, and a swapped id would
/// resolve somebody else's prompt.
#[test]
fn a_prompt_reply_keeps_its_id_and_its_answer() {
    let id: luxe_core::permission::PermissionRequestId =
        serde_json::from_value(serde_json::json!("perm:7")).unwrap();
    let translated = translate(WireAction::RespondPermission {
        id: id.clone(),
        reply: luxe_core::permission::PermissionReply::Reject,
    });
    assert!(matches!(
        translated,
        Ok(Action::RespondPermission { id: actual, reply })
            if actual == id && reply == luxe_core::permission::PermissionReply::Reject
    ));

    let id: luxe_core::question::QuestionRequestId =
        serde_json::from_value(serde_json::json!("ask:7")).unwrap();
    let translated = translate(WireAction::AnswerQuestion {
        id: id.clone(),
        reply: luxe_core::question::QuestionReply::Answered(vec!["postgres".into()]),
    });
    assert!(matches!(
        translated,
        Ok(Action::AnswerQuestion { id: actual, reply: luxe_core::question::QuestionReply::Answered(answers) })
            if actual == id && answers == vec!["postgres".to_string()]
    ));
}

/// A known model is filled in from the catalog rather than taken on the client's word.
#[test]
fn select_model_resolves_the_metadata_the_client_did_not_send() {
    let translated = translate(WireAction::SelectModel(luxe_core::ModelRef::new(
        "anthropic",
        "claude-sonnet-4-20250514",
    )));
    let Ok(Action::SetModel(info)) = translated else { panic!("expected SetModel") };
    assert_eq!(info.r#ref.id, "claude-sonnet-4-20250514");
    assert_eq!(info.context_window, 200_000, "the catalog's window, not a default");
}

/// A model no catalog layer knows is still selectable, on the resolver's defaults.
///
/// The break this catches: adding a "reject what the catalog has not heard of" guard, which
/// reads like prudence and would make every local, proxied, or brand-new model unselectable
/// over RPC while `--model` kept accepting it.
#[test]
fn select_model_accepts_a_model_no_catalog_knows() {
    let translated =
        translate(WireAction::SelectModel(luxe_core::ModelRef::new("lmstudio", "qwen3-coder-30b")));
    let Ok(Action::SetModel(info)) = translated else { panic!("a local model is still a model") };
    assert_eq!(info.r#ref.id, "qwen3-coder-30b");
    assert_eq!(info.r#ref.provider, "lmstudio");
}

/// An unknown frame is refused by name, so a client sees which of its actions this build lacks.
#[test]
fn an_unknown_frame_is_refused_by_name() {
    let rejected =
        translate(WireAction::Other { kind: "switch_role".into(), content: serde_json::json!({}) })
            .expect_err("this build has no switch_role");
    assert_eq!(rejected.kind.as_deref(), Some("switch_role"));
    assert!(rejected.reason.contains("switch_role"), "{}", rejected.reason);
}

/// The real catalog resolver keeps the identity it was asked about, and gives a usable window.
///
/// Every other test here injects a double, so nothing exercised the production path —
/// `cargo mutants` replaced this whole resolver with `Default::default()` and the suite stayed
/// green. That default is a `ModelInfo` with an empty ref and a **zero** context window, which
/// over RPC means switching to a nameless model and reporting context fullness against a
/// divisor of nothing. Asserted against a model no config can plausibly contain, so the answer
/// comes from the built-in defaults rather than from whatever the developer has configured.
#[test]
fn the_real_resolver_answers_about_the_model_it_was_asked() {
    let dir = tempfile::tempdir().unwrap();
    let overrides = crate::engine_setup::CliOverrides::default();
    let settings = crate::engine_setup::AgentSettings::resolve(dir.path().into(), overrides);
    let info = CatalogResolver { settings }.resolve("no-such-provider", "no-such-model-9e3f");
    assert_eq!(info.r#ref.provider, "no-such-provider");
    assert_eq!(info.r#ref.id, "no-such-model-9e3f");
    assert!(info.context_window > 0, "a zero window makes every context percentage a lie");
}

/// Every published kind has a translator arm.
///
/// `WireAction` is `#[non_exhaustive]`, so the compiler cannot hold this match complete from
/// another crate — the count is what does. Publishing one more kind fails here, which is the
/// prompt to write its arm and its case above rather than let it reach clients as
/// `action_rejected`.
#[test]
fn the_translator_covers_the_whole_published_vocabulary() {
    assert_eq!(
        WireAction::KINDS.len(),
        54,
        "a wire action was published without a translator arm: {:?}",
        WireAction::KINDS
    );
}
