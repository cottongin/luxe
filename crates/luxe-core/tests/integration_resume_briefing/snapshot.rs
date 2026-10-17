//! The system prompt is written once per session: the turns after it send it byte for byte, and
//! what changes meanwhile reaches the model as appended context.

use super::*;

/// Records the system prompt and every user text of each request.
#[derive(Default)]
struct Recording {
    requests: Mutex<Vec<(String, Vec<String>)>>,
}

struct RecordingProvider(Arc<Recording>);

#[async_trait::async_trait]
impl Provider for RecordingProvider {
    fn name(&self) -> &str {
        "recording"
    }

    async fn stream(
        &self,
        request: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        let texts = request
            .messages
            .iter()
            .filter(|message| message.role == luxe_core::message::Role::User)
            .flat_map(|message| &message.content)
            .filter_map(|block| match block {
                luxe_core::message::ContentBlock::Text(text) => Some(text.clone()),
                _ => None,
            })
            .collect();
        let system = request.system.clone().unwrap_or_default();
        self.0.requests.lock().await.push((system, texts));
        Ok(Box::pin(stream::iter(vec![
            ProviderEvent::TextDelta { index: 0, delta: "ok".to_string() },
            ProviderEvent::Done { reason: StopReason::EndTurn },
        ])))
    }
}

async fn turn(tx: &tokio::sync::mpsc::Sender<Action>, rx: &mut tokio::sync::mpsc::Receiver<Event>) {
    let content = "continue".to_string();
    tx.send(Action::SubmitUserMessage { content, images: vec![], remediation: None })
        .await
        .unwrap();
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv()).await {
            Ok(Some(Event::TurnEnded { .. })) => break,
            Ok(Some(_)) => {}
            Ok(None) => panic!("event channel closed"),
            Err(_) => panic!("timed out"),
        }
    }
}

/// A resumed session with open todos, launched in `project`, recording into the returned log.
fn resumed(
    tmp: &tempfile::TempDir,
    project: &std::path::Path,
) -> (tokio::sync::mpsc::Sender<Action>, tokio::sync::mpsc::Receiver<Event>, Arc<Recording>) {
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let sid = store.create("/tmp/test").unwrap();
    store.append(&sid, None, EntryKind::Message(Message::user("earlier work"))).unwrap();
    seed_todos(tmp.path(), &sid.0);
    let recording = Arc::new(Recording::default());
    let provider = Arc::new(RecordingProvider(recording.clone()));
    let mut config = config(store, sid, tmp.path().join("todos"), provider);
    config.invoked_from = Some(project.to_path_buf());
    let (tx, rx) = Engine::spawn(config);
    (tx, rx, recording)
}

/// Two turns after a resume send the same system prompt, byte for byte, and the briefing reaches
/// the model once, as part of the conversation.
///
/// The break this catches: the briefing was a transient system section, added on the first turn
/// after a resume and gone on the next, so the system string changed between them and the
/// provider's cached prefix was thrown away on exactly the turns that carry the most context.
#[tokio::test]
async fn turns_after_a_resume_send_one_system_prompt_and_the_briefing_once() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let (tx, mut rx, recording) = resumed(&tmp, project.path());
    turn(&tx, &mut rx).await;
    turn(&tx, &mut rx).await;
    tx.send(Action::Close).await.ok();

    let requests = recording.requests.lock().await;
    let [(first, _), (second, conversation)] = &requests[..] else {
        panic!("two requests: {}", requests.len())
    };
    assert_eq!(first, second, "the system prompt moved between turns");
    assert!(!first.contains("## Resuming"), "the briefing is not a system section");
    let briefings = conversation.iter().filter(|text| text.contains("## Resuming")).count();
    assert_eq!(briefings, 1, "delivered once, and kept as history: {conversation:#?}");
    assert!(conversation.iter().any(|text| text.contains("#1 [in progress] split the module")));
    let noted = conversation.iter().any(|text| text.starts_with("[branch: "));
    assert!(!noted, "nothing moved, so nothing is said about the branch: {conversation:#?}");
}

/// An instruction file edited mid-session reaches the next session, not the next turn.
///
/// The break this catches: every turn re-read `AGENTS.md`, so an edit mid-session rewrote the
/// prompt every later request was cached against.
#[tokio::test]
async fn editing_agents_md_between_turns_leaves_the_system_prompt_alone_until_a_new_session() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("AGENTS.md"), "- FIRST-RULE\n").unwrap();
    let (tx, mut rx, recording) = resumed(&tmp, project.path());
    turn(&tx, &mut rx).await;
    std::fs::write(project.path().join("AGENTS.md"), "- SECOND-RULE\n").unwrap();
    turn(&tx, &mut rx).await;
    tx.send(Action::NewSession).await.unwrap();
    turn(&tx, &mut rx).await;
    tx.send(Action::Close).await.ok();

    let requests = recording.requests.lock().await;
    let systems: Vec<&String> = requests.iter().map(|(system, _)| system).collect();
    assert!(systems[0].contains("FIRST-RULE"), "{}", systems[0]);
    assert_eq!(systems[0], systems[1], "an edit mid-session leaves the prompt as it was");
    assert!(systems[2].contains("SECOND-RULE"), "a new session reads the edit: {}", systems[2]);
}

/// `/context` measures the prompt the session sends, not the files as they are now.
#[tokio::test]
async fn context_measures_the_prompt_the_session_sends() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("AGENTS.md"), "- short\n").unwrap();
    let (tx, mut rx, _) = resumed(&tmp, project.path());
    turn(&tx, &mut rx).await;
    let before = measure(&tx, &mut rx).await;
    std::fs::write(project.path().join("AGENTS.md"), format!("- {}\n", "long ".repeat(4_000)))
        .unwrap();
    let after = measure(&tx, &mut rx).await;
    tx.send(Action::Close).await.ok();

    let instructions = |composition: &luxe_core::context_report::ContextComposition| {
        composition
            .slices
            .iter()
            .filter(|slice| slice.part == luxe_core::context_report::ContextPart::Instructions)
            .map(|slice| slice.tokens)
            .sum::<u64>()
    };
    assert_eq!(instructions(&before), instructions(&after), "{before:#?}\n{after:#?}");
}

/// What `/context` reports, asked for now.
async fn measure(
    tx: &tokio::sync::mpsc::Sender<Action>,
    rx: &mut tokio::sync::mpsc::Receiver<Event>,
) -> Box<luxe_core::context_report::ContextComposition> {
    tx.send(Action::RequestContextComposition).await.unwrap();
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv()).await {
            Ok(Some(Event::ContextComposition(composition))) => break composition,
            Ok(Some(_)) => {}
            other => panic!("no composition: {other:?}"),
        }
    }
}

/// The branch the checkout tests run in, if it is a repository.
fn checkout_branch() -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// A session whose history last told the model of another branch tells it where the checkout is
/// now, once.
///
/// The break this catches: the prompt is frozen, so a branch change mid-session reached the model
/// nowhere — and a note remembered rather than read off the conversation went missing for good
/// once a rewind or a compaction took it away.
#[tokio::test]
async fn a_branch_change_is_told_once_with_the_next_message() {
    let Some(branch) = checkout_branch() else { return };
    let tmp = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let sid = store.create("/tmp/test").unwrap();
    let mut earlier = Message::user("earlier work");
    earlier.content.push(luxe_core::message::ContentBlock::Text(
        "[branch: not-this-branch] The checkout is now on branch `not-this-branch`.".into(),
    ));
    store.append(&sid, None, EntryKind::Message(earlier)).unwrap();
    let recording = Arc::new(Recording::default());
    let provider = Arc::new(RecordingProvider(recording.clone()));
    let mut config = config(store, sid, tmp.path().join("todos"), provider);
    config.invoked_from = Some(project.path().to_path_buf());
    let (tx, mut rx) = Engine::spawn(config);
    turn(&tx, &mut rx).await;
    turn(&tx, &mut rx).await;
    tx.send(Action::Close).await.ok();

    let requests = recording.requests.lock().await;
    let told = format!("[branch: {branch}]");
    let notes = |texts: &[String]| texts.iter().filter(|text| text.starts_with(&told)).count();
    assert_eq!(notes(&requests[0].1), 1, "told on the next message: {:#?}", requests[0].1);
    assert_eq!(notes(&requests[1].1), 1, "and not again: {:#?}", requests[1].1);
}

/// Fails the call numbered `failing` (from zero), and answers every other.
struct FailsOnce(Arc<Recording>, std::sync::atomic::AtomicUsize, usize);

#[async_trait::async_trait]
impl Provider for FailsOnce {
    fn name(&self) -> &str {
        "fails-once"
    }

    async fn stream(
        &self,
        request: CompletionRequest,
        cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        if self.1.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == self.2 {
            return Err(ProviderError::Other("invalid request".into()));
        }
        RecordingProvider(self.0.clone()).stream(request, cancel).await
    }
}

/// A resumed session on a provider that fails call `failing`, with `turns` turns sent and the
/// last retried; returns every text the retry carried.
async fn retried_after(turns: usize, failing: usize) -> Vec<String> {
    let tmp = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let sid = store.create("/tmp/test").unwrap();
    store.append(&sid, None, EntryKind::Message(Message::user("earlier work"))).unwrap();
    seed_todos(tmp.path(), &sid.0);
    let recording = Arc::new(Recording::default());
    let provider = Arc::new(FailsOnce(recording.clone(), Default::default(), failing));
    let mut config = config(store, sid, tmp.path().join("todos"), provider);
    config.invoked_from = Some(project.path().to_path_buf());
    let (tx, mut rx) = Engine::spawn(config);
    for _ in 0..turns {
        turn(&tx, &mut rx).await;
    }
    tx.send(Action::Retry).await.unwrap();
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv()).await {
            Ok(Some(Event::TurnEnded { .. })) => break,
            Ok(Some(_)) => {}
            other => panic!("the retry did not end: {other:?}"),
        }
    }
    tx.send(Action::Close).await.ok();
    let requests = recording.requests.lock().await;
    requests.last().expect("the retry was sent").1.clone()
}

/// Retrying a later message of a resumed session leaves the briefing where it already is: once,
/// on the first message.
#[tokio::test]
async fn a_retry_of_a_later_message_does_not_brief_again() {
    let texts = retried_after(2, 1).await;
    let briefings = texts.iter().filter(|text| text.starts_with("## Resuming")).count();
    assert_eq!(briefings, 1, "{texts:#?}");
}

/// Retrying a message sends what the reader wrote, not the briefing that rode along with it, and
/// the briefing still rides along with the re-send.
///
/// The break this catches: the retry re-sent the message's whole text, so the resume briefing
/// became part of the reader's own words, shown as theirs and sent twice.
#[tokio::test]
async fn a_retry_after_a_resume_sends_only_the_reader_s_words() {
    let texts = retried_after(1, 0).await;
    assert!(texts.contains(&"continue".to_string()), "the reader's words alone: {texts:#?}");
    assert!(!texts.iter().any(|text| text.starts_with("continue\n")), "{texts:#?}");
    let briefings = texts.iter().filter(|text| text.starts_with("## Resuming")).count();
    assert_eq!(briefings, 1, "and the briefing rides with the re-send: {texts:#?}");
}

/// A message the reader opened with what looks like a branch note is still their words, not a
/// note: the branch the model last heard is the prompt's, and nothing is said.
#[tokio::test]
async fn a_message_that_starts_like_a_branch_note_is_not_one() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let sid = store.create("/tmp/test").unwrap();
    let pasted = Message::user("[branch: not-this-branch] The checkout is now on branch `x`.");
    store.append(&sid, None, EntryKind::Message(pasted)).unwrap();
    let recording = Arc::new(Recording::default());
    let provider = Arc::new(RecordingProvider(recording.clone()));
    let mut config = config(store, sid, tmp.path().join("todos"), provider);
    config.invoked_from = Some(project.path().to_path_buf());
    let (tx, mut rx) = Engine::spawn(config);
    turn(&tx, &mut rx).await;
    turn(&tx, &mut rx).await;
    tx.send(Action::Close).await.ok();

    let requests = recording.requests.lock().await;
    let texts = &requests[1].1;
    let notes = texts.iter().filter(|text| text.starts_with("[branch: ")).count();
    assert_eq!(notes, 1, "only the pasted message itself: {texts:#?}");
}
