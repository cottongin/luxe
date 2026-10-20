//! What a client joining mid-session has missed, kept current from the event stream.
//!
//! A joiner opens on the same frames a resume would show: the transcript so far, whether a turn is
//! running, then the latest state, usage and workspace, the tool cards of a turn still running, and
//! any prompt still waiting for an answer. Text a reply has already streamed is not replayed; it
//! arrives whole once the message commits.

use super::super::rpc::SharedTranscript;
use luxe_core::event::SessionMessage;
use luxe_core::Event;
use std::mem::discriminant;
use std::sync::Arc;

/// The session as a client joining now should first see it.
pub(super) struct Snapshot {
    session_id: String,
    /// Shared with the joiners being sent it, so a join costs a reference, not a copy
    transcript: Arc<Vec<SessionMessage>>,
    /// Whether a turn has started and not yet ended
    turning: bool,
    /// The newest event of each kind where only the newest matters
    latest: Vec<Event>,
    /// Tool cards of the turn in flight, by call id
    activity: Vec<Event>,
    /// Permission prompts and questions nobody has answered yet
    waiting: Vec<Event>,
}

impl Snapshot {
    /// An empty snapshot of `session_id`, before the engine has said anything.
    pub(super) fn new(session_id: String) -> Self {
        Self {
            session_id,
            transcript: Arc::default(),
            turning: false,
            latest: Vec::new(),
            activity: Vec::new(),
            waiting: Vec::new(),
        }
    }

    /// The session the stream is about now.
    pub(super) fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Whether the prompt `id` names is still waiting for an answer.
    pub(super) fn is_waiting(&self, id: &str) -> bool {
        self.waiting.iter().any(|prompt| prompt_id(prompt) == id)
    }

    /// Fold one event in.
    pub(super) fn observe(&mut self, event: &Event) {
        match event {
            Event::SessionLoaded { session_id, messages } => self.load(session_id, messages),
            Event::MessageCommitted { entry_id, message, .. } => {
                let message =
                    SessionMessage { entry_id: entry_id.clone(), message: message.clone() };
                // Copies the list only if a joiner is still being sent it, which is brief
                Arc::make_mut(&mut self.transcript).push(message);
            }
            Event::StateChanged(_)
            | Event::SessionUsage(_)
            | Event::ContextUsage { .. }
            | Event::PlanUsage(_)
            | Event::WorkspaceUpdated(_) => replace_kind(&mut self.latest, event),
            Event::ToolActivityUpdated(activity) => self.track_activity(&activity.call_id, event),
            Event::PermissionRequested { .. } | Event::QuestionAsked { .. } => {
                self.waiting.push(event.clone());
            }
            Event::PromptClosed { id } => self.waiting.retain(|prompt| prompt_id(prompt) != id),
            Event::TurnStarted => self.turning = true,
            Event::TurnEnded { .. } => self.end_turn(),
            _ => {}
        }
    }

    /// The transcript so far, which a new client is sent first, as its `session_loaded` frame.
    pub(super) fn transcript(&self) -> SharedTranscript {
        SharedTranscript {
            session_id: self.session_id.clone(),
            messages: Arc::clone(&self.transcript),
        }
    }

    /// The frames that bring a new client up to now after the transcript, oldest first.
    pub(super) fn frames(&self) -> Vec<Event> {
        self.turning
            .then_some(Event::TurnStarted)
            .into_iter()
            .chain(self.latest.iter().cloned())
            .chain(self.activity.iter().cloned())
            .chain(self.waiting.iter().cloned())
            .collect()
    }

    /// Start over on a loaded session, which replaces everything a front-end shows.
    fn load(&mut self, session_id: &str, messages: &[SessionMessage]) {
        self.session_id = session_id.to_string();
        self.transcript = Arc::new(messages.to_vec());
        self.end_turn();
    }

    /// Nothing of a finished turn is still running or waiting.
    fn end_turn(&mut self) {
        self.turning = false;
        self.activity.clear();
        self.waiting.clear();
    }

    /// Keep only the newest card for `call_id`.
    fn track_activity(&mut self, call_id: &str, event: &Event) {
        self.activity.retain(|kept| activity_call(kept) != Some(call_id));
        self.activity.push(event.clone());
    }
}

/// Replace the kept event of `event`'s kind, or keep it as the first of its kind.
fn replace_kind(kept: &mut Vec<Event>, event: &Event) {
    let kind = discriminant(event);
    kept.retain(|old| discriminant(old) != kind);
    kept.push(event.clone());
}

/// The id a waiting prompt answers to, in the form `PromptClosed` carries.
fn prompt_id(prompt: &Event) -> &str {
    match prompt {
        Event::PermissionRequested { id, .. } => id.as_str(),
        Event::QuestionAsked { id, .. } => id.as_str(),
        _ => "",
    }
}

/// The tool call a kept card belongs to.
fn activity_call(event: &Event) -> Option<&str> {
    match event {
        Event::ToolActivityUpdated(activity) => Some(&activity.call_id),
        _ => None,
    }
}
