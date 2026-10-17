//! A cancel reaches the summarizer: no summary lands after it, and what the call billed stays billed.

use futures::StreamExt;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use luxe_core::usage::UsageSource;

use super::*;
use crate::fixtures::events_until;

/// Sets its flag when dropped, so a test can see the summarizer's stream was let go.
struct DropFlag(Arc<AtomicBool>);

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// How the summarizer answers its nth call.
#[derive(Clone, Copy, Debug)]
enum Summary {
    /// Bills, then says nothing ever again: still thinking when the reader gives up on it
    Stalls,
    /// Never even returns its stream
    NeverOpens,
    /// Bills, then finishes a real summary once the test releases it
    AfterRelease,
}

/// Answers turns like [`CompactingProvider`], and each summarization as `script` says, the last
/// entry repeating. `overflow_first` refuses the first turn call as too long for the window.
struct ScriptedSummaryProvider {
    calls: Arc<Mutex<Vec<CompletionRequest>>>,
    summarizing: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
    dropped: Arc<AtomicBool>,
    script: Vec<Summary>,
    summaries: AtomicUsize,
    overflow_first: AtomicBool,
}

#[async_trait::async_trait]
impl Provider for ScriptedSummaryProvider {
    fn name(&self) -> &str {
        "scripted-summary-mock"
    }

    async fn stream(
        &self,
        request: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        let summarizing = CompactingProvider::is_summarization(&request);
        if !summarizing && self.overflow_first.swap(false, Ordering::SeqCst) {
            return Err(ProviderError::Other(
                "400: prompt is too long: maximum context length exceeded".to_string(),
            ));
        }
        let occupancy = occupancy_of(&request);
        self.calls.lock().await.push(request);
        if !summarizing {
            return Ok(Box::pin(stream::iter(vec![
                ProviderEvent::TextDelta { index: 0, delta: "ok".to_string() },
                ProviderEvent::Usage(Usage {
                    prompt_tokens: occupancy,
                    completion_tokens: 1,
                    total_tokens: occupancy,
                    ..Usage::default()
                }),
                ProviderEvent::Done { reason: StopReason::EndTurn },
            ])));
        }
        let nth = self.summaries.fetch_add(1, Ordering::SeqCst);
        let step = self.script[nth.min(self.script.len() - 1)];
        self.summarizing.notify_one();
        let guard = DropFlag(self.dropped.clone());
        let billed = ProviderEvent::Usage(Usage {
            prompt_tokens: 1_234,
            completion_tokens: 0,
            total_tokens: 1_234,
            ..Usage::default()
        });
        // The mock ignores its cancel receiver, as a provider mid-request may: the engine has to
        // stop waiting on its own
        let rest: BoxStream<'static, ProviderEvent> = match step {
            Summary::NeverOpens => {
                let _held = guard;
                std::future::pending::<()>().await;
                unreachable!()
            }
            Summary::Stalls => Box::pin(stream::pending()),
            Summary::AfterRelease => {
                let release = self.release.clone();
                let summary = format!("## Goal\n{SUMMARY_MARKER}: carry on with what is left.");
                Box::pin(
                    stream::once(async move {
                        release.notified().await;
                        ProviderEvent::TextDelta { index: 0, delta: summary }
                    })
                    .chain(stream::iter([ProviderEvent::Done { reason: StopReason::EndTurn }])),
                )
            }
        };
        let events = stream::iter([billed]).chain(rest).map(move |event| {
            let _held = &guard;
            event
        });
        Ok(Box::pin(events))
    }
}

/// What a test holds of its provider while the engine owns it.
struct Script {
    calls: Arc<Mutex<Vec<CompletionRequest>>>,
    summarizing: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
    dropped: Arc<AtomicBool>,
}

fn scripted(script: &[Summary], overflow_first: bool) -> (Arc<ScriptedSummaryProvider>, Script) {
    let provider = Arc::new(ScriptedSummaryProvider {
        calls: Arc::new(Mutex::new(Vec::new())),
        summarizing: Arc::new(tokio::sync::Notify::new()),
        release: Arc::new(tokio::sync::Notify::new()),
        dropped: Arc::new(AtomicBool::new(false)),
        script: script.to_vec(),
        summaries: AtomicUsize::new(0),
        overflow_first: AtomicBool::new(overflow_first),
    });
    let held = Script {
        calls: provider.calls.clone(),
        summarizing: provider.summarizing.clone(),
        release: provider.release.clone(),
        dropped: provider.dropped.clone(),
    };
    (provider, held)
}

fn stalled() -> (Arc<ScriptedSummaryProvider>, Script) {
    scripted(&[Summary::Stalls], false)
}

async fn summarizer_asked(script: &Script) {
    tokio::time::timeout(std::time::Duration::from_secs(10), script.summarizing.notified())
        .await
        .expect("the summarizer was asked");
}

fn compaction_billed(events: &[Event]) -> bool {
    events.iter().any(|event| {
        matches!(
            event,
            Event::TurnUsage { source: UsageSource::Compaction, prompt_tokens: 1_234, .. }
        )
    })
}

fn no_compaction_on_disk(root: &std::path::Path, session_id: &SessionId) -> bool {
    let entries = JsonlSessionStore::new(root.join("sessions")).load(session_id).unwrap();
    !entries.iter().any(|entry| matches!(entry.kind, EntryKind::Compaction { .. }))
}

/// A session `turns` turns long on `provider`, with nothing compacted yet.
async fn session_of(
    turns: usize,
    provider: Arc<ScriptedSummaryProvider>,
    tmp: &tempfile::TempDir,
) -> (Sender, Receiver, SessionId) {
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/test").unwrap();
    let (action_tx, mut event_rx) =
        Engine::spawn(config(store, session_id.clone(), provider, tmp.path()));
    for turn in 0..turns {
        run_turn(&action_tx, &mut event_rx, bulky(&format!("TURN-{turn}"))).await;
    }
    (action_tx, event_rx, session_id)
}

type Sender = tokio::sync::mpsc::Sender<Action>;
type Receiver = tokio::sync::mpsc::Receiver<Event>;

/// Sends `/compact`, waits for the summarizer to be asked, and cancels it; returns what the engine
/// said up to the refresh sent after the cancel, which proves it is answering again.
async fn compact_then_cancel(
    action_tx: &Sender,
    event_rx: &mut Receiver,
    script: &Script,
) -> Vec<Event> {
    action_tx.send(Action::CompactNow { focus: None }).await.unwrap();
    summarizer_asked(script).await;
    action_tx.send(Action::Cancel).await.unwrap();
    action_tx.send(Action::RefreshWorkspace).await.unwrap();
    events_until(event_rx, "the engine to answer after the cancel", |event| {
        matches!(event, Event::WorkspaceRefreshRequested)
    })
    .await
}

/// Cancel during a threshold compaction ends the turn there: the summarizer's stream is dropped,
/// no summary is committed, and no further provider call goes out.
///
/// The break this catches: compaction read its summary over a private cancel channel that was
/// never raised, so a cancel landing mid-summary left the turn waiting on the summarizer — and a
/// reply that arrived later still replaced the conversation the reader had just stopped.
#[tokio::test]
async fn a_cancel_during_threshold_compaction_ends_the_turn_and_commits_no_summary() {
    let tmp = tempfile::tempdir().unwrap();
    let (provider, script) = stalled();
    let (action_tx, mut event_rx, session_id) = session_of(0, provider, &tmp).await;

    let mut turn = 0;
    loop {
        assert!(turn < TURNS, "the threshold never fired");
        let prompt = bulky(&format!("TURN-{turn}"));
        let submit =
            Action::SubmitUserMessage { content: prompt, images: vec![], remediation: None };
        action_tx.send(submit).await.unwrap();
        turn += 1;
        tokio::select! {
            () = script.summarizing.notified() => break,
            _ = events_until(&mut event_rx, "a turn", |e| matches!(e, Event::TurnEnded { .. })) => {}
        }
    }

    action_tx.send(Action::Cancel).await.unwrap();
    let seen = events_until(&mut event_rx, "the cancelled turn to end", |event| {
        matches!(event, Event::TurnEnded { .. })
    })
    .await;
    let Some(Event::TurnEnded { reason }) = seen.last() else { unreachable!() };
    assert_eq!(*reason, luxe_core::event::StopReason::Other("cancelled".into()));
    assert!(!seen.iter().any(|e| matches!(e, Event::CompactionDone { .. })), "{seen:?}");
    assert!(script.dropped.load(Ordering::SeqCst), "the summarizer's stream is let go");
    let calls = script.calls.lock().await;
    let last = calls.last().unwrap();
    assert!(CompactingProvider::is_summarization(last), "nothing was sent after the cancel");
    drop(calls);
    action_tx.send(Action::Close).await.unwrap();
    assert!(no_compaction_on_disk(tmp.path(), &session_id), "no summary was committed");
}

/// `/compact` is serviced like a turn, so a cancel reaches it, the engine answers what comes next,
/// and what the summarizer billed stays billed.
///
/// The break this catches: the action loop awaited the summarizer inline, so a cancel sat in the
/// inbox behind it and nothing after it was served until the summarizer gave up on its own.
#[tokio::test]
async fn a_cancel_during_compaction_on_request_stops_it_and_the_engine_answers_again() {
    let tmp = tempfile::tempdir().unwrap();
    let (provider, script) = stalled();
    let (action_tx, mut event_rx, session_id) = session_of(6, provider, &tmp).await;

    let seen = compact_then_cancel(&action_tx, &mut event_rx, &script).await;

    assert!(!seen.iter().any(|e| matches!(e, Event::CompactionDone { .. })), "{seen:?}");
    assert!(
        !seen
            .iter()
            .any(|e| matches!(e, Event::Error(message) if message.contains("Nothing to compact"))),
        "a cancelled compaction is not reported as having nothing to do: {seen:?}"
    );
    assert!(script.dropped.load(Ordering::SeqCst), "the summarizer's stream is let go");
    assert!(compaction_billed(&seen), "what the summarizer billed stays billed: {seen:?}");
    action_tx.send(Action::Close).await.unwrap();
    assert!(no_compaction_on_disk(tmp.path(), &session_id), "no summary was committed");
}

/// A cancel reaches a summarizer that has not even returned its stream yet.
#[tokio::test]
async fn a_cancel_before_the_summarizer_answers_at_all_still_lands() {
    let tmp = tempfile::tempdir().unwrap();
    let (provider, script) = scripted(&[Summary::NeverOpens], false);
    let (action_tx, mut event_rx, session_id) = session_of(6, provider, &tmp).await;

    compact_then_cancel(&action_tx, &mut event_rx, &script).await;

    assert!(script.dropped.load(Ordering::SeqCst), "the pending request is let go");
    action_tx.send(Action::Close).await.unwrap();
    assert!(no_compaction_on_disk(tmp.path(), &session_id), "no summary was committed");
}

/// Cancelled compactions are not failures: as many as the breaker allows, and the next time the
/// window fills it still compacts.
///
/// The break this catches: a cancel unwound through the failure path, so a reader who stopped
/// three summaries switched automatic compaction off for the rest of the session.
#[tokio::test]
async fn cancelled_compactions_do_not_trip_the_failure_breaker() {
    let tmp = tempfile::tempdir().unwrap();
    let breaker = luxe_core::compaction::MAX_COMPACTION_FAILURES as usize;
    let mut script = vec![Summary::Stalls; breaker];
    script.push(Summary::AfterRelease);
    let (provider, script) = scripted(&script, false);
    let (action_tx, mut event_rx, _) = session_of(6, provider, &tmp).await;
    for _ in 0..breaker {
        compact_then_cancel(&action_tx, &mut event_rx, &script).await;
    }

    script.release.notify_one();
    for turn in 6..TURNS {
        run_turn(&action_tx, &mut event_rx, bulky(&format!("TURN-{turn}"))).await;
    }

    let calls = script.calls.lock().await;
    let summaries = calls.iter().filter(|call| CompactingProvider::is_summarization(call)).count();
    assert!(summaries > breaker, "the threshold still compacts: {summaries} summaries");
    let last = calls.last().unwrap();
    assert!(texts(last).iter().any(|text| text.contains(SUMMARY_MARKER)), "and it took effect");
    drop(calls);
    action_tx.send(Action::Close).await.unwrap();
}

/// A cancel during the summary an overflowing prompt forces ends the turn as cancelled, not as an
/// error.
///
/// The break this catches: the retry read an unchanged leaf as "nothing left to summarize" and
/// failed the turn, which a loop counts toward its consecutive-error budget.
#[tokio::test]
async fn a_cancel_during_overflow_compaction_ends_the_turn_as_cancelled() {
    let tmp = tempfile::tempdir().unwrap();
    let (provider, _) = scripted(&[Summary::Stalls], false);
    let (action_tx, event_rx, session_id) = session_of(8, provider, &tmp).await;
    action_tx.send(Action::Close).await.unwrap();

    let store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let (provider, script) = scripted(&[Summary::Stalls], true);
    let (action_tx, mut event_rx2) = Engine::spawn(config(store, session_id, provider, tmp.path()));
    drop(event_rx);
    let submit =
        Action::SubmitUserMessage { content: "and now".into(), images: vec![], remediation: None };
    action_tx.send(submit).await.unwrap();
    summarizer_asked(&script).await;
    action_tx.send(Action::Cancel).await.unwrap();
    let seen = events_until(&mut event_rx2, "the turn to end", |event| {
        matches!(event, Event::TurnEnded { .. })
    })
    .await;

    let Some(Event::TurnEnded { reason }) = seen.last() else { unreachable!() };
    assert_eq!(*reason, luxe_core::event::StopReason::Other("cancelled".into()), "{seen:?}");
    assert!(!seen.iter().any(|e| matches!(e, Event::Error(_))), "{seen:?}");
    action_tx.send(Action::Close).await.unwrap();
}

/// A steer sent while `/compact` runs gets a turn of its own once it finishes, as a steer with no
/// turn running always does.
///
/// The break this catches: serviced like a turn, the compaction buffered the steer for a turn
/// that never came, and it surfaced only after the reader's next message.
#[tokio::test]
async fn a_steer_sent_during_compaction_on_request_runs_as_a_turn_after_it() {
    let tmp = tempfile::tempdir().unwrap();
    let (provider, script) = scripted(&[Summary::AfterRelease], false);
    let (action_tx, mut event_rx, _) = session_of(6, provider, &tmp).await;

    action_tx.send(Action::CompactNow { focus: None }).await.unwrap();
    summarizer_asked(&script).await;
    action_tx.send(Action::SteerTurn { content: "STEERED-WHILE-COMPACTING".into() }).await.unwrap();
    buffered_mid_work(&action_tx, &mut event_rx).await;
    script.release.notify_one();
    let seen = events_until(&mut event_rx, "the steer's turn", |event| {
        matches!(event, Event::TurnEnded { .. })
    })
    .await;

    assert!(seen.iter().any(|e| matches!(e, Event::CompactionDone { .. })), "{seen:?}");
    let calls = script.calls.lock().await;
    let last = calls.last().unwrap();
    assert!(texts(last).iter().any(|text| text == "STEERED-WHILE-COMPACTING"), "{:?}", texts(last));
    drop(calls);
    action_tx.send(Action::Close).await.unwrap();
}

/// Returns once the engine has answered a glance it serves mid-work, so whatever was sent before
/// it reached the work in progress rather than waiting behind it.
async fn buffered_mid_work(action_tx: &Sender, event_rx: &mut Receiver) {
    let request_id = luxe_core::project_files::ListingRequestId::new();
    let path = String::new();
    let listing = Action::ListDirectory { request_id, path, include_ignored: false };
    action_tx.send(listing).await.unwrap();
    events_until(event_rx, "the listing", |event| {
        matches!(event, Event::DirectoryListed { .. } | Event::DirectoryListFailed { .. })
    })
    .await;
}

/// A cancelled `/compact` drops the steers sent during it, the way a cancel with no turn running
/// does: no turn runs for them.
#[tokio::test]
async fn a_cancelled_compaction_runs_no_turn_for_steers_sent_during_it() {
    let tmp = tempfile::tempdir().unwrap();
    let (provider, script) = stalled();
    let (action_tx, mut event_rx, _) = session_of(6, provider, &tmp).await;
    let turns_before = script.calls.lock().await.len();

    action_tx.send(Action::CompactNow { focus: None }).await.unwrap();
    summarizer_asked(&script).await;
    action_tx.send(Action::SteerTurn { content: "STEERED-THEN-CANCELLED".into() }).await.unwrap();
    buffered_mid_work(&action_tx, &mut event_rx).await;
    action_tx.send(Action::Cancel).await.unwrap();
    action_tx.send(Action::RefreshWorkspace).await.unwrap();
    let seen = events_until(&mut event_rx, "the engine to answer", |event| {
        matches!(event, Event::WorkspaceRefreshRequested)
    })
    .await;
    action_tx.send(Action::RefreshWorkspace).await.unwrap();
    let seen_after = events_until(&mut event_rx, "the engine to answer again", |event| {
        matches!(event, Event::WorkspaceRefreshRequested)
    })
    .await;

    for event in seen.iter().chain(&seen_after) {
        assert!(!matches!(event, Event::TurnEnded { .. }), "a turn ran: {event:?}");
    }
    let calls = script.calls.lock().await;
    assert_eq!(calls.len(), turns_before + 1, "only the summary was asked for");
    drop(calls);
    action_tx.send(Action::Close).await.unwrap();
}
