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

mod translate;

use luxe_core::engine::Action;
use luxe_core::event::Action as WireAction;
use luxe_core::Event;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use translate::{to_engine_action, CatalogResolver, GithubWrites, ModelResolver, Policy, Rejected};

/// Depth of the channel carrying locally-generated frames (rejections) to the single writer.
///
/// Small on purpose: it holds refusals, and a client producing enough bad lines to fill it is
/// already being told so.
const LOCAL_FRAME_BUFFER: usize = 32;

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
        ready,
        events,
        handle.actions,
        tokio::fs::File::from_std(input),
        tokio::io::stdout(),
        &CatalogResolver,
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
async fn serve<R, W>(
    ready: Event,
    mut events: mpsc::Receiver<Event>,
    engine_tx: mpsc::Sender<Action>,
    input: R,
    mut output: W,
    models: &'static (dyn ModelResolver + Sync),
    policy: Policy,
) -> anyhow::Result<()>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin,
{
    if !emit(&mut output, &ready).await? {
        engine_tx.send(Action::Close).await.ok();
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
    let mut refusals_open = true;
    loop {
        let frame = tokio::select! {
            event = events.recv() => match event {
                Some(event) => event,
                None => break,
            },
            refusal = local_rx.recv(), if refusals_open => match refusal {
                Some(refusal) => refusal,
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
    engine_tx.send(Action::Close).await.ok();
    Ok(())
}

/// Read wire actions until EOF, forwarding what translates and refusing what does not.
///
/// EOF means the client is gone, so the engine is told to close rather than left running behind a
/// stream nobody is reading.
async fn read_actions<R>(
    input: R,
    engine_tx: mpsc::Sender<Action>,
    local_tx: mpsc::Sender<Event>,
    models: &'static (dyn ModelResolver + Sync),
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
        let action = match parse_action(&line, models, &policy) {
            Ok(action) => action,
            Err(rejected) => {
                let refusal =
                    Event::ActionRejected { kind: rejected.kind, reason: rejected.reason };
                if local_tx.send(refusal).await.is_err() {
                    break;
                }
                continue;
            }
        };
        let closing = matches!(action, Action::Close);
        if engine_tx.send(action).await.is_err() {
            break;
        }
        if closing {
            break;
        }
    }
    engine_tx.send(Action::Close).await.ok();
}

/// Parse and translate one inbound line.
///
/// Split out from the read loop so the whole inbound contract — framing, vocabulary, translation —
/// is testable without any IO at all.
fn parse_action(
    line: &str,
    models: &(dyn ModelResolver + Sync),
    policy: &Policy,
) -> Result<Action, Rejected> {
    // Read the envelope before the payload, so a frame that names itself and then fails to
    // deserialize is still refused *by name*. Reporting `kind: null` for a `steer_turn` missing its
    // text sends the client looking through every frame it wrote instead of the one that was wrong.
    let raw: serde_json::Value = serde_json::from_str(line)
        .map_err(|error| Rejected { kind: None, reason: error.to_string() })?;
    let kind = raw.get("kind").and_then(serde_json::Value::as_str).map(str::to_string);
    let wire: WireAction = serde_json::from_value(raw)
        .map_err(|error| Rejected { kind, reason: error.to_string() })?;
    to_engine_action(wire, models, policy)
}

/// Write one frame, reporting `false` once the client has stopped reading.
///
/// A hangup is how a session ends when the other end is `head` or a closed editor pane, so it
/// resolves to a clean exit rather than an error written to a pipe nobody is holding.
async fn emit<W: AsyncWrite + Unpin>(out: &mut W, event: &Event) -> anyhow::Result<bool> {
    match write_frame(out, event).await {
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

/// Write one frame and flush it, so a streaming client sees tokens as they arrive.
///
/// A frame that cannot serialize is logged and dropped rather than ending the session: losing one
/// event is recoverable for a client, and a dead stream is not.
async fn write_frame<W: AsyncWrite + Unpin>(out: &mut W, event: &Event) -> std::io::Result<()> {
    let line = match serde_json::to_string(event) {
        Ok(line) => line,
        Err(error) => {
            tracing::warn!(%error, "rpc: dropping an event that would not serialize");
            return Ok(());
        }
    };
    out.write_all(line.as_bytes()).await?;
    out.write_all(b"\n").await?;
    out.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests;
