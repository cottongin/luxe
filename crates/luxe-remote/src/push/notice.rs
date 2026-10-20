//! Notice — what a notification says, read from one session's events.
//!
//! A notification says what is waiting and where, never the detail: "Approve bash" with the
//! folder's name, not the command, and opening it leads to the prompt. Answering takes the app.
//!
//! Two rules keep it from buzzing at someone sitting at their desk. A prompt is held for a few
//! seconds first, and one answered meanwhile is never sent. A turn's end is only news when the
//! turn ran long enough for someone to have walked away.

use luxe_core::{Event, StopReason};
use std::time::Duration;
use tokio::time::Instant;

/// How long a prompt waits before it's sent, for an answer at the desk to beat it.
pub(crate) const PROMPT_GRACE: Duration = Duration::from_secs(10);

/// The shortest turn whose end is sent: a quick reply is read where it was asked for.
pub(crate) const LONG_TURN: Duration = Duration::from_secs(30);

/// One notification, as the service worker shows it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct Notice {
    /// The project folder's name
    pub(crate) title: String,
    /// What's waiting, in a few words
    pub(crate) body: String,
    /// The session it opens
    pub(crate) session: String,
    /// Whether to deliver now rather than when the device saves power
    #[serde(skip)]
    pub(crate) urgent: bool,
}

/// What an event means for notifications.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Heard {
    /// Send this now
    Now(Notice),
    /// Send this unless prompt `id` closes within [`PROMPT_GRACE`]
    Prompt(String, Notice),
    /// Prompt `id` was answered, wherever
    Closed(String),
}

/// One session's stream, read for what's worth a notification.
pub(crate) struct Reading {
    session: String,
    folder: String,
    turn_began: Option<Instant>,
}

impl Reading {
    /// A reading of the session `session` running in `cwd`.
    pub(crate) fn new(session: &str, cwd: &str) -> Self {
        let folder = std::path::Path::new(cwd).file_name().map(|name| name.to_string_lossy());
        let folder = folder.map_or_else(|| cwd.to_string(), |name| name.into_owned());
        Self { session: session.to_string(), folder, turn_began: None }
    }

    /// What `event`, seen at `now`, means.
    pub(crate) fn read(&mut self, event: &Event, now: Instant) -> Option<Heard> {
        if is_turn_sign(event) {
            self.turn_began.get_or_insert(now);
        }
        match event {
            Event::Ready { session_id, .. } | Event::SessionLoaded { session_id, .. } => {
                self.session.clone_from(session_id);
                None
            }
            Event::PermissionRequested { .. } | Event::QuestionAsked { .. } => self.prompt(event),
            Event::PromptClosed { id } => Some(Heard::Closed(id.clone())),
            Event::TurnEnded { reason } => self.turn_ended(reason, now).map(Heard::Now),
            _ => None,
        }
    }

    fn prompt(&self, event: &Event) -> Option<Heard> {
        let (id, body) = match event {
            Event::PermissionRequested { id, tool, .. } => (id.as_str(), format!("Approve {tool}")),
            Event::QuestionAsked { id, .. } => (id.as_str(), "Luxe has a question".to_string()),
            _ => return None,
        };
        Some(Heard::Prompt(id.to_string(), self.notice(body, true)))
    }

    fn turn_ended(&mut self, reason: &StopReason, now: Instant) -> Option<Notice> {
        let began = self.turn_began.take()?;
        if now.duration_since(began) < LONG_TURN {
            return None;
        }
        let body = match reason {
            StopReason::EndTurn | StopReason::ToolCall => "Finished, and waiting for you",
            StopReason::Other(_) => "Stopped",
        };
        Some(self.notice(body.into(), false))
    }

    fn notice(&self, body: String, urgent: bool) -> Notice {
        Notice { title: self.folder.clone(), body, session: self.session.clone(), urgent }
    }
}

/// Whether `event` shows a turn running. The engine says so with `TurnStarted`, which comes
/// first; the message that opened the turn, or anything it streams, stands in for it from a
/// session whose Luxe predates that event.
fn is_turn_sign(event: &Event) -> bool {
    match event {
        Event::TurnStarted
        | Event::TokenDelta { .. }
        | Event::ThinkingDelta { .. }
        | Event::ToolCallComposing { .. } => true,
        Event::MessageCommitted { message, .. } => message.role == luxe_core::message::Role::User,
        _ => false,
    }
}

#[cfg(test)]
mod tests;
