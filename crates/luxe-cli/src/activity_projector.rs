//! Tool activity projection — turns legacy lifecycle events into bounded UI state.
//!
//! The runtime keeps legacy lifecycle events intact for existing consumers, then emits this
//! revisioned companion state for workspace front-ends.

use luxe_core::message::{ContentBlock, Message, Role, ToolOutput};
use luxe_core::redact::redact_secrets;
use luxe_core::{Event, ToolActivity, ToolActivityState};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

// Enough to carry a compact edit/write diff (~20 lines) or a chunk of expanded tool output; the
// collapsed one-line card uses `label`, not this preview, so a larger budget only enriches details.
const PREVIEW_LIMIT: usize = 2000;

/// Projects raw tool lifecycle events for the active session into typed activity updates.
#[derive(Default)]
pub(crate) struct ToolActivityProjector {
    session_id: Option<String>,
    calls: HashMap<String, ToolActivity>,
    /// The call the model is writing right now, if any.
    ///
    /// One, because a model writes the calls of a message in order — which is what lets a card
    /// stop looking busy the moment the model moves on to the next one.
    writing: Option<String>,
    pending_calls: HashMap<String, ToolCallDetails>,
    pending_results: HashMap<String, String>,
}

struct ToolCallDetails {
    label: String,
    input_preview: String,
    /// The multi-line payload the card renders, for the tools that have one.
    card_body: Option<luxe_core::ToolCardBody>,
    /// Where this call sits in the run its message asked for, for a run longer than one.
    batch: Option<luxe_core::ToolBatch>,
    /// Where this call sits in a folded run of lookups, for a call that is in one.
    run: Option<luxe_core::ToolRun>,
}

impl ToolActivityProjector {
    /// Updates the active session and returns the activity updates an event produced.
    ///
    /// More than one, because a call the model *stops* writing changes at the same moment another
    /// starts: two cards move on one event, and dropping either leaves a transcript claiming two
    /// things are happening at once.
    pub(crate) fn project(&mut self, event: &Event) -> Vec<ToolActivity> {
        if let Event::SessionLoaded { session_id, .. } = event {
            self.session_id = Some(session_id.clone());
            self.calls.clear();
            self.writing = None;
            self.pending_calls.clear();
            self.pending_results.clear();
            return Vec::new();
        }

        match event {
            Event::MessageCommitted { session_id, message, .. }
                if self.session_id.as_deref() == Some(session_id) =>
            {
                let mut updates = self.record_committed_message(message);
                // Everything it asked for is written in full now, so nothing is still being
                // composed — what they are all waiting on is their turn to run.
                updates.extend(self.stop_writing());
                updates
            }
            Event::ToolCallComposing { call_id, name, subject, body } => {
                self.composing(call_id, name, subject.as_deref(), body.as_ref())
            }
            Event::ToolStarted { call_id, name } => {
                self.started(call_id, name).into_iter().chain(self.fit_writing()).collect()
            }
            Event::ToolProgress { call_id, chunk } => {
                self.progressed(call_id, chunk).into_iter().collect()
            }
            Event::ToolCompleted { call_id, is_error, output } => {
                let settled = self.settle(call_id, *is_error, Some(bounded_display(output)));
                settled.into_iter().chain(self.fit_writing()).collect()
            }
            Event::ToolEnded { call_id, is_error } => {
                self.ended(call_id, *is_error).into_iter().chain(self.fit_writing()).collect()
            }
            _ => Vec::new(),
        }
    }

    /// Settles whichever call the model was writing into the queue it is now waiting in.
    fn stop_writing(&mut self) -> Option<ToolActivity> {
        let call_id = self.writing.take()?;
        let open = self.calls.get_mut(&call_id)?;
        if open.state != ToolActivityState::Composing {
            return None;
        }
        open.revision += 1;
        open.state = ToolActivityState::Queued;
        Some(open.clone())
    }

    /// How the card of a call still being written should read: waiting while another call runs,
    /// since that one is the busy card and this one can't run before it, and working otherwise.
    fn writing_state(&self, call_id: &str) -> ToolActivityState {
        let running = self
            .calls
            .iter()
            .any(|(id, open)| id != call_id && open.state == ToolActivityState::Running);
        if running {
            ToolActivityState::Queued
        } else {
            ToolActivityState::Composing
        }
    }

    /// Moves the card of the call being written between waiting and working as a call ahead of
    /// it starts or stops.
    fn fit_writing(&mut self) -> Option<ToolActivity> {
        let call_id = self.writing.clone()?;
        let state = self.writing_state(&call_id);
        let open = self.calls.get_mut(&call_id)?;
        let unsent = matches!(open.state, ToolActivityState::Composing | ToolActivityState::Queued);
        if !unsent || open.state == state {
            return None;
        }
        open.revision += 1;
        open.state = state;
        Some(open.clone())
    }

    /// Opens (or fills in) the card for the call the model is writing right now.
    ///
    /// `Composing` is a *working* state — the model is producing this call as the reader watches,
    /// and the card says so the same way a running one does. What it is not is `Running`: the tool
    /// has not been dispatched, and it cannot be until whatever is running now has finished.
    ///
    /// The model writes a message's calls in order, so the one this opens is the only one being
    /// written; whichever came before it has been written in full and is merely waiting its turn.
    /// Saying that is what keeps exactly one card looking busy — two spinning `bash` cards read as
    /// two shells at once, and a paused mark on all of them put a stopped sign over a card that
    /// was visibly filling in. An earlier call can run while this one is written, though, and then
    /// that one is the busy card: this one opens `Queued`, since it can't run before it either,
    /// and [`Self::fit_writing`] puts it back to work if the model is still writing it.
    ///
    /// What it does not yet have is a batch position — the message has not committed, so how many
    /// calls it asked for is not known — or a body, whose payload is the argument still arriving.
    fn composing(
        &mut self,
        call_id: &str,
        name: &str,
        subject: Option<&str>,
        body: Option<&luxe_core::ToolCardBody>,
    ) -> Vec<ToolActivity> {
        let Some(session_id) = self.session_id.clone() else {
            return Vec::new();
        };
        let mut updates = Vec::new();
        if self.writing.as_deref() != Some(call_id) {
            updates.extend(self.stop_writing());
            self.writing = Some(call_id.to_owned());
        }

        let label = subject.map(bounded_display).unwrap_or_default();
        let state = self.writing_state(call_id);
        match self.calls.get_mut(call_id) {
            // The call growing under the card that is already drawing it: the subject arriving, or
            // another few lines of the payload. Said twice, neither is worth a revision. Queued
            // here is a card held behind a running call, since only the call being written composes.
            Some(open)
                if matches!(
                    open.state,
                    ToolActivityState::Composing | ToolActivityState::Queued
                ) =>
            {
                let renamed = !label.is_empty() && open.label != label;
                let grew = body.is_some_and(|body| open.card_body.as_ref() != Some(body));
                if renamed || grew {
                    open.revision += 1;
                    if renamed {
                        open.label = label;
                    }
                    if grew {
                        open.card_body = body.cloned();
                    }
                    updates.push(open.clone());
                }
            }
            // Dispatched, or over. Either way its arguments are known in full and a guess made
            // from a fragment cannot improve on them.
            Some(_) => {}
            None => {
                let mut activity =
                    ToolActivity::composing(session_id, call_id, name, label, now_ms());
                activity.card_body = body.cloned();
                activity.state = state;
                self.calls.insert(call_id.to_owned(), activity.clone());
                updates.push(activity);
            }
        }
        updates
    }

    fn started(&mut self, call_id: &str, name: &str) -> Option<ToolActivity> {
        let session_id = self.session_id.clone()?;
        if self.writing.as_deref() == Some(call_id) {
            self.writing = None;
        }
        // A card still being written, or written and waiting its turn, is exactly what dispatch
        // adopts. One that is already running or already over is a second `ToolStarted` for the
        // same call, which is not a thing that happens and not a thing to act on.
        let adoptable = |state: ToolActivityState| {
            matches!(state, ToolActivityState::Composing | ToolActivityState::Queued)
        };
        if self.calls.get(call_id).is_some_and(|open| !adoptable(open.state)) {
            return None;
        }

        let details = self.pending_calls.remove(call_id);
        // Dispatch is what the card was waiting for: it is running now, its arguments are
        // known in full so a label guessed from a fragment gives way to the real one, and the
        // clock restarts — a `write` the model spent a minute composing still ran in two
        // milliseconds, and the card must say so.
        let open = self.calls.remove(call_id);
        let label = match (&details, open.as_ref()) {
            (Some(details), _) => details.label.clone(),
            // Nothing committed to describe the call by, so whatever the composing card managed to
            // read out of the fragment beats repeating the tool's own name back at the reader.
            (None, Some(open)) if !open.label.is_empty() => open.label.clone(),
            (None, _) => bounded_display(name),
        };
        let mut activity = match open {
            Some(open) => ToolActivity {
                revision: open.revision + 1,
                state: ToolActivityState::Running,
                label,
                started_at_ms: now_ms(),
                ..open
            },
            None => ToolActivity::running(session_id, call_id, name, label, now_ms()),
        };
        if let Some(details) = details {
            activity.input_preview = Some(details.input_preview);
            activity.card_body = details.card_body;
            activity.batch = details.batch;
            activity.run = details.run;
        }
        self.calls.insert(call_id.to_owned(), activity.clone());
        Some(activity)
    }

    /// Remembers what a committed message says about its calls, and fills in the cards of the
    /// ones that already started.
    ///
    /// A call can run while the model is still writing the next one, so by the time its message
    /// commits its card may be running or over, labeled from the fragment that was there when it
    /// started. The committed arguments are the whole call, so those cards take its real label and
    /// body now; the rest wait for `ToolStarted` to adopt them, as before.
    fn record_committed_message(&mut self, message: &Message) -> Vec<ToolActivity> {
        let mut updates = Vec::new();
        match message.role {
            Role::Assistant => {
                // The whole run is known here, before any of it has been dispatched, which is what
                // lets a card say "3 of 7" while the seventh is still hypothetical.
                let calls = message
                    .content
                    .iter()
                    .filter(|block| matches!(block, ContentBlock::ToolCall { .. }))
                    .count();
                let total = u32::try_from(calls).unwrap_or(u32::MAX);
                // Which of them a front-end may fold together is decided here too, so the run's
                // first card already knows it is one of seven and can draw as the run from the
                // start — a fold that forms later has to take back rows it already spent.
                let runs = luxe_core::lookup_runs(&message.content);
                // where `bash` starts, so a card can leave off a `cd` into it
                let root = luxe_core::ToolContext::working_dir();
                let mut index = 0u32;
                for block in &message.content {
                    let ContentBlock::ToolCall { id, name, arguments } = block else {
                        continue;
                    };
                    index += 1;
                    // The card's label is a human description of the call, not its argument JSON.
                    let label = bounded_display(&luxe_core::describe_call(name, arguments, &root));
                    let input_preview = bounded_display(&arguments.to_string());
                    // Redacted inside `tool_card_body`; a written file is not bounded by the
                    // model's preview budget because it never reaches the model.
                    let card_body = luxe_core::tool_card_body(name, arguments, &root);
                    let started = self.calls.get_mut(id).filter(|open| {
                        !matches!(
                            open.state,
                            ToolActivityState::Composing | ToolActivityState::Queued
                        )
                    });
                    if let Some(open) = started {
                        open.revision += 1;
                        open.label = label;
                        open.input_preview = Some(input_preview);
                        open.card_body = card_body;
                        open.batch = batch_position(index, total);
                        open.run = runs.get(id).copied();
                        updates.push(open.clone());
                        continue;
                    }
                    self.pending_calls.insert(
                        id.clone(),
                        ToolCallDetails {
                            label,
                            input_preview,
                            card_body,
                            batch: batch_position(index, total),
                            run: runs.get(id).copied(),
                        },
                    );
                }
            }
            Role::Tool => {
                for block in &message.content {
                    let ContentBlock::ToolResult { call_id, content, .. } = block else {
                        continue;
                    };
                    self.pending_results
                        .insert(call_id.clone(), bounded_display(&tool_output_text(content)));
                }
            }
            Role::System | Role::User => {}
        }
        updates
    }

    fn progressed(&mut self, call_id: &str, chunk: &str) -> Option<ToolActivity> {
        let activity = self.calls.get_mut(call_id)?;
        if activity.is_terminal() {
            return None;
        }
        activity.revision += 1;
        activity.output_preview = Some(bounded_display(chunk));
        Some(activity.clone())
    }

    /// Settles a call once its message's results commit, unless `ToolCompleted` already did.
    fn ended(&mut self, call_id: &str, is_error: bool) -> Option<ToolActivity> {
        let output = self.pending_results.remove(call_id);
        self.settle(call_id, is_error, output)
    }

    /// Marks a call succeeded or failed with what it returned; a card already over stays as it is.
    fn settle(
        &mut self,
        call_id: &str,
        is_error: bool,
        output: Option<String>,
    ) -> Option<ToolActivity> {
        // A refused or cancelled call is answered without ever being dispatched, so its card may
        // still be the one the model's writing opened.
        if self.writing.as_deref() == Some(call_id) {
            self.writing = None;
        }
        let activity = self.calls.get_mut(call_id)?;
        if activity.is_terminal() {
            return None;
        }
        activity.revision += 1;
        activity.state =
            if is_error { ToolActivityState::Failed } else { ToolActivityState::Succeeded };
        activity.elapsed_ms = Some(now_ms().saturating_sub(activity.started_at_ms));
        // A failed card that only says "Failed" makes the reader open the details to learn what a
        // one-line message would have told them, so the tool's own words become the summary.
        activity.summary = Some(match (is_error, output.as_deref()) {
            (true, Some(output)) => luxe_core::activity::failure_reason(output),
            (true, None) => "Failed".to_string(),
            (false, _) => "Completed".to_string(),
        });
        activity.output_preview = output;
        Some(activity.clone())
    }
}

/// A call's place in its message's run, or `None` when the message asked for only the one.
///
/// A lone call has no run to be part of, and a front-end that drew `1/1` would be reporting the
/// absence of a queue as though it were one.
pub(crate) fn batch_position(index: u32, total: u32) -> Option<luxe_core::ToolBatch> {
    (total > 1).then_some(luxe_core::ToolBatch { index, total })
}

fn tool_output_text(content: &[ToolOutput]) -> String {
    content
        .iter()
        .map(|output| match output {
            ToolOutput::Text(text) => text.as_str(),
            ToolOutput::Image { .. } => "[image output]",
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |duration| duration.as_millis() as u64)
}

fn bounded_display(value: &str) -> String {
    let redacted = redact_secrets(value, &[]);
    let mut preview: String = redacted.chars().take(PREVIEW_LIMIT).collect();
    if redacted.chars().count() > PREVIEW_LIMIT {
        preview.push('…');
    }
    preview
}

#[cfg(test)]
mod tests;
