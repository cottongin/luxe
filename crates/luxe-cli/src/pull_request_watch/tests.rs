use super::*;
use luxe_core::github::{Account, BranchHead, CheckRollup, PullRequestStatus, RateLimit};
use luxe_core::message::{Message, Role};
use std::path::Path;
use std::sync::Mutex;

/// A GitHub double answering `branch_status` from a queue, and counting what it was asked.
#[derive(Default)]
struct Queue {
    answers: Mutex<Vec<Result<BranchStatus, GitHubError>>>,
    repository_asks: Mutex<usize>,
    status_asks: Mutex<usize>,
}

#[async_trait::async_trait]
impl luxe_core::github::Discovery for Queue {
    async fn account(&self, _: Option<&str>) -> Result<Account, GitHubError> {
        unreachable!()
    }

    async fn repository(&self, _: &Path) -> Result<Repository, GitHubError> {
        *self.repository_asks.lock().unwrap() += 1;
        Ok(Repository {
            host: "github.com".into(),
            owner: "me".into(),
            name: "app".into(),
            default_branch: "main".into(),
            remote: Some("origin".into()),
        })
    }

    async fn search(
        &self,
        _host: &str,
        _query: &str,
        _after: Option<&str>,
    ) -> Result<luxe_core::github::SearchPage, GitHubError> {
        Ok(luxe_core::github::SearchPage::default())
    }
}

#[async_trait::async_trait]
impl luxe_core::github::Branches for Queue {
    async fn branch_status(
        &self,
        _: &Repository,
        head: &BranchHead,
    ) -> Result<BranchStatus, GitHubError> {
        assert_eq!(head.name, "topic");
        *self.status_asks.lock().unwrap() += 1;
        self.answers.lock().unwrap().remove(0)
    }

    async fn pull_request_head(
        &self,
        _repository: &Repository,
        _number: u64,
    ) -> Result<luxe_core::github::PullRequestHead, GitHubError> {
        unreachable!("this double reads no pull request heads")
    }

    async fn merged_below(
        &self,
        _repository: &Repository,
        _number: u64,
        _base: &str,
    ) -> Result<Option<luxe_core::restack::MergedBelow>, GitHubError> {
        Ok(None)
    }
}

fn open(pending: usize, remaining: u32) -> Result<BranchStatus, GitHubError> {
    Ok(BranchStatus {
        pull_request: Some(PullRequestStatus {
            number: 7,
            title: "Topic".into(),
            url: "https://github.com/me/app/pull/7".into(),
            state: PullRequestState::Open,
            draft: false,
            review_decision: None,
            checks: CheckRollup { passed: 1, pending, ..CheckRollup::default() },
            unresolved_threads: 0,
            head_oid: "abc".into(),
            stack: None,
        }),
        rate_limit: Some(RateLimit { remaining, reset_at: 4_102_444_800 }),
    })
}

/// A real checkout on branch `topic`, so `branch_head` has something to read.
fn checkout() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let run = |args: &[&str]| {
        let output =
            std::process::Command::new("git").args(args).current_dir(dir.path()).output().unwrap();
        assert!(output.status.success(), "{args:?}: {output:?}");
    };
    run(&["init", "-q", "-b", "topic"]);
    dir
}

async fn refresh(watch: &mut PullRequestWatch<Queue>, now: Instant) -> Option<PullRequestPresence> {
    let (answers, mut answered) = mpsc::channel(1);
    watch.start(now, &answers);
    assert_eq!(watch.due(now), None, "one at a time");
    let looked = answered.recv().await.unwrap();
    watch.finish(looked, now)
}

fn tool_call(id: &str, command: &str) -> Event {
    Event::MessageCommitted {
        session_id: "s".into(),
        entry_id: luxe_core::session::EntryId("e".into()),
        message: Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolCall {
                id: id.into(),
                name: "bash".into(),
                arguments: serde_json::json!({ "command": command }),
            }],
        },
    }
}

#[tokio::test]
async fn the_first_look_is_due_at_once_and_the_repository_is_asked_for_once() {
    let dir = checkout();
    let github = Arc::new(Queue::default());
    github.answers.lock().unwrap().extend([open(0, 4_000), open(0, 4_000)]);
    let mut watch = PullRequestWatch::new(github.clone(), dir.path().to_path_buf(), 60);
    let t0 = Instant::now();
    assert_eq!(watch.due(t0), Some(t0));

    let presence = refresh(&mut watch, t0).await;
    assert!(matches!(presence, Some(PullRequestPresence::Found(pr)) if pr.number == 7));
    assert_eq!(watch.due(t0), Some(t0 + Duration::from_secs(60)));
    refresh(&mut watch, t0 + Duration::from_secs(60)).await;
    assert_eq!(*github.repository_asks.lock().unwrap(), 1);
    assert_eq!(*github.status_asks.lock().unwrap(), 2);
}

/// A push the agent ran is noticed when the call finishes, not when it was written, and it
/// makes the watch hurry for the checks it started.
#[tokio::test]
async fn a_finished_push_asks_at_once_and_then_hurries() {
    let dir = checkout();
    let github = Arc::new(Queue::default());
    github.answers.lock().unwrap().extend([open(0, 4_000), open(2, 4_000)]);
    let mut watch = PullRequestWatch::new(github, dir.path().to_path_buf(), 60);
    let t0 = Instant::now();
    refresh(&mut watch, t0).await;

    let t1 = t0 + Duration::from_secs(8);
    watch.observe(&tool_call("call-1", "cargo fmt && git push"), t1);
    assert_eq!(watch.due(t1), Some(t0 + Duration::from_secs(60)), "not while it runs");
    watch.observe(&Event::ToolEnded { call_id: "call-1".into(), is_error: false }, t1);
    assert_eq!(watch.due(t1), Some(t1 + SETTLE));

    refresh(&mut watch, t1).await;
    assert_eq!(watch.due(t1), Some(t1 + Duration::from_secs(15)), "checks are running");
}

#[tokio::test]
async fn events_in_a_burst_or_right_after_a_refresh_wait_their_turn() {
    let dir = checkout();
    let github = Arc::new(Queue::default());
    github.answers.lock().unwrap().push(open(0, 4_000));
    let mut watch = PullRequestWatch::new(github, dir.path().to_path_buf(), 60);
    let t0 = Instant::now();
    refresh(&mut watch, t0).await;

    watch.observe(&Event::TerminalFocusChanged { focused: true }, t0);
    watch.observe(&Event::WorkspaceRefreshRequested, t0 + Duration::from_millis(100));
    assert_eq!(watch.due(t0), Some(t0 + EVENT_GAP));
    watch.observe(&Event::TerminalFocusChanged { focused: false }, t0);
    assert_eq!(watch.due(t0), Some(t0 + EVENT_GAP), "losing focus asks for nothing");
}

/// A pull request opened from the review shows on the row at the next turn allowed, not at the
/// next scheduled look.
#[tokio::test]
async fn opening_a_pull_request_asks_for_a_look() {
    let dir = checkout();
    let github = Arc::new(Queue::default());
    github.answers.lock().unwrap().push(open(0, 4_000));
    let mut watch = PullRequestWatch::new(github, dir.path().to_path_buf(), 60);
    let t0 = Instant::now();
    refresh(&mut watch, t0).await;
    assert!(watch.due(t0) > Some(t0 + EVENT_GAP));
    let opened = Event::PullRequestOpened {
        request_id: luxe_core::guided_review::PullRequestOpenId::new(),
        number: 151,
        url: String::new(),
        committed: Vec::new(),
    };
    watch.observe(&opened, t0);
    assert_eq!(watch.due(t0), Some(t0 + EVENT_GAP));
}

#[tokio::test]
async fn a_failed_look_keeps_the_row_and_an_unfixable_one_says_why() {
    let dir = checkout();
    let github = Arc::new(Queue::default());
    github.answers.lock().unwrap().extend([
        open(0, 4_000),
        Err(GitHubError::Timeout { after: Duration::from_secs(60) }),
        Err(GitHubError::NotAuthenticated { host: None }),
    ]);
    let mut watch = PullRequestWatch::new(github.clone(), dir.path().to_path_buf(), 60);
    let t0 = Instant::now();
    refresh(&mut watch, t0).await;
    assert_eq!(refresh(&mut watch, t0).await, None, "a timeout is no news");
    let Some(PullRequestPresence::Unavailable { reason }) = refresh(&mut watch, t0).await else {
        panic!("signed out is said");
    };
    assert!(reason.contains("gh auth login"), "{reason}");
    assert_eq!(watch.due(t0), None, "and not retried on a timer");
    watch.observe(&Event::TerminalFocusChanged { focused: true }, t0);
    assert!(watch.due(t0).is_some(), "but coming back to the terminal asks again");
    assert_eq!(*github.repository_asks.lock().unwrap(), 1, "the cached repository was used");
}

#[tokio::test]
async fn a_spent_budget_waits_for_the_refill_even_for_events() {
    let dir = checkout();
    let github = Arc::new(Queue::default());
    github.answers.lock().unwrap().extend([open(0, 0)]);
    let mut watch = PullRequestWatch::new(github, dir.path().to_path_buf(), 60);
    let t0 = Instant::now();
    refresh(&mut watch, t0).await;
    watch.observe(&Event::WorkspaceRefreshRequested, t0);
    let due = watch.due(t0).unwrap();
    assert!(due > t0 + Duration::from_secs(3_600), "until 2100, as GitHub said: {due:?}");

    let mut limited = PullRequestWatch::new(Arc::new(Queue::default()), PathBuf::new(), 60);
    let looked = Looked { repository: None, outcome: Err(GitHubError::RateLimited) };
    assert_eq!(limited.finish(looked, t0), None);
    assert_eq!(limited.due(t0), Some(t0 + IDLE), "no refill time known: a few minutes");
}

#[test]
fn a_new_branch_asks_again_and_the_first_one_seen_does_not() {
    let mut watch = PullRequestWatch::new(Arc::new(Queue::default()), PathBuf::new(), 0);
    let t0 = Instant::now();
    assert!(!watch.saw_branch(Some("topic"), t0));
    assert_eq!(watch.due(t0), None, "background off, nothing asked");
    assert!(!watch.saw_branch(Some("topic"), t0));
    assert!(watch.saw_branch(Some("other"), t0));
    assert_eq!(watch.due(t0), Some(t0 + SETTLE));
}

/// End to end through the bridge: the first snapshot says nothing is known, and the one after
/// the first look carries the pull request.
#[tokio::test]
async fn the_bridge_carries_what_the_watch_found() {
    let dir = checkout();
    let github = Arc::new(Queue::default());
    github.answers.lock().unwrap().push(open(0, 4_000));
    let watch = PullRequestWatch::new(github, dir.path().to_path_buf(), 60);
    let (_engine_tx, engine_rx) = mpsc::channel(8);
    let mut events = crate::workspace_runtime::bridge(
        engine_rx,
        crate::workspace_runtime::Roots::same(dir.path().to_path_buf()),
        Some(watch),
    );

    let Some(Event::WorkspaceUpdated(first)) = events.recv().await else {
        panic!("an initial snapshot");
    };
    assert_eq!(first.pull_request, PullRequestPresence::Unknown);
    let found = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let Some(Event::WorkspaceUpdated(snapshot)) = events.recv().await {
                if let PullRequestPresence::Found(pr) = snapshot.pull_request {
                    break pr;
                }
            }
        }
    })
    .await
    .expect("the watch's answer reaches a snapshot");
    assert_eq!(found.number, 7);
}
