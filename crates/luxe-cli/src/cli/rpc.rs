//! RPC front-end mode — the engine's `Action`/`Event` protocol over stdio.
//!
//! One JSON object per line each way: wire actions in on stdin, events out on stdout. A client is
//! a *front-end*, with the authority of the person who launched it — it answers permission prompts
//! rather than having them auto-allowed, and the permission engine stays upstream of everything it
//! can ask for. `docs/guide/rpc.md` is the reference.
//!
//! stdout carries the protocol and nothing else. Diagnostics already go to a file rather than the
//! terminal (the TUI owns the screen), so that holds without any special care here — and
//! `rpc_stdout_carries_only_protocol_frames` keeps it holding.

mod frame;
mod translate;

use frame::write_frame;
pub(super) use frame::{Opening, SharedTranscript};
use luxe_core::engine::Action;
use luxe_core::event::Action as WireAction;
use luxe_core::Event;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, BufReader};
use tokio::sync::mpsc;
use translate::{to_engine_action, Rejected};
pub(super) use translate::{CatalogResolver, GithubWrites, ModelResolver, Policy};

/// Depth of the channel carrying locally-generated frames (rejections) to the single writer.
///
/// Small on purpose: it holds refusals, and a client producing enough bad lines to fill it is
/// already being told so.
const LOCAL_FRAME_BUFFER: usize = 32;

/// The catalog a session's clients resolve and list models through, shared by every one of them.
pub(super) type Models = std::sync::Arc<dyn ModelResolver + Send + Sync>;

/// Run the RPC front-end against stdio until the client closes the stream or stdin ends.
///
/// Args: `(settings, github_writes)` — the second is `--github-writes`, which lets the client
/// publish to GitHub unless the session is read-only.
pub(crate) async fn run_rpc(
    settings: crate::engine_setup::AgentSettings,
    github_writes: bool,
) -> anyhow::Result<()> {
    // Before anything is started: a child that shared the protocol's pipe could read frames, and
    // on Windows would stall behind the read waiting on the client
    let input = luxe_core::program::take_stdin()
        .map_err(|error| anyhow::anyhow!("could not take stdin for the protocol: {error}"))?;
    let mode = crate::engine_setup::effective_permission_mode(
        &settings.state_root,
        settings.permission_mode,
        settings.trust_project,
    );
    let policy = Policy {
        github_writes: GithubWrites::for_launch(github_writes, mode),
        github_sections: luxe_core::config::Config::load(&settings.state_root).github.sections(),
        joined: false,
    };
    let handle = crate::engine_setup::build_engine(
        &settings,
        luxe_core::HookRegistry::new(),
        // A client can answer, unlike headless. Auto-allowing on its behalf would make the
        // permission prompt invisible to the one front-end able to show it.
        luxe_core::permission::AskResolution::Prompt,
    )
    .await?;
    // Same bridge the TUI runs on, so an RPC client gets workspace snapshots and tool-activity
    // cards rather than a thinner stream than the built-in front-end sees.
    let events = crate::workspace_runtime::bridge(
        handle.events,
        crate::workspace_runtime::Roots {
            state: settings.state_root.clone(),
            work: settings.work_root.clone(),
        },
        crate::workspace_runtime::pull_request_watch(&settings.work_root),
    );
    // Shared like a terminal's, which is how a phone reaches a session the bridge started
    let (events, _sharing) =
        super::share::share(events, handle.actions.clone(), &settings, &handle.session_id.0);

    let ready = Event::Ready {
        protocol_version: luxe_core::PROTOCOL_VERSION,
        luxe_version: env!("CARGO_PKG_VERSION").to_string(),
        session_id: handle.session_id.0.clone(),
        cwd: settings.work_root.to_string_lossy().into_owned(),
        github_writes: policy.github_writes.allowed(),
    };

    if let Some(level) = settings.initial_thinking {
        handle.actions.send(Action::SetThinkingLevel(level)).await.ok();
    }
    // Walk-away mode from launch, for a client driving a session nobody is watching. The same
    // state `set_permission_auto_approve` reaches, so a client can turn it back off.
    if settings.auto_approve {
        handle.actions.send(Action::SetPermissionAutoApprove(true)).await.ok();
    }

    serve(
        ready.into(),
        events,
        handle.actions,
        tokio::fs::File::from_std(input),
        tokio::io::stdout(),
        std::sync::Arc::new(CatalogResolver { settings: settings.clone() }),
        policy,
    )
    .await?;
    super::tui::shutdown_workspace_services().await;
    Ok(())
}

/// Drive one RPC session over an arbitrary duplex.
///
/// Generic over the streams so the protocol — handshake, framing, translation, refusal, shutdown —
/// is exercised end to end in tests without a real stdin, a real terminal, or a live provider.
/// `run_rpc` is this function with stdio and the real catalog plugged in.
///
/// A client that joined a terminal's session (`policy.joined`) never closes the engine: its
/// hangup or `close` ends its own connection, and the terminal carries on.
pub(super) async fn serve<R, W>(
    opening: Opening,
    mut events: mpsc::Receiver<Event>,
    engine_tx: mpsc::Sender<Action>,
    input: R,
    mut output: W,
    models: Models,
    policy: Policy,
) -> anyhow::Result<()>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin,
{
    let joined = policy.joined;
    if !emit_opening(&mut output, opening).await? {
        close_unless_joined(&engine_tx, joined).await;
        return Ok(());
    }

    let (local_tx, mut local_rx) = mpsc::channel::<Event>(LOCAL_FRAME_BUFFER);
    let reader = tokio::spawn(read_actions(input, engine_tx.clone(), local_tx, models, policy));

    // One writer, so a refusal can never interleave with an event mid-line.
    //
    // The engine's stream ending is what ends the session; the refusal channel merely runs dry
    // when the reader stops, and must not take the session with it — the engine is usually still
    // finishing a turn at that point.
    //
    // Refusals keep their order among themselves, and events keep theirs. The two are *not*
    // ordered against each other: `select!` takes whichever is ready. Nothing needs them to be — a
    // refusal is about the client's own input, not about engine state — and pretending otherwise
    // would mean holding one stream up for the other.
    //
    // A joined client's reader ending *is* the end of its session, since the engine outlives it.
    let mut refusals_open = true;
    loop {
        let frame = tokio::select! {
            event = events.recv() => match event {
                Some(event) => event,
                None => break,
            },
            refusal = local_rx.recv(), if refusals_open => match refusal {
                Some(refusal) => refusal,
                None if joined => break,
                None => {
                    refusals_open = false;
                    continue;
                }
            },
        };
        if !emit(&mut output, &frame).await? {
            break;
        }
    }

    // The engine's stream has ended, but the reader may have queued refusals the client has not
    // been told about. Dropping them would leave someone debugging a frame that vanished without
    // a word, so close the channel to stop new ones and write out what is already there.
    local_rx.close();
    while let Some(refusal) = local_rx.recv().await {
        if !emit(&mut output, &refusal).await? {
            break;
        }
    }

    reader.abort();
    // The engine may already be gone (the client sent `close`); saying so twice is harmless.
    close_unless_joined(&engine_tx, joined).await;
    Ok(())
}

/// Close the engine as a client leaves, unless the client only joined it.
async fn close_unless_joined(engine_tx: &mpsc::Sender<Action>, joined: bool) {
    if !joined {
        engine_tx.send(Action::Close).await.ok();
    }
}

/// Read wire actions until EOF, forwarding what translates and refusing what does not.
///
/// EOF means the client is gone, so the engine is told to close rather than left running behind a
/// stream nobody is reading. A joined client's EOF or `close` only ends the read.
async fn read_actions<R>(
    input: R,
    engine_tx: mpsc::Sender<Action>,
    local_tx: mpsc::Sender<Event>,
    models: Models,
    policy: Policy,
) where
    R: AsyncRead + Unpin,
{
    let mut lines = BufReader::new(input).lines();
    loop {
        let line = match lines.next_line().await {
            Ok(Some(line)) => line,
            Ok(None) => break,
            Err(error) => {
                tracing::warn!(%error, "rpc: input read failed");
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match parse_action(&line, &*models, &policy) {
            Ok(Inbound::Engine(action)) => Ok(*action),
            Ok(Inbound::Answer(answer)) => Err(answer),
            Err(rejected) => {
                Err(Event::ActionRejected { kind: rejected.kind, reason: rejected.reason })
            }
        };
        let action = match reply {
            Ok(action) => action,
            Err(reply) => {
                if local_tx.send(reply).await.is_err() {
                    break;
                }
                continue;
            }
        };
        let closing = matches!(action, Action::Close);
        if closing && policy.joined {
            break;
        }
        if engine_tx.send(action).await.is_err() {
            break;
        }
        if closing {
            break;
        }
    }
    close_unless_joined(&engine_tx, policy.joined).await;
}

/// Parse and translate one inbound line.
///
/// Split out from the read loop so the whole inbound contract — framing, vocabulary, translation —
/// is testable without any IO at all.
fn parse_action(
    line: &str,
    models: &(dyn ModelResolver + Send + Sync),
    policy: &Policy,
) -> Result<Inbound, Rejected> {
    // Read the envelope before the payload, so a frame that names itself and then fails to
    // deserialize is still refused *by name*. Reporting `kind: null` for a `steer_turn` missing its
    // text sends the client looking through every frame it wrote instead of the one that was wrong.
    let raw: serde_json::Value = serde_json::from_str(line)
        .map_err(|error| Rejected { kind: None, reason: error.to_string() })?;
    let kind = raw.get("kind").and_then(serde_json::Value::as_str).map(str::to_string);
    let wire: WireAction = serde_json::from_value(raw)
        .map_err(|error| Rejected { kind, reason: error.to_string() })?;
    if let WireAction::ListModels { request_id } = wire {
        return Ok(Inbound::Answer(Event::ModelsListed { request_id, models: models.choices() }));
    }
    to_engine_action(wire, models, policy).map(|action| Inbound::Engine(Box::new(action)))
}

/// What one inbound line comes to.
#[derive(Debug)]
enum Inbound {
    /// An action for the engine
    Engine(Box<Action>),
    /// An answer for this client alone, which the engine has no part in
    Answer(Event),
}

/// Write one frame, reporting `false` once the client has stopped reading.
///
/// A hangup is how a session ends when the other end is `head` or a closed editor pane, so it
/// resolves to a clean exit rather than an error written to a pipe nobody is holding.
/// Send `ready` and any transcript, saying whether the client is still there.
///
/// Takes the opening by value so the transcript is let go once sent: held for the connection, it
/// would pin a superseded copy of every message, and each commit would copy the snapshot's whole
/// list to append to it.
async fn emit_opening<W: AsyncWrite + Unpin>(
    out: &mut W,
    opening: Opening,
) -> anyhow::Result<bool> {
    if !emit(out, &opening.ready).await? {
        return Ok(false);
    }
    match opening.transcript {
        Some(transcript) => emit(out, &transcript).await,
        None => Ok(true),
    }
}

async fn emit<W, F>(out: &mut W, frame: &F) -> anyhow::Result<bool>
where
    W: AsyncWrite + Unpin,
    F: serde::Serialize + ?Sized,
{
    match write_frame(out, frame).await {
        Ok(()) => Ok(true),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset
            ) =>
        {
            tracing::info!("rpc: client closed the stream");
            Ok(false)
        }
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests;
