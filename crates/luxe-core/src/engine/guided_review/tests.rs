use super::target::pull_request_failure;
use super::*;
use crate::event::GuidedReviewFailureKind;
use crate::github::GitHubError;

fn failure(error: GitHubError) -> GuidedReviewFailure {
    pull_request_failure(&PullRequestResolveError::GitHub {
        number: 12,
        repository: Some("acme/app".into()),
        error,
    })
}

/// Each way `gh` can fail is a sentence naming what to do, and only a missing pull request is
/// the reader's target being wrong rather than the machine being unready.
#[test]
fn every_github_failure_says_what_to_do() {
    let cases = [
        (GitHubError::NotFound, "no pull request #12 in acme/app"),
        (GitHubError::NotInstalled, "https://cli.github.com"),
        (GitHubError::NotAuthenticated { host: None }, "gh auth login"),
        (GitHubError::NotGitHub, "no remote on GitHub"),
        (GitHubError::Timeout { after: std::time::Duration::from_secs(60) }, "within 60s"),
        (
            GitHubError::Run {
                program: "gh",
                source: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
            },
            "Could not run `gh`",
        ),
        (
            GitHubError::Unreadable(serde_json::from_str::<u8>("x").unwrap_err()),
            "Updating the GitHub CLI",
        ),
        (GitHubError::Failed { message: "HTTP 502".into() }, "HTTP 502"),
        (GitHubError::Rejected { reasons: vec!["Not accessible".into()] }, "Not accessible"),
    ];
    for (error, says) in cases {
        let not_found = matches!(error, GitHubError::NotFound);
        let failure = failure(error);
        assert!(failure.message.contains(says), "{}", failure.message);
        let expected = if not_found {
            GuidedReviewFailureKind::InvalidTarget
        } else {
            GuidedReviewFailureKind::Unavailable
        };
        assert_eq!(failure.kind, expected, "{}", failure.message);
    }
}

/// Pauses a record write under `root` between its claim and the write, when a test asked for
/// that root: the window a newer request used to begin in.
pub(super) async fn paused_before_record_save(root: &std::path::Path) {
    paused_at(root, Stage::RecordSave).await;
}

/// Pauses a publication under `root` between its record write and its desk send, when a test asked
/// for that root: the window a claim released early would let a newer request into.
pub(super) async fn paused_before_desk_send(root: &std::path::Path) {
    paused_at(root, Stage::DeskSend).await;
}

#[derive(PartialEq)]
enum Stage {
    RecordSave,
    DeskSend,
}

async fn paused_at(root: &std::path::Path, stage: Stage) {
    let pause = {
        let mut pauses = PAUSES.lock().unwrap();
        let at = pauses.iter().position(|(paused, at, _)| paused == root && *at == stage);
        at.map(|at| pauses.swap_remove(at).2)
    };
    if let Some((entered, release)) = pause {
        entered.notify_one();
        release.notified().await;
    }
}

type Pause = (Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>);
static PAUSES: std::sync::Mutex<Vec<(std::path::PathBuf, Stage, Pause)>> =
    std::sync::Mutex::new(Vec::new());

/// Arms one-shot pauses on the next record write under `root` and the desk send after it.
fn pause_record_save(root: &std::path::Path) -> (Pause, Pause) {
    let arm = |stage| {
        let pause: Pause = (Arc::default(), Arc::default());
        PAUSES.lock().unwrap().push((root.to_path_buf(), stage, pause.clone()));
        pause
    };
    (arm(Stage::RecordSave), arm(Stage::DeskSend))
}

/// Whether the active-review record under `root` exists by the time a newer request begins, and
/// what had reached the desk by then.
async fn written_before_a_successor_begins(
    active: &Arc<tokio::sync::Mutex<ActiveGuidedReview>>,
    root: &std::path::Path,
    ((entered, release), (sending, send)): (Pause, Pause),
    mut inbox: super::super::review_desk::DeskInbox,
) -> tokio::task::JoinHandle<(bool, Vec<super::super::review_desk::DeskCommand>)> {
    let reached = |notify: Arc<tokio::sync::Notify>| async move {
        tokio::time::timeout(std::time::Duration::from_secs(10), notify.notified())
            .await
            .expect("the paused stage was reached");
    };
    reached(entered).await;
    let record = root.join(crate::guided_review::ACTIVE_REVIEW_RECORD_RELATIVE_PATH);
    let active = Arc::clone(active);
    let successor = tokio::spawn(async move {
        let mut active = active.lock().await;
        let written = record.exists();
        let mut delivered = Vec::new();
        while let Ok(command) = inbox.try_recv() {
            delivered.push(command);
        }
        active.begin_request(ReviewLoadRequestId::new());
        (written, delivered)
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    release.notify_one();
    reached(sending).await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    send.notify_one();
    successor
}

fn guide_context(
    cancel: super::super::review_state::GuideCancel,
) -> super::super::guide_run::GuideRunContext {
    super::super::guide_run::GuideRunContext {
        provider: Arc::new(super::super::tests::OversizedGuideProvider),
        model: crate::provider::ModelInfo::default(),
        cancel,
        ledger: Arc::default(),
    }
}

/// A load that begins while an older one is writing its record waits for that record to be
/// written and published, and never finds the older one landing after it.
///
/// The break this catches: the claim was checked, then released, then the record written and
/// the review published — so a request superseded during the write still overwrote the newer
/// one's record and reached the desk after it.
#[tokio::test]
async fn a_superseded_load_cannot_land_after_its_successor_began() {
    let root = tempfile::tempdir().unwrap();
    let pause = pause_record_save(root.path());
    let active = Arc::new(tokio::sync::Mutex::new(ActiveGuidedReview::default()));
    let first = ReviewLoadRequestId::new();
    let (_, cancel) = active.lock().await.begin_request(first.clone());
    let (desk, inbox) = super::super::review_desk::ReviewDeskHandle::channel();
    let (event_tx, _events) = mpsc::channel(64);
    let publish = ReviewPublish {
        provenance: None,
        insight_provider: None,
        persistence_root: root.path().to_path_buf(),
        active_review: Arc::clone(&active),
        guide: guide_context(cancel),
        event_tx,
        desk,
    };
    let review = super::super::tests::single_file_review();
    let target = review.target.clone();
    let loading = tokio::spawn(publish_review(Ok(review), first.clone(), target, publish));

    let successor = written_before_a_successor_begins(&active, root.path(), pause, inbox).await;
    loading.await.unwrap();
    let (written, delivered) = successor.await.unwrap();
    assert!(written, "the older record was written before the newer began");
    assert!(
        delivered.iter().any(|command| matches!(
            command,
            super::super::review_desk::DeskCommand::Publish { request_id, .. } if *request_id == first
        )),
        "and reached the desk before it: {delivered:?}"
    );
}

/// The Guide's upgrade of a review's record lands before a newer request begins, too.
///
/// The break this catches: the upgrade checked it was current, let go, and then wrote the
/// chaptered record, so a load begun in between had its record overwritten by the older review.
#[tokio::test]
async fn a_superseded_guide_cannot_land_after_its_successor_began() {
    let root = tempfile::tempdir().unwrap();
    let review = super::super::tests::single_file_review();
    let brief = crate::guided_review::ReviewBrief { intent: "Why".into(), account: vec![] };
    let provenance = super::super::tests::luxe_core_guide_provenance_model();
    let chapters = crate::guided_review::fallback_chapters(&review.files).unwrap();
    crate::guided_review::GuideCache::new(root.path())
        .save(&review.files, &chapters, Some(&brief), provenance)
        .unwrap();
    let pause = pause_record_save(root.path());
    let active = Arc::new(tokio::sync::Mutex::new(ActiveGuidedReview::default()));
    let first = ReviewLoadRequestId::new();
    let (_, cancel) = active.lock().await.begin_request(first.clone());
    let (desk, inbox) = super::super::review_desk::ReviewDeskHandle::channel();
    let (event_tx, _events) = mpsc::channel(64);
    let root_path = root.path().to_path_buf();
    Engine::spawn_guide_generation(
        guide_context(cancel),
        Arc::clone(&active),
        review,
        first.clone(),
        root_path,
        event_tx,
        desk,
    );

    let successor = written_before_a_successor_begins(&active, root.path(), pause, inbox).await;
    let (written, delivered) = successor.await.unwrap();
    assert!(written, "the chaptered record was written before the newer began");
    assert!(
        delivered.iter().any(|command| matches!(
            command,
            super::super::review_desk::DeskCommand::GuideUpdated { request_id, .. }
                if *request_id == first
        )),
        "and reached the desk before it: {delivered:?}"
    );
}
