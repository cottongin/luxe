//! Empty replies — a call that produced nothing commits nothing, and an empty reply is scrubbed.

use super::*;

/// Answers with nothing at all — the shape a stream dropped by a cancel leaves behind.
struct SilentProvider;

#[async_trait::async_trait]
impl Provider for SilentProvider {
    fn name(&self) -> &str {
        "silent"
    }

    async fn stream(
        &self,
        _request: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        Ok(Box::pin(stream::iter(Vec::new())))
    }
}

/// A call that produces nothing commits nothing, and says so.
///
/// The placeholder this used to write was an empty text block, which Anthropic answers with
/// `400 text content blocks must be non-empty` on every later request — so the reply nobody saw
/// took the session with it.
#[tokio::test]
async fn a_call_that_comes_back_empty_commits_nothing_and_says_so() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/test").unwrap();

    let reader = JsonlSessionStore::new(tmp.path().join("sessions"));
    let (action_tx, mut event_rx) = Engine::spawn(config(
        store,
        session_id.clone(),
        Arc::new(SilentProvider),
        ToolRegistry::new(),
    ));

    action_tx
        .send(Action::SubmitUserMessage {
            content: "are you there".to_string(),
            images: vec![],
            remediation: None,
        })
        .await
        .unwrap();

    let reported =
        events_until_turn_end(&mut event_rx).await.into_iter().find_map(|event| match event {
            Event::Error(message) => Some(message),
            _ => None,
        });
    let reported = reported.expect("an answer nobody can see is still an answer to report");
    assert!(reported.contains("empty response"), "{reported}");

    let messages: Vec<Message> = reader
        .load(&session_id)
        .unwrap()
        .into_iter()
        .filter_map(|entry| match entry.kind {
            EntryKind::Message(message) => Some(message),
            _ => None,
        })
        .collect();
    assert_eq!(messages.len(), 1, "only the user's own message is in the transcript");
    assert_eq!(messages[0].role, Role::User);

    let _ = action_tx.send(Action::Close).await;
}

/// A session an older build poisoned still resumes: the empty block is dropped on the way to the
/// provider, so no migration is needed and the durable log keeps saying what actually happened.
#[tokio::test]
async fn a_transcript_that_already_carries_an_empty_reply_still_reaches_the_provider_valid() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/test").unwrap();

    // Copied out of the session this fixed: the interrupted call's result, the message the user
    // sent next, and the empty reply the cancelled stream committed in place of an answer.
    let user = store
        .append(&session_id, None, EntryKind::Message(Message::user("Strata is open.")))
        .unwrap();
    store
        .append(
            &session_id,
            Some(user),
            EntryKind::Message(Message {
                role: Role::Assistant,
                content: vec![ContentBlock::Text(String::new())],
            }),
        )
        .unwrap();

    let calls = Arc::new(Mutex::new(Vec::new()));
    let (action_tx, mut event_rx) = Engine::spawn(config(
        store,
        session_id.clone(),
        Arc::new(EchoProvider { calls: calls.clone() }),
        ToolRegistry::new(),
    ));

    action_tx.send(Action::LoadSession(session_id.to_string())).await.unwrap();
    action_tx
        .send(Action::SubmitUserMessage {
            content: "Please continue.".to_string(),
            images: vec![],
            remediation: None,
        })
        .await
        .unwrap();
    events_until_turn_end(&mut event_rx).await;

    let requests = calls.lock().await;
    let request = requests.first().expect("the turn made a provider call");
    assert!(
        !request
            .messages
            .iter()
            .flat_map(|message| &message.content)
            .any(|block| matches!(block, ContentBlock::Text(text) if text.trim().is_empty())),
        "the empty block must not reach the wire: {:#?}",
        request.messages
    );
    assert!(
        request.messages.iter().all(|message| !message.content.is_empty()),
        "and nothing may be left as a message with no content"
    );

    // The durable log is a record, not a workspace: it still shows the reply that never came.
    let reader = JsonlSessionStore::new(tmp.path().join("sessions"));
    let persisted: Vec<Message> = reader
        .load(&session_id)
        .unwrap()
        .into_iter()
        .filter_map(|entry| match entry.kind {
            EntryKind::Message(message) => Some(message),
            _ => None,
        })
        .collect();
    assert!(
        persisted.iter().any(|message| message.content == vec![ContentBlock::Text(String::new())]),
        "the empty reply stays in the record"
    );

    let _ = action_tx.send(Action::Close).await;
}
