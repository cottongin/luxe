//! Composing — what a card is told about a call the model is still writing, and when.

use super::*;

/// Shortest gap between two frames about the same half-written call.
///
/// A model emits fragments far faster than a terminal repaints, and the payload is resent whole
/// each time rather than as a delta, so an unthrottled body would spend the channel on frames
/// nobody draws. Three of the TUI's own ~33ms frames: fast enough that a file visibly fills in,
/// slow enough that the cost is a rounding error against what a redraw already does.
const COMPOSING_FRAME: std::time::Duration = std::time::Duration::from_millis(100);

/// Whitespace a call's arguments may trail before the model is judged stuck writing it.
///
/// GPT models now and then fall into writing nothing but newlines and spaces after a call's last
/// value and keep it up to the output limit: twenty minutes and over a hundred thousand billed
/// tokens of blank, while the run waits. JSON allows a raw newline, tab or carriage return only
/// *between* tokens, so a run this long holding one is never content, and pretty-printing never
/// needs a fraction of it.
const RUNAWAY_WHITESPACE: usize = 256;

/// Whether `arguments` ends in the blank run [`RUNAWAY_WHITESPACE`] describes.
pub(super) fn runs_away(arguments: &str) -> bool {
    let mut breaks = false;
    for (seen, byte) in arguments.bytes().rev().enumerate() {
        if seen == RUNAWAY_WHITESPACE {
            return breaks;
        }
        match byte {
            b' ' => {}
            b'\n' | b'\r' | b'\t' => breaks = true,
            _ => return false,
        }
    }
    false
}

/// Whether a call the model is opening earns a card before it is dispatched.
///
/// Both halves of the identity have to be real, or the row cannot be the one the dispatch settles;
/// a stream still missing either keeps the old behavior and gets its card at `ToolStarted`.
///
/// **A lookup never composes.** Three adjacent ones fold into a single line, and the fold is formed
/// from the run the *committed* message declares — which does not exist yet while the model is
/// still writing. Composing them would put three cards on screen and then take two of them back the
/// moment the message lands. Waiting for the committed message avoids that late fold. Nothing is
/// lost: a `read`'s arguments are a path, so its composing window is a few
/// milliseconds of a spinner nobody sees. The tools this is *for* are the ones whose arguments are
/// the payload, and none of them fold.
pub(super) fn composes(id: &str, name: &str) -> bool {
    !id.is_empty()
        && !name.is_empty()
        && !crate::activity::is_lookup(name)
        && !crate::activity::is_bookkeeping(name)
}

/// What a front-end has already been told about a call the model is still writing.
#[derive(Default)]
pub(super) struct Composed {
    /// Whether the subject has been sent. It cannot improve, and a header that rewrote itself as
    /// more arrived would read as a glitch rather than as progress.
    named: bool,
    /// Bytes and lines of the payload last sent, so a body that has not grown is not resent. Both,
    /// because the text is capped and stops growing while the line count it reports does not.
    sent: (usize, usize),
    /// When the last frame went out.
    at: Option<std::time::Instant>,
}

/// The parts of a composing frame that have something new to say.
pub(super) struct ComposingFrame {
    pub(super) subject: Option<String>,
    pub(super) body: Option<crate::activity::ToolCardBody>,
}

impl Composed {
    /// What to send about `arguments` at `now`, or `None` when nothing has changed worth a frame.
    ///
    /// The instant is a parameter because the cadence is the whole behavior here, and a test
    /// cannot pace the wall clock.
    pub(super) fn next_frame(
        &mut self,
        now: std::time::Instant,
        name: &str,
        arguments: &str,
    ) -> Option<ComposingFrame> {
        let cwd = crate::ToolContext::working_dir; // asked only when a frame is built
        let subject = if self.named { None } else { describe_partial_call(name, arguments, cwd()) };
        // The subject is worth interrupting the cadence for — it is one small string, sent once,
        // and it is what turns a bare spinner into a card. The body waits its turn.
        let due = self.at.is_none_or(|at| now.duration_since(at) >= COMPOSING_FRAME);
        let body = due
            .then(|| partial_card_body(name, arguments, cwd()))
            .flatten()
            .filter(|body| (body.text.len(), body.total_lines) > self.sent);

        if subject.is_none() && body.is_none() {
            return None;
        }
        self.named |= subject.is_some();
        if let Some(body) = &body {
            self.sent = (body.text.len(), body.total_lines);
        }
        self.at = Some(now);
        Some(ComposingFrame { subject, body })
    }
}
