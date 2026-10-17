//! Harness — shared pieces of the pull request scenarios.

use std::path::Path;
use std::sync::Arc;

use futures::stream::{self, BoxStream};
use luxe_core::engine::{Action, Engine, EngineConfig};
use luxe_core::event::{Event, GuidedReviewFailure};
use luxe_core::github::GitHub;
use luxe_core::guided_review::{GuidedReview, ReviewLoadRequestId, ReviewTarget};
use luxe_core::provider::{
    CompletionRequest, ModelInfo, Provider, ProviderError, ProviderEvent, StopReason,
};
use luxe_core::session::{JsonlSessionStore, SessionStore};
use luxe_core::tool::ToolRegistry;
use luxe_core::HookRegistry;
use tokio::sync::Mutex;

use super::*;

/// How long a scenario waits for the engine before calling it a hang
pub(crate) const WAIT: std::time::Duration = std::time::Duration::from_secs(20);

pub(crate) struct Recorder {
    pub(crate) requests: Arc<Mutex<Vec<CompletionRequest>>>,
}

#[async_trait::async_trait]
impl Provider for Recorder {
    fn name(&self) -> &str {
        "recorder"
    }

    async fn stream(
        &self,
        request: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        self.requests.lock().await.push(request);
        Ok(Box::pin(stream::iter(vec![
            ProviderEvent::TextDelta { index: 0, delta: "Understood.".into() },
            ProviderEvent::Done { reason: StopReason::EndTurn },
        ])))
    }
}

pub(crate) struct Harness {
    pub(crate) actions: tokio::sync::mpsc::Sender<Action>,
    pub(crate) events: tokio::sync::mpsc::Receiver<Event>,
    pub(crate) requests: Arc<Mutex<Vec<CompletionRequest>>>,
    pub(crate) sessions: tempfile::TempDir,
    pub(crate) session: luxe_core::session::SessionId,
}

pub(crate) async fn harness(project: &Path, github: Option<Arc<dyn GitHub>>) -> Harness {
    harness_on_issue(project, github, None).await
}

/// A harness whose session took up `issue`, the way `luxe --issue` records it.
pub(crate) async fn harness_on_issue(
    project: &Path,
    github: Option<Arc<dyn GitHub>>,
    issue: Option<u64>,
) -> Harness {
    harness_with(project, github, issue, |_| {}).await
}

/// A harness whose store `before` writes into first, beside the session the engine runs.
pub(crate) async fn harness_with(
    project: &Path,
    github: Option<Arc<dyn GitHub>>,
    issue: Option<u64>,
    before: impl FnOnce(&mut JsonlSessionStore),
) -> Harness {
    let sessions = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(sessions.path());
    before(&mut store);
    let session_id = store.create(project.to_str().unwrap()).unwrap();
    if let Some(number) = issue {
        let leaf = luxe_core::session::latest_leaf(&store.load(&session_id).unwrap());
        let url = format!("https://github.com/acme/app/issues/{number}");
        store
            .append(&session_id, leaf, luxe_core::session::EntryKind::IssueLink { number, url })
            .unwrap();
    }
    let requests = Arc::new(Mutex::new(Vec::new()));
    let config = EngineConfig {
        session_store: std::sync::RwLock::new(Box::new(store)),
        provider: Arc::new(Recorder { requests: requests.clone() }),
        providers: std::collections::HashMap::new(),
        model: ModelInfo::default(),
        tool_registry: ToolRegistry::new(),
        hook_registry: HookRegistry::new(),
        session_id: session_id.clone(),
        system_prompt: None,
        todo_store: None,
        compaction_settings: luxe_core::compaction::CompactionSettings::default(),
        permission: Default::default(),
        modes: Default::default(),
        reviewer: None,
        formatter_registry: None,
        spend_sink: None,
        guided_review_project_root: Some(project.to_path_buf()),
        invoked_from: None,
        external_work: None,
        review_context_files: Vec::new(),
        review_exclude: Vec::new(),
        remember_runtime_state: false,
        cache_warming: luxe_core::cache_warming::CacheWarming::Off,
        review_insight_provider: None,
        github,
        permissions_file: None,
        max_tool_iterations: luxe_core::engine::DEFAULT_MAX_TOOL_ITERATIONS,
        startup_notices: Vec::new(),
        action_tx: None,
    };
    let (actions, mut events) = Engine::spawn(config);
    assert!(matches!(events.recv().await, Some(Event::SessionLoaded { .. })));
    Harness { actions, events, requests, sessions, session: session_id }
}

impl Harness {
    /// A turn the session really had, so it has a prompt and a record a review could borrow.
    pub(crate) async fn chat(&mut self, text: &str) {
        self.actions
            .send(Action::SubmitUserMessage {
                content: text.into(),
                images: vec![],
                remediation: None,
            })
            .await
            .unwrap();
        loop {
            match tokio::time::timeout(WAIT, self.events.recv()).await.unwrap() {
                Some(Event::TurnEnded { .. }) => break,
                Some(_) => {}
                None => panic!("engine stopped"),
            }
        }
    }

    pub(crate) async fn load(
        &mut self,
        target: ReviewTarget,
    ) -> Result<GuidedReview, GuidedReviewFailure> {
        let request_id = ReviewLoadRequestId::new();
        self.actions
            .send(Action::LoadGuidedReview { request_id: request_id.clone(), target })
            .await
            .unwrap();
        self.outcome(&request_id).await
    }

    pub(crate) async fn refresh(
        &mut self,
        review: GuidedReview,
    ) -> Result<GuidedReview, GuidedReviewFailure> {
        let request_id = ReviewLoadRequestId::new();
        let expected_revision = review.revision_id.clone();
        self.actions
            .send(Action::RefreshGuidedReview {
                request_id: request_id.clone(),
                review_id: review.id.clone(),
                expected_revision,
            })
            .await
            .unwrap();
        self.outcome(&request_id).await
    }

    pub(crate) async fn outcome(
        &mut self,
        wanted: &ReviewLoadRequestId,
    ) -> Result<GuidedReview, GuidedReviewFailure> {
        loop {
            match tokio::time::timeout(WAIT, self.events.recv()).await.unwrap() {
                Some(Event::GuidedReviewLoaded { request_id, review }) if &request_id == wanted => {
                    return Ok(*review)
                }
                Some(Event::GuidedReviewFailed { request_id, error, .. })
                    if &request_id == wanted =>
                {
                    return Err(error)
                }
                Some(_) => {}
                None => panic!("engine stopped"),
            }
        }
    }

    /// The guide's prompt: the first request made after the review loaded.
    pub(crate) async fn guide_prompt(&self, after: usize) -> String {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            if let Some(request) = self.requests.lock().await.get(after) {
                return serde_json::to_string(&request.messages).unwrap();
            }
            assert!(tokio::time::Instant::now() < deadline, "the guide never asked the model");
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }
}

pub(crate) fn scripted(remote: &Remote, fail_with: Failure) -> Arc<ScriptedGitHub> {
    Arc::new(ScriptedGitHub {
        base: remote.base.clone(),
        head: std::sync::Mutex::new(remote.head.clone()),
        fail_with,
        threads_fail: false,
        viewer: "reader".into(),
        posting: std::sync::Mutex::new(Posting::Lands),
        reviews: std::sync::Mutex::new(vec![luxe_core::github::ExistingReview {
            id: 1,
            author: "someone-else".into(),
            commit_id: remote.head.clone(),
            body: "An earlier review.".into(),
            submitted_at: String::new(),
            url: String::new(),
        }]),
        submitted: std::sync::Mutex::new(Vec::new()),
        answered: std::sync::Mutex::new(Vec::new()),
        opened: std::sync::Mutex::new(Vec::new()),
        opening_lost: std::sync::atomic::AtomicBool::new(false),
        branch_pull_request: std::sync::Mutex::new(None),
        push: std::sync::Mutex::new(luxe_core::github::PushAccess::Allowed),
        searched: std::sync::Mutex::new(Vec::new()),
        pull_request_base: std::sync::Mutex::new("main".into()),
        merged: std::sync::Mutex::new(None),
    })
}

impl Harness {
    /// Waits for the guide pass to ask the model, so what follows is only the feedback's own
    /// traffic, and returns how many requests that was.
    pub(crate) async fn settled(&self) -> usize {
        self.guide_prompt(0).await;
        self.requests.lock().await.len()
    }

    pub(crate) async fn submit(
        &mut self,
        review: &GuidedReview,
        mode: luxe_core::guided_review::ReviewFeedbackMode,
        severity: luxe_core::guided_review::FindingSeverity,
    ) -> Event {
        let submission_id = luxe_core::guided_review::SubmissionId::new();
        self.actions
            .send(Action::SubmitReviewFeedback {
                submission_id: submission_id.clone(),
                review_id: review.id.clone(),
                source: review.source.clone(),
                mode,
                findings: vec![finding(severity)],
            })
            .await
            .unwrap();
        loop {
            match tokio::time::timeout(WAIT, self.events.recv()).await.unwrap() {
                Some(event @ Event::ReviewFeedbackTurnFailed { .. })
                | Some(event @ Event::ReviewFeedbackTurnDispatched { .. }) => return event,
                Some(_) => {}
                None => panic!("engine stopped"),
            }
        }
    }
}

impl Harness {
    pub(crate) async fn post_answer(
        &mut self,
        wanted: &luxe_core::guided_review::PullRequestPostId,
    ) -> Event {
        loop {
            match tokio::time::timeout(WAIT, self.events.recv()).await.unwrap() {
                Some(event @ Event::PullRequestPostPrepared { .. })
                | Some(event @ Event::PullRequestReviewPosted { .. })
                | Some(event @ Event::PullRequestPostFailed { .. }) => {
                    let request_id = match &event {
                        Event::PullRequestPostPrepared { request_id, .. }
                        | Event::PullRequestReviewPosted { request_id, .. }
                        | Event::PullRequestPostFailed { request_id, .. } => request_id,
                        _ => unreachable!(),
                    };
                    if request_id == wanted {
                        return event;
                    }
                }
                Some(_) => {}
                None => panic!("engine stopped"),
            }
        }
    }

    pub(crate) async fn prepare(&mut self, review: &GuidedReview) -> Event {
        self.prepare_as(luxe_core::guided_review::PullRequestPostId::new(), review).await
    }

    pub(crate) async fn prepare_as(
        &mut self,
        request_id: luxe_core::guided_review::PullRequestPostId,
        review: &GuidedReview,
    ) -> Event {
        self.actions
            .send(Action::PreparePullRequestPost {
                request_id: request_id.clone(),
                review_id: review.id.clone(),
            })
            .await
            .unwrap();
        self.post_answer(&request_id).await
    }

    /// Writes `body` as a comment on the review as a whole and answers the session that stored it.
    pub(crate) async fn comment(
        &mut self,
        review: &GuidedReview,
        body: &str,
    ) -> luxe_core::guided_review::ReviewSession {
        let request_id = luxe_core::guided_review::ReviewSessionSaveRequestId::new();
        self.actions
            .send(Action::EditReview {
                request_id: request_id.clone(),
                review_id: review.id.clone(),
                edit: luxe_core::engine::ReviewEdit::Comment {
                    draft: luxe_core::engine::CommentDraft {
                        anchor: luxe_core::guided_review::FindingAnchor::Review,
                        severity: luxe_core::guided_review::FindingSeverity::Issue,
                        title: String::new(),
                        body: body.into(),
                        proposed_fix: None,
                    },
                    send: false,
                },
            })
            .await
            .unwrap();
        tokio::time::timeout(WAIT, async {
            loop {
                match self.events.recv().await {
                    Some(Event::ReviewSessionSaved { request_id: saved, session })
                        if saved == request_id =>
                    {
                        return *session;
                    }
                    Some(Event::ReviewSessionSaveFailed {
                        request_id: refused, message, ..
                    }) if refused == request_id => {
                        panic!("the comment was refused: {message}");
                    }
                    Some(_) => {}
                    None => panic!("engine stopped"),
                }
            }
        })
        .await
        .unwrap()
    }

    /// Prepares the post, writes `body` as the review's summary, and posts it as comments.
    pub(crate) async fn post(&mut self, review: &GuidedReview, body: &str) -> Event {
        self.post_with(review, body, luxe_core::github::ReviewEvent::Comment).await
    }

    /// [`Harness::post`] with a verdict.
    pub(crate) async fn post_with(
        &mut self,
        review: &GuidedReview,
        body: &str,
        verdict: luxe_core::github::ReviewEvent,
    ) -> Event {
        let request_id = luxe_core::guided_review::PullRequestPostId::new();
        let prepared = self.prepare_as(request_id.clone(), review).await;
        assert!(matches!(prepared, Event::PullRequestPostPrepared { .. }), "{prepared:?}");
        let session = self.comment(review, body).await;
        self.actions
            .send(Action::PostPullRequestReview {
                request_id: request_id.clone(),
                verdict,
                session_revision: session.revision,
            })
            .await
            .unwrap();
        self.post_answer(&request_id).await
    }
}

impl Harness {
    pub(crate) async fn answers_event(
        &mut self,
        wanted: &luxe_core::guided_review::ThreadAnswersId,
    ) -> Event {
        loop {
            match tokio::time::timeout(WAIT, self.events.recv()).await.unwrap() {
                Some(event @ Event::ThreadAnswersPrepared { .. })
                | Some(event @ Event::ThreadsAnswered { .. })
                | Some(event @ Event::ThreadAnswersFailed { .. }) => {
                    let request_id = match &event {
                        Event::ThreadAnswersPrepared { request_id, .. }
                        | Event::ThreadsAnswered { request_id, .. }
                        | Event::ThreadAnswersFailed { request_id, .. } => request_id,
                        _ => unreachable!(),
                    };
                    if request_id == wanted {
                        return event;
                    }
                }
                Some(_) => {}
                None => panic!("engine stopped"),
            }
        }
    }

    pub(crate) async fn prepare_answers(&mut self, review: &GuidedReview) -> Event {
        let request_id = luxe_core::guided_review::ThreadAnswersId::new();
        self.actions
            .send(Action::PrepareThreadAnswers {
                request_id: request_id.clone(),
                review_id: review.id.clone(),
            })
            .await
            .unwrap();
        self.answers_event(&request_id).await
    }

    pub(crate) async fn answer(
        &mut self,
        review: &GuidedReview,
        answers: Vec<luxe_core::guided_review::ThreadReply>,
    ) -> Event {
        let request_id = luxe_core::guided_review::ThreadAnswersId::new();
        self.actions
            .send(Action::AnswerThreads {
                request_id: request_id.clone(),
                review_id: review.id.clone(),
                answers,
            })
            .await
            .unwrap();
        self.answers_event(&request_id).await
    }

    /// Puts a comment for each of `threads` in the review's session, the way loading a pull
    /// request adopts its threads, answering their ids in order.
    pub(crate) async fn threads(
        &mut self,
        review: &GuidedReview,
        threads: &[&str],
    ) -> Vec<luxe_core::guided_review::FindingId> {
        use luxe_core::guided_review::{
            FindingAnchor, FindingId, FindingSeverity, FindingState, GitHubThread, ReviewFinding,
        };
        let findings: Vec<ReviewFinding> = threads
            .iter()
            .map(|thread| ReviewFinding {
                id: FindingId::new(),
                label: String::new(),
                anchor: FindingAnchor::Review,
                severity: FindingSeverity::Issue,
                title: format!("Thread {thread}"),
                body: "From GitHub.".into(),
                proposed_fix: None,
                state: FindingState::Open,
                thread: Vec::new(),
                posted: None,
                github_thread: Some(GitHubThread {
                    id: (*thread).into(),
                    url: format!("https://github.com/acme/app/pull/1#{thread}"),
                    author: "mara".into(),
                    can_reply: true,
                    can_resolve: true,
                    reply_url: None,
                    resolved: false,
                    conversation: thread.starts_with("IC_"),
                }),
            })
            .collect();
        let ids = findings.iter().map(|finding| finding.id.clone()).collect();
        let request_id = luxe_core::guided_review::ReviewSessionSaveRequestId::new();
        self.actions
            .send(Action::EditReview {
                request_id: request_id.clone(),
                review_id: review.id.clone(),
                edit: luxe_core::engine::ReviewEdit::AdoptCarried { findings, send: false },
            })
            .await
            .unwrap();
        tokio::time::timeout(WAIT, async {
            loop {
                match self.events.recv().await {
                    Some(Event::ReviewSessionSaved { request_id: saved, .. })
                        if saved == request_id =>
                    {
                        return;
                    }
                    Some(_) => {}
                    None => panic!("engine stopped"),
                }
            }
        })
        .await
        .unwrap();
        ids
    }
}

impl Harness {
    pub(crate) async fn next_opening(&mut self) -> Event {
        tokio::time::timeout(WAIT, async {
            loop {
                match self.events.recv().await {
                    Some(
                        event @ (Event::PullRequestPrepared { .. }
                        | Event::PullRequestOpened { .. }
                        | Event::PullRequestOpenFailed { .. }),
                    ) => break event,
                    Some(_) => {}
                    None => panic!("engine stopped"),
                }
            }
        })
        .await
        .unwrap()
    }
}

impl Harness {
    pub(crate) async fn next_plan(
        &mut self,
    ) -> (Option<luxe_core::restack::RestackPlan>, Option<String>) {
        tokio::time::timeout(WAIT, async {
            loop {
                if let Some(Event::RestackPlanned { plan, refusal, .. }) = self.events.recv().await
                {
                    break (plan, refusal);
                }
            }
        })
        .await
        .unwrap()
    }

    pub(crate) async fn next_stack(&mut self) -> Event {
        tokio::time::timeout(WAIT, async {
            loop {
                match self.events.recv().await {
                    Some(event @ (Event::StackPrepared { .. } | Event::StackOpened { .. })) => {
                        break event
                    }
                    Some(_) => {}
                    None => panic!("engine stopped"),
                }
            }
        })
        .await
        .unwrap()
    }
}
