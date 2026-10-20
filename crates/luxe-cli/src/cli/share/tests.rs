use super::*;
use luxe_core::local_endpoint::{connect, LocalStream};
use luxe_core::permission::PermissionRequestId;
use luxe_core::Message;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// A shared session with no engine or terminal behind it, and the far ends of both.
struct Harness {
    shared: Shared,
    engine_rx: mpsc::Receiver<Action>,
    terminal_rx: mpsc::Receiver<Event>,
    /// The strong end `tee` owns in a real session, which keeps the terminal's stream open
    terminal: mpsc::Sender<Event>,
}

/// No process descends from this one, so the tripwire lets the test's own connections in.
const NOBODY: u32 = u32::MAX;

/// A catalog with nothing in it, so no test reads the developer's `models.json`.
struct NoModels;

impl super::super::rpc::ModelResolver for NoModels {
    fn resolve(&self, provider: &str, id: &str) -> luxe_core::ModelInfo {
        let r#ref = luxe_core::provider::ModelRef::new(provider, id);
        luxe_core::ModelInfo { r#ref, ..Default::default() }
    }

    fn choices(&self) -> Vec<luxe_core::event::ModelChoice> {
        Vec::new()
    }
}

fn harness(agent_pid: u32) -> Harness {
    let (engine_tx, engine_rx) = mpsc::channel(16);
    let (terminal, terminal_rx) = mpsc::channel(16);
    let shared = Shared {
        snapshot: Arc::new(Mutex::new(Snapshot::new("session-1".into()))),
        live: broadcast::channel(CLIENT_BACKLOG).0,
        engine_tx,
        terminal: terminal.downgrade(),
        cwd: "/work".into(),
        policy: Policy {
            github_writes: GithubWrites::NotEnabled,
            github_sections: Vec::new(),
            joined: true,
        },
        models: Arc::new(NoModels),
        agent_pid,
        guests: super::guests::GuestList::default(),
    };
    Harness { shared, engine_rx, terminal_rx, terminal }
}

fn permission_asked(id: &PermissionRequestId) -> Event {
    Event::PermissionRequested {
        id: id.clone(),
        session_id: "session-1".into(),
        tool: "bash".into(),
        permission: "bash".into(),
        summary: "Run: cargo test".into(),
        always_patterns: Vec::new(),
    }
}

fn committed(text: &str) -> Event {
    Event::MessageCommitted {
        session_id: "session-1".into(),
        entry_id: luxe_core::session::EntryId(text.into()),
        message: Message::user(text),
    }
}

fn kinds(frames: &[Event]) -> Vec<&'static str> {
    frames
        .iter()
        .map(|frame| match frame {
            Event::SessionLoaded { .. } => "session_loaded",
            Event::StateChanged(_) => "state_changed",
            Event::ContextUsage { .. } => "context_usage",
            Event::PermissionRequested { .. } => "permission_requested",
            Event::TurnStarted => "turn_started",
            _ => "other",
        })
        .collect()
}

/// A joiner opens on the transcript, then the newest of each kept state, then the prompt.
///
/// The break this catches: dropping the commit fold, which opens a joined phone on an empty
/// conversation, or keeping every usage update, which replays a turn's worth of stale readings.
#[test]
fn a_joiner_catches_up_on_the_transcript_state_and_waiting_prompt() {
    let mut snapshot = Snapshot::new("session-1".into());
    let id = PermissionRequestId::new();
    for event in [
        committed("first"),
        Event::ContextUsage { used_tokens: 1, context_window: 10 },
        committed("second"),
        Event::ContextUsage { used_tokens: 2, context_window: 10 },
        permission_asked(&id),
    ] {
        snapshot.observe(&event);
    }
    let frames = snapshot.frames();
    assert_eq!(kinds(&frames), ["context_usage", "permission_requested"]);
    assert_eq!(snapshot.transcript().messages.len(), 2);
    assert!(matches!(frames[0], Event::ContextUsage { used_tokens: 2, .. }));
}

/// A joiner mid-turn is told a turn is running, right after the transcript; one after it isn't.
///
/// The break this catches: leaving `turn_started` out of the catch-up, so a phone opening on a
/// running session shows it idle, with no stop button, until the next token arrives.
#[test]
fn a_joiner_mid_turn_hears_the_turn_started() {
    let mut snapshot = Snapshot::new("session-1".into());
    snapshot.observe(&Event::TurnStarted);
    snapshot.observe(&committed("first"));
    assert_eq!(kinds(&snapshot.frames()), ["turn_started"]);
    snapshot.observe(&Event::TurnEnded { reason: luxe_core::event::StopReason::EndTurn });
    assert!(snapshot.frames().is_empty());
}

/// A prompt someone answered isn't put in front of the next joiner.
///
/// The break this catches: ignoring `prompt_closed`, so a phone joining after the terminal
/// approved shows a prompt whose answer the engine would throw away.
#[test]
fn an_answered_prompt_is_not_replayed() {
    let mut snapshot = Snapshot::new("session-1".into());
    let answered = PermissionRequestId::new();
    let open = PermissionRequestId::new();
    snapshot.observe(&permission_asked(&answered));
    snapshot.observe(&permission_asked(&open));
    snapshot.observe(&Event::PromptClosed { id: answered.as_str().into() });
    assert!(!snapshot.is_waiting(answered.as_str()));
    assert!(snapshot.is_waiting(open.as_str()));
}

/// Loading a session replaces the transcript rather than adding to it.
///
/// The break this catches: appending on `session_loaded`, which shows a joiner the previous
/// session's conversation above the one the terminal switched to.
#[test]
fn a_loaded_session_replaces_what_came_before() {
    let mut snapshot = Snapshot::new("session-1".into());
    snapshot.observe(&committed("old"));
    snapshot.observe(&Event::SessionLoaded { session_id: "session-2".into(), messages: vec![] });
    let transcript = snapshot.transcript();
    assert_eq!(transcript.session_id, "session-2");
    assert!(transcript.messages.is_empty());
}

/// A commit while a joiner is still being sent the transcript leaves the joiner's copy alone.
///
/// The break this catches: the snapshot appending to a list a joiner shares, which would put a
/// message in a `session_loaded` frame the live stream then sends again.
#[test]
fn a_commit_mid_join_leaves_the_joiners_transcript_alone() {
    let mut snapshot = Snapshot::new("session-1".into());
    snapshot.observe(&committed("first"));
    let sending = snapshot.transcript();
    snapshot.observe(&committed("second"));
    assert_eq!(sending.messages.len(), 1);
    assert_eq!(snapshot.transcript().messages.len(), 2);
}

/// Read one frame off a client's stream.
async fn next_frame(lines: &mut tokio::io::Lines<impl tokio::io::AsyncBufRead + Unpin>) -> String {
    let line = tokio::time::timeout(std::time::Duration::from_secs(5), lines.next_line());
    line.await.expect("a frame in time").expect("readable").expect("the stream is open")
}

/// Wait for the terminal to be told something containing `needle`.
async fn notice_containing(terminal_rx: &mut mpsc::Receiver<Event>, needle: &str) -> String {
    let wait = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while let Some(event) = terminal_rx.recv().await {
            if let Event::Notice(text) = event {
                if text.contains(needle) {
                    return text;
                }
            }
        }
        panic!("the terminal stream ended before a notice about {needle:?}");
    });
    wait.await.expect("a notice in time")
}

type ClientLines = tokio::io::Lines<BufReader<tokio::io::ReadHalf<LocalStream>>>;

/// Serve `shared` on a fresh endpoint and connect one client to it.
async fn join(
    shared: &Shared,
) -> (tempfile::TempDir, JoinHandle<()>, ClientLines, tokio::io::WriteHalf<LocalStream>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let listener = EndpointListener::bind(dir.path()).expect("bind");
    let address = listener.address().to_string();
    let accept = tokio::spawn(clients::accept(listener, shared.clone()));
    let stream = connect(&address).await.expect("connect");
    let (read, write) = tokio::io::split(stream);
    (dir, accept, BufReader::new(read).lines(), write)
}

/// A command the agent ran is turned away before it sees a frame, and the terminal hears why.
///
/// The break this catches: skipping the ancestry check, so the agent's own `bash` could join
/// its session and approve the prompt it's waiting on.
#[tokio::test]
async fn the_agents_own_command_is_refused() {
    let Harness { shared, mut engine_rx, mut terminal_rx, terminal: _terminal } =
        harness(std::process::id());
    let (_dir, _accept, mut lines, _write) = join(&shared).await;
    notice_containing(&mut terminal_rx, "Refused").await;
    let read = tokio::time::timeout(std::time::Duration::from_secs(5), lines.next_line());
    assert!(matches!(read.await, Ok(Ok(None)) | Ok(Err(_))), "the refused client got a frame");
    assert!(engine_rx.try_recv().is_err());
}

/// A client joins over the endpoint, catches up, answers, and leaves without closing anything.
///
/// The break this catches: a joined client's hangup reaching the engine as `close`, which would
/// end the terminal's session the moment a phone dropped off the network.
#[tokio::test]
async fn a_client_joins_answers_and_leaves_without_closing_the_session() {
    let Harness { shared, mut engine_rx, mut terminal_rx, terminal: _terminal } = harness(NOBODY);
    let id = PermissionRequestId::new();
    shared.publish(&permission_asked(&id));
    let (_dir, _accept, mut lines, mut write) = join(&shared).await;
    assert!(next_frame(&mut lines).await.contains(r#""kind":"ready""#));
    assert!(next_frame(&mut lines).await.contains(r#""kind":"session_loaded""#));
    assert!(next_frame(&mut lines).await.contains(r#""kind":"permission_requested""#));
    notice_containing(&mut terminal_rx, "joined").await;

    let answer = format!(
        r#"{{"kind":"respond_permission","content":{{"id":"{}","reply":"once"}}}}"#,
        id.as_str()
    );
    write.write_all(format!("{answer}\n").as_bytes()).await.expect("write");
    assert!(matches!(engine_rx.recv().await, Some(Action::RespondPermission { .. })));
    notice_containing(&mut terminal_rx, "Approved from").await;

    drop((lines, write));
    notice_containing(&mut terminal_rx, "left").await;
    assert!(engine_rx.try_recv().is_err(), "leaving must not reach the engine");
}

/// A client that introduces itself is named when it joins and when it answers.
///
/// The break this catches: the terminal saying who joined before the introduction can arrive, or
/// naming the bridge's process, so "Approved from" never says which device answered.
#[tokio::test]
async fn an_introduced_client_is_named_when_it_joins_and_answers() {
    let Harness { shared, mut engine_rx, mut terminal_rx, terminal: _terminal } = harness(NOBODY);
    let id = PermissionRequestId::new();
    shared.publish(&permission_asked(&id));
    let (_dir, _accept, _lines, mut write) = join(&shared).await;
    let introduce = r#"{"kind":"introduce","content":{"name":"iPhone"}}"#;
    write.write_all(format!("{introduce}\n").as_bytes()).await.expect("write");
    notice_containing(&mut terminal_rx, "iPhone joined").await;

    let answer = format!(
        r#"{{"kind":"respond_permission","content":{{"id":"{}","reply":"once"}}}}"#,
        id.as_str()
    );
    write.write_all(format!("{answer}\n").as_bytes()).await.expect("write");
    assert!(matches!(engine_rx.recv().await, Some(Action::RespondPermission { .. })));
    notice_containing(&mut terminal_rx, "Approved from iPhone").await;
}

/// Every notice the terminal hears, up to and including the one containing `needle`.
async fn notices_until(terminal_rx: &mut mpsc::Receiver<Event>, needle: &str) -> Vec<String> {
    let mut heard = Vec::new();
    while !heard.last().is_some_and(|text: &String| text.contains(needle)) {
        heard.push(notice_containing(terminal_rx, "").await);
    }
    heard
}

/// A client that opens as watch-only gets the session but isn't announced, joining or leaving.
/// An ordinary client, joining and leaving after it, marks where its notices would have been.
///
/// The break this catches: the terminal saying "Notifications joined this session" in every
/// session while a phone has notifications on.
#[tokio::test]
async fn a_watch_only_client_comes_and_goes_unannounced() {
    let Harness { shared, engine_rx: _engine_rx, mut terminal_rx, terminal: _terminal } =
        harness(NOBODY);
    let (_dir, _accept, mut lines, mut write) = join(&shared).await;
    let watcher = r#"{"kind":"introduce","content":{"name":"Notifications","watch_only":true}}"#;
    write.write_all(format!("{watcher}\n").as_bytes()).await.expect("write");
    assert!(next_frame(&mut lines).await.contains(r#""kind":"ready""#));
    drop((lines, write));

    let (_dir, _accept, lines, mut write) = join(&shared).await;
    let introduce = r#"{"kind":"introduce","content":{"name":"iPad"}}"#;
    write.write_all(format!("{introduce}\n").as_bytes()).await.expect("write");
    let mut heard = notices_until(&mut terminal_rx, "iPad joined").await;
    drop((lines, write));
    heard.extend(notices_until(&mut terminal_rx, "iPad left").await);
    assert!(heard.iter().all(|text| !text.contains("Notifications")), "{heard:?}");
}

/// Everything the terminal hears until it's told the guests are `expected`.
async fn heard_until_guests(
    terminal_rx: &mut mpsc::Receiver<Event>,
    expected: &[&str],
) -> Vec<Event> {
    let wait = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let mut heard = Vec::new();
        while let Some(event) = terminal_rx.recv().await {
            let done = matches!(&event, Event::GuestsChanged(guests) if guests == expected);
            heard.push(event);
            if done {
                return heard;
            }
        }
        panic!("the terminal stream ended before the guests were {expected:?}");
    });
    wait.await.expect("the guests in time")
}

fn notices(heard: &[Event]) -> Vec<&str> {
    heard
        .iter()
        .filter_map(|event| match event {
            Event::Notice(text) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

/// The owner's own device joins and leaves the dock's list without a line in the transcript.
///
/// The break this catches: a phone's every reconnect adding "joined" and "left" lines to the
/// owner's transcript, or the dock never told it came or went.
#[tokio::test]
async fn the_owners_device_is_listed_quietly() {
    let Harness { shared, engine_rx: _engine_rx, mut terminal_rx, terminal: _terminal } =
        harness(NOBODY);
    let (_dir, _accept, lines, mut write) = join(&shared).await;
    let yours = r#"{"kind":"introduce","content":{"name":"iPhone","yours":true}}"#;
    write.write_all(format!("{yours}\n").as_bytes()).await.expect("write");
    let mut heard = heard_until_guests(&mut terminal_rx, &["iPhone"]).await;
    drop((lines, write));
    heard.extend(heard_until_guests(&mut terminal_rx, &[]).await);
    assert!(notices(&heard).is_empty(), "{heard:?}");
}

/// Someone else's device is listed and still announced, joining and leaving.
///
/// The break this catches: a stranger's device joining the owner's session unremarked, or kept
/// off the dock's list.
#[tokio::test]
async fn anyone_elses_device_is_listed_and_announced() {
    let Harness { shared, engine_rx: _engine_rx, mut terminal_rx, terminal: _terminal } =
        harness(NOBODY);
    let (_dir, _accept, lines, mut write) = join(&shared).await;
    let theirs = r#"{"kind":"introduce","content":{"name":"Sam's iPad"}}"#;
    write.write_all(format!("{theirs}\n").as_bytes()).await.expect("write");
    let mut heard = heard_until_guests(&mut terminal_rx, &["Sam's iPad"]).await;
    drop((lines, write));
    heard.extend(notices_until(&mut terminal_rx, "left").await.into_iter().map(Event::Notice));
    let said = notices(&heard);
    assert!(said.iter().any(|text| text.contains("Sam's iPad joined")), "{said:?}");
    assert!(said.iter().any(|text| text.contains("Sam's iPad left")), "{said:?}");
}

/// The terminal's stream ends when the engine's does, even with a client still connected.
///
/// The break this catches: a connected client holding the terminal's stream open, so `luxe --rpc`
/// never sees the session end and never exits.
#[tokio::test]
async fn the_terminal_stream_ends_with_the_engines() {
    let Harness { shared, engine_rx: _engine_rx, mut terminal_rx, terminal } = harness(NOBODY);
    let (events_tx, events) = mpsc::channel(4);
    let (_dir, _accept, mut lines, _write) = join(&shared).await;
    assert!(next_frame(&mut lines).await.contains(r#""kind":"ready""#));
    notice_containing(&mut terminal_rx, "joined").await;

    let tee = tokio::spawn(tee(events, shared, terminal, None));
    drop(events_tx);
    tee.await.expect("tee ends with the engine's stream");
    let end = tokio::time::timeout(std::time::Duration::from_secs(5), terminal_rx.recv());
    assert_eq!(end.await.expect("the stream ends in time"), None);
}
