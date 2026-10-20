//! The running turn — streamed deltas, tool status, how a turn ends, and telling the reader.

use crate::app::{App, AppState, ConversationItem, ConvoBlock};
use luxe_core::config::{NotificationEvent, ThinkingDisplay};
use luxe_core::event::EngineState;
use luxe_core::ToolActivityState;

pub(super) fn thinking_delta(app: &mut App, block: usize, delta: String) {
    let display = app.config.conversation.thinking();
    if !matches!(display, ThinkingDisplay::Hidden) {
        // Keep appending into the reasoning block this delta belongs to, and open a new one
        // when the provider moves to the next item. Streaming a message's several reasoning
        // items as one block would only have to be undone at commit, which the reader sees
        // as the whole thing reflowing under them.
        let open = app.streaming_thinking_block == Some(block)
            && app
                .convo
                .last()
                .is_some_and(|item| matches!(item.block(), ConvoBlock::Thinking { .. }));
        match app.convo.last_mut().filter(|_| open).map(ConversationItem::block_mut) {
            Some(ConvoBlock::Thinking { text, .. }) => {
                text.push_str(&delta);
            }
            _ => {
                app.push_convo(ConvoBlock::Thinking {
                    text: delta,
                    expanded: matches!(display, ThinkingDisplay::Shown),
                });
                app.reveal.begin(app.convo.len() - 1);
                app.streaming_thinking_block = Some(block);
            }
        }
        app.streamed_thinking = true;
    }
    if app.status.turn_started.is_none() {
        app.status.on_turn_start();
    }
    app.edit_resend_mode = false;
    app.status.state = AppState::Streaming;
}

pub(super) fn token_delta(app: &mut App, delta: String) {
    // Append into the open answer block. Opening a new one when the tail isn't one keeps a
    // reasoning block (streamed just before the answer) from swallowing the first token.
    let append = app.streaming
        && app.convo.last().is_some_and(|item| matches!(item.block(), ConvoBlock::Assistant(_)));
    if append {
        if let Some(ConvoBlock::Assistant(existing)) =
            app.convo.last_mut().map(ConversationItem::block_mut)
        {
            existing.push_str(&delta);
        }
    } else {
        app.push_convo(ConvoBlock::Assistant(delta));
        app.reveal.begin(app.convo.len() - 1);
        app.streaming = true;
        if app.status.turn_started.is_none() {
            app.status.on_turn_start();
        }
        app.edit_resend_mode = false; // Clear on new turn.
    }
    app.status.state = AppState::Streaming;
}

/// A turn began, maybe one a joined client asked for. One sent from here has been running since
/// Enter, so its clock is left alone.
pub(super) fn turn_started(app: &mut App) {
    app.status.state = AppState::Streaming;
    if app.status.turn_started.is_none() {
        app.status.on_turn_start();
    }
}

pub(super) fn turn_ended(app: &mut App) {
    end_turn(app, "The turn ended before this call ran");
    // Steering the turn ended before taking in stays held. That window is now narrow — the
    // engine keeps a turn going for a steer that lands while its final response streams —
    // but a steer that arrives in the moment between that check and the turn ending is
    // buffered for the next turn's first checkpoint, so it really is still on its way and
    // the tray should keep saying so.
    //
    // Follow-ups run strictly one at a time, in submission order, only now that the turn
    // they were queued behind has finished.
    // A review asked for mid-turn was parked rather than captured, because there is no
    // honest revision to pin while the change is still being written. There is one now.
    crate::app::commands::flush_queued_review(app);
    let promoted = app.promote_next_queued();
    let plan = app.plan_written.take();
    // Notify only when the work is really done — a turn that hands straight over to a
    // queued follow-up has not finished anything yet.
    if !promoted {
        if let Some(path) = plan.filter(|_| app.status.mode != luxe_core::mode::BUILD) {
            crate::app::plan_handoff::offer(app, path);
        }
        alert(app, NotificationEvent::Turn, "Turn finished");
    }
}

pub(super) fn failed(app: &mut App, msg: String) {
    // A failure is not something to animate around: show whatever arrived, then the error.
    app.reveal_catch_up();
    end_turn(app, "The turn failed before this call ran");
    // A failed turn ends without a `TurnEnded`, so mid-turn submissions would otherwise
    // keep a pending marker forever. The follow-ups come back to the composer rather than
    // being auto-run — feeding a failing provider more turns just compounds the failure, and
    // a message handed back can be re-sent with one key. Steering stays held: the engine
    // still has it buffered for the next turn, so the tray is telling the truth.
    app.return_queued_messages();
    // `plan_written` is kept on purpose: the plan is on disk, and the next turn to finish
    // is when a handoff can be offered without interrupting anything
    app.push_convo(ConvoBlock::System(format!("[error] {msg}")));
    // A failure is exactly when attention has wandered elsewhere, so it notifies too.
    alert(app, NotificationEvent::Turn, &format!("Turn failed: {msg}"));
}

/// Puts the app back at rest when a turn is over, however it ended.
///
/// A prompt can't outlive its turn (the engine already resolved or dropped it), and a card still
/// spinning is settled as cancelled with `reason`.
fn end_turn(app: &mut App, reason: &str) {
    // A load that failed sends no snapshot, and one that never comes must not mute the next real one
    app.arrival.owes_snapshot = false;
    // A prompt a hook refused never commits, and must not claim the next client's message
    app.own_commit_pending = false;
    app.interrupt_sent_at = None;
    app.streaming = false;
    app.streamed_thinking = false;
    app.streaming_thinking_block = None;
    app.status.state = AppState::Idle;
    app.status.turn_started = None;
    app.status.active_tool = None;
    app.status.tool_started = None;
    mark_unfinished_tools(app, ToolActivityState::Cancelled, reason);
    app.overlays.permission = None;
    app.overlays.question = None;
}

pub(super) fn tool_started(app: &mut App, name: String) {
    app.status.state = AppState::Streaming;
    if app.status.turn_started.is_none() {
        app.status.on_turn_start();
    }
    app.status.active_tool = Some(name);
    app.status.tool_started = Some(app.frame_now());
}

pub(super) fn state_changed(app: &mut App, state: EngineState) {
    // The load's own snapshot was taken before the turn its `SessionLoaded` sent, so its idle is
    // about a moment that has passed; believed, it hides the running turn until the first token
    let stale = std::mem::take(&mut app.arrival.owes_snapshot) && app.turn_in_flight();
    if !stale {
        follow_reported_activity(app, state.streaming);
    }
    if !state.model.model.is_empty() {
        app.status.model = state.model.display().to_string();
        app.status.provider = state.model.provider.clone();
        follow_reported_model(app, &state.model);
    }
    app.status.thinking = state.thinking;
    // Not marked in the transcript: the composer's title, the bar and the dock say which
    // mode is on, and a line per switch would fill the scrollback with every toggle
    app.status.mode = state.mode.clone();
}

/// Takes the engine's word on whether a turn is running. A turn that is still running leaves a
/// compaction inside it on show: the summary is the turn's work until its own end event lands.
fn follow_reported_activity(app: &mut App, streaming: bool) {
    if streaming && matches!(app.status.state, AppState::Compacting) {
        return;
    }
    app.status.state = if streaming { AppState::Streaming } else { AppState::Idle };
    if !streaming {
        app.status.active_tool = None;
        app.status.tool_started = None;
    }
}

/// Marks the model the engine reports as the active one, which the engine can move to without the
/// reader picking it: a mode with a model of its own, or an engine that took over from another.
/// One the catalog does not list is left as it was, since the pickers have nothing to mark.
fn follow_reported_model(app: &mut App, reported: &luxe_core::event::ModelRef) {
    let reported_is = |info: &luxe_core::provider::ModelInfo| {
        info.r#ref.provider == reported.provider && info.r#ref.id == reported.model
    };
    if app.active_model_info.as_ref().is_some_and(reported_is) {
        return;
    }
    if let Some(entry) = app.model_catalog.iter().find(|entry| reported_is(&entry.info)) {
        app.active_model_info = Some(entry.info.clone());
    }
}

/// Rings the bell and sends a desktop notification for `event`, each where the reader asked for it.
///
/// The bell answers every moment, since asking for one is asking to be told when luxe needs you;
/// the notification answers the moments `notifications` names. Headless has nobody to tell.
pub(super) fn alert(app: &App, event: NotificationEvent, body: &str) {
    if app.headless {
        return;
    }
    crate::notify::bell(app.config.bell());
    if app.config.notifies(event) {
        notify_reader(app, body);
    }
}

/// `Approve bash: rm -rf build`, from a permission prompt's tool and summary.
pub(super) fn permission_body(tool: &str, summary: &str) -> String {
    match summary.trim() {
        "" => format!("Approve {tool}?"),
        summary => format!("Approve {tool}: {summary}"),
    }
}

/// Tells the reader `body` under the project's name, unless they are already looking.
///
/// A terminal that has reported focus is being watched, and a banner over the window someone is
/// reading is noise; one that has never said either way is treated as in the background, since
/// missing a notification costs more than an extra one.
fn notify_reader(app: &App, body: &str) {
    if app.terminal_focused == Some(true) {
        return;
    }
    let project = app.project_root.as_deref().and_then(std::path::Path::file_name);
    let title = match project {
        Some(name) => format!("luxe · {}", name.to_string_lossy()),
        None => "luxe".to_string(),
    };
    app.notifier.notify(&title, body);
}

/// How long an interrupt may go unconfirmed before the transcript stops waiting for the engine.
///
/// The engine drops a running call the moment it sees the interrupt and ends the turn right after,
/// which takes milliseconds. Seconds of silence mean the confirmation is not coming.
pub(in crate::app) const INTERRUPT_CONFIRM: std::time::Duration = std::time::Duration::from_secs(5);

/// Whether any card is still composing, queued or running.
pub(in crate::app) fn has_unfinished_tools(app: &App) -> bool {
    app.tool_activity.values().any(|activity| !activity.is_terminal())
}

/// Stops waiting on an interrupt the engine never confirmed.
///
/// Without this a card stayed on its last spinner frame with its clock running for as long as the
/// session lasted, beside a statusline that already said the turn was over. The cards settle as
/// interrupted and the transcript says what happened, because a turn that ignored an interrupt is
/// worth knowing about and the log is where its cause will be.
pub(in crate::app) fn expire_unconfirmed_interrupt(app: &mut App) {
    let Some(sent) = app.interrupt_sent_at else { return };
    if app.frame_now().saturating_duration_since(sent) < INTERRUPT_CONFIRM {
        return;
    }
    app.interrupt_sent_at = None;
    let stranded = app.tool_activity.values().filter(|activity| !activity.is_terminal()).count();
    tracing::warn!(
        stranded,
        waited_ms = INTERRUPT_CONFIRM.as_millis(),
        "the engine did not confirm an interrupt"
    );
    mark_unfinished_tools(app, ToolActivityState::Cancelled, "Interrupted");
    app.push_convo(ConvoBlock::System(
        "The agent did not confirm it stopped. Its unfinished calls are marked interrupted; \
         anything it still sends will appear below. Details are in .luxe/logs/luxe.log."
            .to_string(),
    ));
}

/// Closes out any tool card still composing, queued or running, as `state` with `summary`.
///
/// Every dispatched call is answered, so at the end of a live turn there is ordinarily nothing
/// here. The cards this exists for are the ones opened while the model was *writing* a call
/// ([`luxe_core::event::Event::ToolCallComposing`]): a stream that fails, or a cancel that lands
/// between the call arriving and dispatch, ends the turn without that call ever reaching the
/// engine — and a spinner nothing will ever come back for reads as a hang. A replayed session
/// whose last turn never finished leaves the same cards.
pub(super) fn mark_unfinished_tools(app: &mut App, state: ToolActivityState, summary: &str) {
    let stranded = app
        .tool_activity
        .values()
        .filter(|activity| !activity.is_terminal())
        .cloned()
        .map(|mut activity| {
            activity.revision += 1;
            activity.state = state;
            activity.summary = Some(summary.to_string());
            activity
        })
        .collect::<Vec<_>>();
    for activity in stranded {
        app.apply_tool_activity(activity);
    }
}

/// The transcript line saying a requested review began.
pub(super) fn review_started(model: &str) -> String {
    format!(
        "A reviewer on {model} is reading the uncommitted changes in a fresh context. Its \
         findings will show here."
    )
}

/// What a requested review found, as one line of the transcript and the list under it.
pub(super) fn review_summary(
    verdict: luxe_core::event::RequestedReviewVerdict,
    findings: &[String],
) -> String {
    use luxe_core::event::RequestedReviewVerdict as Verdict;
    match verdict {
        Verdict::Continue => "The reviewer found nothing to fix.".to_string(),
        Verdict::Fix => {
            let list: String = findings.iter().map(|finding| format!("\n- {finding}")).collect();
            format!("The reviewer would fix these, and the agent has them now:{list}")
        }
        Verdict::Unavailable => format!(
            "No review came back: {}",
            findings.first().map(String::as_str).unwrap_or("the reviewer did not answer")
        ),
    }
}
