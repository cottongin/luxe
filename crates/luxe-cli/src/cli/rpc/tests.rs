use super::*;
use luxe_core::provider::ModelInfo;

/// A resolver with no dependence on the developer's `models.json`.
struct FixedModels;

impl ModelResolver for FixedModels {
    fn resolve(&self, provider: &str, id: &str) -> ModelInfo {
        ModelInfo { r#ref: luxe_core::provider::ModelRef::new(provider, id), ..Default::default() }
    }
}

fn policy(github_writes: GithubWrites) -> Policy {
    Policy { github_writes, github_sections: Vec::new() }
}

fn parse(line: &str) -> Result<Action, Rejected> {
    parse_action(line, &FixedModels, &policy(GithubWrites::NotEnabled))
}

/// A well-formed frame becomes the engine action it names.
#[test]
fn a_good_line_translates() {
    let action =
        parse(r#"{"kind":"submit_user_message","content":{"text":"hi"}}"#).expect("a valid frame");
    assert!(matches!(action, Action::SubmitUserMessage { content, .. } if content == "hi"));
}

/// A line that is not JSON is refused without a `kind`, because it never had one.
///
/// The break this catches: inventing a kind for an unparseable line, which would send a client
/// looking for a bug in a frame it never wrote.
#[test]
fn a_line_that_is_not_json_is_refused_without_a_kind() {
    let rejected = parse("not json at all").expect_err("garbage is not a frame");
    assert_eq!(rejected.kind, None);
    assert!(!rejected.reason.is_empty());
}

/// A known frame with a bad payload is refused, and the reason names the frame.
#[test]
fn a_known_frame_with_a_bad_payload_is_refused_by_name() {
    let rejected =
        parse(r#"{"kind":"steer_turn","content":{}}"#).expect_err("steer_turn needs text");
    assert!(rejected.reason.contains("steer_turn"), "{}", rejected.reason);
}

/// An unknown frame is refused with its kind, so a client can tell "not implemented" from
/// "malformed" without reading the harness's source.
#[test]
fn an_unknown_frame_is_refused_with_its_kind() {
    let rejected = parse(r#"{"kind":"launch_missiles","content":{}}"#).expect_err("no such action");
    assert_eq!(rejected.kind.as_deref(), Some("launch_missiles"));
}

// === Whole-session tests, over an in-memory duplex ===

/// Everything a session needs: the frames written out, and the actions the engine received.
struct Session {
    frames: Vec<serde_json::Value>,
    actions: Vec<Action>,
}

/// Run one RPC session over in-memory pipes, feeding `input` and collecting both directions.
///
/// The engine is a pair of channels rather than a real one: this is a test of the protocol
/// layer, and what that layer promises is that frames arrive in order and actions reach the
/// channel. `events` is what a real engine would emit.
///
/// Two pipes rather than one duplex, because that is what stdio is — and because dropping one
/// half of a *split* `DuplexStream` does not signal EOF to the peer, only dropping the whole
/// stream does. A harness built the other way hangs instead of ending the session, which is a
/// property of the fake and would have been mistaken for one of the code.
async fn run_session(input: &str, events: Vec<Event>) -> Session {
    let (mut client_write, server_read) = tokio::io::duplex(64 * 1024);
    let (server_write, mut client_read) = tokio::io::duplex(64 * 1024);
    let (engine_tx, mut engine_rx) = mpsc::channel::<Action>(64);
    let (event_tx, event_rx) = mpsc::channel::<Event>(64);

    client_write.write_all(input.as_bytes()).await.unwrap();
    drop(client_write); // EOF: the client is done talking

    // Drain concurrently so a session that outgrows the pipe buffer cannot deadlock the test.
    let output = tokio::spawn(async move {
        let mut raw = String::new();
        tokio::io::AsyncReadExt::read_to_string(&mut client_read, &mut raw).await.unwrap();
        raw
    });

    // Stand in for the engine, faithfully in the one way that matters here: its event stream
    // outlives the client's input and ends only after `Close`. A fake that closed the stream
    // as soon as it ran out of scripted events would race the reader and make the assertions
    // below depend on task scheduling.
    let closing_tx = event_tx.clone();
    let collector = tokio::spawn(async move {
        let mut actions = Vec::new();
        while let Some(action) = engine_rx.recv().await {
            let closing = matches!(action, Action::Close);
            actions.push(action);
            if closing {
                break;
            }
        }
        drop(closing_tx);
        actions
    });
    let feeder = tokio::spawn(async move {
        for event in events {
            if event_tx.send(event).await.is_err() {
                return;
            }
        }
    });

    let ready = Event::Ready {
        protocol_version: luxe_core::PROTOCOL_VERSION,
        luxe_version: "0.1.0".into(),
        session_id: "session-1".into(),
        cwd: "/work/luxe".into(),
        github_writes: false,
    };
    let served = serve(
        ready,
        event_rx,
        engine_tx,
        server_read,
        server_write,
        &FixedModels,
        policy(GithubWrites::NotEnabled),
    );
    tokio::time::timeout(std::time::Duration::from_secs(5), served)
        .await
        .expect("the session must end once input and events are exhausted")
        .expect("serve");
    feeder.await.unwrap();

    let raw = output.await.unwrap();
    Session {
        frames: raw
            .lines()
            .map(|line| {
                serde_json::from_str(line)
                    .unwrap_or_else(|e| panic!("stdout carried a non-JSON line {line:?}: {e}"))
            })
            .collect(),
        actions: collector.await.unwrap(),
    }
}

/// The handshake is the first frame, before anything the engine has to say.
///
/// The break this catches: emitting `ready` after the first engine event, which would make a
/// client that reads one line to check the version read a token delta instead.
#[tokio::test]
async fn the_handshake_leads_the_stream() {
    let session =
        run_session("", vec![Event::Notice("an engine had something to say first".into())]).await;
    assert_eq!(session.frames[0]["kind"], "ready");
    assert_eq!(session.frames[0]["content"]["protocol_version"], 2);
    assert_eq!(session.frames[0]["content"]["session_id"], "session-1");
}

/// An EOF on the input closes the engine rather than leaving it running behind a dead stream.
#[tokio::test]
async fn a_client_that_goes_away_closes_the_engine() {
    let session = run_session("", Vec::new()).await;
    assert!(
        session.actions.iter().any(|action| matches!(action, Action::Close)),
        "EOF must reach the engine as Close, got {:?}",
        session.actions
    );
}

/// The permission round-trip: the prompt reaches the client and the answer reaches the engine.
///
/// This is the whole reason RPC resolves `ask` to `Prompt` instead of auto-allowing. The break
/// it catches: dropping the reply's id, which would leave the turn blocked on a prompt that was
/// already answered.
#[tokio::test]
async fn a_permission_prompt_is_answered_by_the_client() {
    let prompt = Event::PermissionRequested {
        id: serde_json::from_value(serde_json::json!("perm:9")).unwrap(),
        session_id: "session-1".into(),
        tool: "bash".into(),
        permission: "bash".into(),
        summary: "Run: cargo test".into(),
        always_patterns: vec!["cargo *".into()],
    };
    let session = run_session(
        "{\"kind\":\"respond_permission\",\"content\":{\"id\":\"perm:9\",\"reply\":\"once\"}}\n",
        vec![prompt],
    )
    .await;

    let asked = session
        .frames
        .iter()
        .find(|frame| frame["kind"] == "permission_requested")
        .expect("the prompt must reach the client");
    assert_eq!(asked["content"]["summary"], "Run: cargo test");

    let answered = session
        .actions
        .iter()
        .find(|action| matches!(action, Action::RespondPermission { .. }))
        .expect("the answer must reach the engine");
    assert!(matches!(
        answered,
        Action::RespondPermission { id, reply: luxe_core::permission::PermissionReply::Once }
            if id.as_str() == "perm:9"
    ));
}

/// A bad line is refused and the session carries on with the next one.
///
/// The break this catches: treating a parse failure as fatal, which would let one typo in a
/// client end a working session.
#[tokio::test]
async fn one_bad_line_does_not_end_the_session() {
    let session = run_session(
        "not json\n{\"kind\":\"steer_turn\",\"content\":{}}\n{\"kind\":\"cancel\"}\n",
        Vec::new(),
    )
    .await;

    let refusals: Vec<&serde_json::Value> =
        session.frames.iter().filter(|frame| frame["kind"] == "action_rejected").collect();
    assert_eq!(refusals.len(), 2, "both bad lines are refused: {:?}", session.frames);
    assert!(refusals[0]["content"].get("kind").is_none(), "garbage has no kind");
    assert_eq!(refusals[1]["content"]["kind"], "steer_turn", "a known frame is named");
    assert!(
        session.actions.iter().any(|action| matches!(action, Action::Cancel)),
        "the good line after two bad ones still ran"
    );
}

/// A blank line is not a frame, and must not be answered with a refusal.
///
/// Trailing newlines are what every line-oriented client produces; answering them with an
/// error would make a correct client look broken.
#[tokio::test]
async fn blank_lines_are_not_frames() {
    let session = run_session("\n   \n\t\n{\"kind\":\"cancel\"}\n", Vec::new()).await;
    assert!(
        !session.frames.iter().any(|frame| frame["kind"] == "action_rejected"),
        "whitespace is not an error: {:?}",
        session.frames
    );
    assert!(session.actions.iter().any(|action| matches!(action, Action::Cancel)));
}

/// `close` reaches the engine, and the reader stops asking for more.
#[tokio::test]
async fn close_ends_the_session() {
    let session = run_session("{\"kind\":\"close\"}\n{\"kind\":\"cancel\"}\n", Vec::new()).await;
    assert!(matches!(session.actions.first(), Some(Action::Close)));
    assert!(
        !session.actions.iter().any(|action| matches!(action, Action::Cancel)),
        "nothing after `close` is forwarded, got {:?}",
        session.actions
    );
}

/// A client writing Windows line endings is understood.
///
/// Three platforms are first-class, and a client on the one that ends its lines `\r\n` is the
/// obvious way a line-oriented protocol breaks on it. Today it holds twice over — tokio's
/// `lines()` strips the `\r`, and JSON counts a stray one as whitespace anyway — so this cannot
/// fail against the current framing, and it is kept for the framing that replaces it. Anything
/// stricter (a length prefix, a hand-rolled `strip_suffix("\n")`, a parser that rejects trailing
/// bytes) fails here rather than in a bug report from the one platform nobody develops on.
#[tokio::test]
async fn crlf_line_endings_are_frames_too() {
    let session =
        run_session("{\"kind\":\"cancel\"}\r\n{\"kind\":\"close\"}\r\n", Vec::new()).await;
    assert!(
        !session.frames.iter().any(|frame| frame["kind"] == "action_rejected"),
        "a CRLF client is not a broken client: {:?}",
        session.frames
    );
    assert!(
        session.actions.iter().any(|action| matches!(action, Action::Cancel)),
        "the frame must survive its own line ending, got {:?}",
        session.actions
    );
}

/// A write that fails for any reason *other* than a hangup is still an error.
///
/// The hangup case below makes `luxe --rpc | head` exit 0, and the tempting way to write it is
/// to treat every write failure as "the client left" — `cargo mutants` confirmed that replacing
/// the error-kind guard with `true` went unnoticed. It should not: a full disk or a broken
/// redirect would then end the session reporting success, and the frames nobody received would
/// look delivered.
#[tokio::test]
async fn a_real_write_failure_is_not_mistaken_for_a_hangup() {
    /// A sink that fails the way a full disk does, not the way a closed pipe does.
    struct FailingWriter;

    impl AsyncWrite for FailingWriter {
        fn poll_write(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
            _: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            std::task::Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::StorageFull,
                "no space left on device",
            )))
        }

        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }

        fn poll_shutdown(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
    }

    let (client_write, server_read) = tokio::io::duplex(1024);
    drop(client_write);
    let (engine_tx, mut engine_rx) = mpsc::channel::<Action>(8);
    let (_event_tx, event_rx) = mpsc::channel::<Event>(8);
    tokio::spawn(async move { while engine_rx.recv().await.is_some() {} });

    let ready = Event::Ready {
        protocol_version: luxe_core::PROTOCOL_VERSION,
        luxe_version: "0.1.0".into(),
        session_id: "session-1".into(),
        cwd: "/work/luxe".into(),
        github_writes: false,
    };
    let served = serve(
        ready,
        event_rx,
        engine_tx,
        server_read,
        FailingWriter,
        &FixedModels,
        policy(GithubWrites::NotEnabled),
    );
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), served)
        .await
        .expect("a failing write must not hang the session");
    assert!(outcome.is_err(), "a storage failure is an error, not a clean end");
}

/// A client that stops reading ends the session cleanly rather than raising an error.
///
/// The break this catches: propagating `BrokenPipe` out of `run_rpc`, which makes the
/// ordinary `luxe --rpc | head` exit non-zero with an error nobody can read — it goes to a
/// pipe that is already closed.
#[tokio::test]
async fn a_client_that_stops_reading_is_not_an_error() {
    let (client_write, server_read) = tokio::io::duplex(1024);
    let (server_write, client_read) = tokio::io::duplex(1024);
    let (engine_tx, mut engine_rx) = mpsc::channel::<Action>(64);
    let (event_tx, event_rx) = mpsc::channel::<Event>(64);

    drop(client_read); // the client hung up before reading a single frame
    drop(client_write);
    tokio::spawn(async move { while engine_rx.recv().await.is_some() {} });
    tokio::spawn(async move {
        for _ in 0..64 {
            if event_tx.send(Event::Notice("still talking".into())).await.is_err() {
                return;
            }
        }
    });

    let ready = Event::Ready {
        protocol_version: luxe_core::PROTOCOL_VERSION,
        luxe_version: "0.1.0".into(),
        session_id: "session-1".into(),
        cwd: "/work/luxe".into(),
        github_writes: false,
    };
    let served = serve(
        ready,
        event_rx,
        engine_tx,
        server_read,
        server_write,
        &FixedModels,
        policy(GithubWrites::NotEnabled),
    );
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), served)
        .await
        .expect("a hangup must end the session promptly");
    assert!(outcome.is_ok(), "a client hangup is a clean end: {outcome:?}");
}

/// Every frame written is one line of JSON — the framing rule a client parses by.
///
/// `run_session` panics on any line that will not parse, so a stray `println!` anywhere in this
/// path fails here. The multi-line payload is the interesting case: a newline inside a string
/// must be escaped rather than splitting the frame in two.
#[tokio::test]
async fn every_frame_is_exactly_one_line() {
    let session = run_session(
        "",
        vec![
            Event::Notice("one\ntwo\nthree".into()),
            Event::TokenDelta { block: 0, delta: "a\nb".into() },
        ],
    )
    .await;
    assert_eq!(session.frames.len(), 3, "ready + two events, no more, no fewer");
    assert_eq!(session.frames[1]["content"], "one\ntwo\nthree");
    assert_eq!(session.frames[2]["content"]["delta"], "a\nb");
}
