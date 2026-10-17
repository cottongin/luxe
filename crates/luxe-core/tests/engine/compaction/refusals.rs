//! Summaries that must not replace the conversation they were meant to summarize.

use super::*;

/// A summarizer that answers with nothing at all, which a local model on a bad day really does.
struct EmptySummaryProvider {
    calls: Arc<Mutex<Vec<CompletionRequest>>>,
}

#[async_trait::async_trait]
impl Provider for EmptySummaryProvider {
    fn name(&self) -> &str {
        "empty-summary-mock"
    }

    async fn stream(
        &self,
        request: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        let summarizing = CompactingProvider::is_summarization(&request);
        let occupancy = occupancy_of(&request);
        self.calls.lock().await.push(request);

        let mut events = Vec::new();
        if summarizing {
            // Nothing. Not an error, not a refusal — an empty reply.
            events.push(ProviderEvent::TextDelta { index: 0, delta: String::new() });
        } else {
            events.push(ProviderEvent::TextDelta { index: 0, delta: "ok".to_string() });
            events.push(ProviderEvent::Usage(Usage {
                prompt_tokens: occupancy,
                completion_tokens: 1,
                total_tokens: occupancy,
                ..Usage::default()
            }));
        }
        events.push(ProviderEvent::Done { reason: StopReason::EndTurn });
        Ok(Box::pin(stream::iter(events)))
    }
}

/// An empty summary must not be committed over the transcript it was meant to summarize.
///
/// The break this catches, measured on a live loop with a 27B local summarizer: the model returned
/// nothing usable, the formatter supplied the six required headings and the running file list
/// anyway, and a 20,677-token transcript was replaced by that — logged as a successful compaction.
/// The session lost everything it knew, silently, and the next turn started from an empty document
/// of headings.
#[tokio::test]
async fn a_summary_that_says_nothing_does_not_replace_the_conversation() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/test").unwrap();

    let calls = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(EmptySummaryProvider { calls: calls.clone() });
    let (action_tx, mut event_rx) =
        Engine::spawn(config(store, session_id.clone(), provider, tmp.path()));
    for turn in 0..TURNS {
        run_turn(&action_tx, &mut event_rx, bulky(&format!("TURN-{turn}"))).await;
    }
    action_tx.send(Action::Close).await.unwrap();

    let calls = calls.lock().await;
    assert!(
        calls.iter().any(CompactingProvider::is_summarization),
        "the threshold still fired; this is about what happens next"
    );
    let last = calls.last().expect("a final turn request");
    assert!(!CompactingProvider::is_summarization(last), "the last call is the turn itself");
    let sent = texts(last);
    assert!(
        sent.iter().any(|text| text.starts_with("TURN-0")),
        "the conversation an unusable summary could not replace is still there: {sent:?}"
    );

    // And nothing on disk claims a compaction happened.
    let store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let entries = store.load(&session_id).unwrap();
    assert!(
        !entries.iter().any(|entry| matches!(entry.kind, EntryKind::Compaction { .. })),
        "a refused compaction leaves no summary behind"
    );
}

/// How a summarizer's reply can stop short after writing a summary that looks complete.
#[derive(Clone, Copy, Debug)]
enum CutOff {
    Error,
    Length,
    EndOfStream,
}

/// A summarizer that writes a substantive summary, then fails to finish it.
struct CutOffSummaryProvider {
    calls: Arc<Mutex<Vec<CompletionRequest>>>,
    cut_off: CutOff,
}

#[async_trait::async_trait]
impl Provider for CutOffSummaryProvider {
    fn name(&self) -> &str {
        "cut-off-summary-mock"
    }

    async fn stream(
        &self,
        request: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        let summarizing = CompactingProvider::is_summarization(&request);
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
        let mut events = vec![ProviderEvent::TextDelta {
            index: 0,
            delta: format!("## Goal\n{SUMMARY_MARKER}: half of what happened, and then"),
        }];
        match self.cut_off {
            CutOff::Error => events.push(ProviderEvent::Error {
                message: "connection reset".into(),
                kind: luxe_core::provider::ProviderErrorKind::Other("transport".into()),
            }),
            CutOff::Length => events.push(ProviderEvent::Done { reason: StopReason::Length }),
            CutOff::EndOfStream => {}
        }
        Ok(Box::pin(stream::iter(events)))
    }
}

/// A summary the summarizer did not finish must not replace the conversation it summarizes.
///
/// The break this catches: the reply loop took an error, a length stop or a dropped stream as
/// success, and the half-summary that had already arrived passed the content check, so the
/// session's history was replaced by a fragment.
#[tokio::test]
async fn a_summary_cut_off_partway_does_not_replace_the_conversation() {
    for cut_off in [CutOff::Error, CutOff::Length, CutOff::EndOfStream] {
        let tmp = tempfile::tempdir().unwrap();
        let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
        let session_id = store.create("/tmp/test").unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let provider = Arc::new(CutOffSummaryProvider { calls: calls.clone(), cut_off });
        let (action_tx, mut event_rx) =
            Engine::spawn(config(store, session_id.clone(), provider, tmp.path()));
        for turn in 0..TURNS {
            run_turn(&action_tx, &mut event_rx, bulky(&format!("TURN-{turn}"))).await;
        }
        action_tx.send(Action::Close).await.unwrap();

        let calls = calls.lock().await;
        assert!(calls.iter().any(CompactingProvider::is_summarization), "{cut_off:?}: it ran");
        let sent = texts(calls.last().expect("a final turn request"));
        assert!(
            sent.iter().any(|text| text.starts_with("TURN-0")),
            "{cut_off:?}: the conversation is still sent whole"
        );
        let entries =
            JsonlSessionStore::new(tmp.path().join("sessions")).load(&session_id).unwrap();
        assert!(
            !entries.iter().any(|entry| matches!(entry.kind, EntryKind::Compaction { .. })),
            "{cut_off:?}: no summary was committed"
        );
    }
}
