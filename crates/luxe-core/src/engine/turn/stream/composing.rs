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
    /// The subject last sent. A later member can improve it: `bash` is named by its command until
    /// the `description` written after it arrives, and the card has to carry that before it runs,
    /// since dispatch brings no arguments with it. Members only count once whole, so it changes a
    /// handful of times at most, never character by character.
    named: Option<String>,
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
        let due = self.at.is_none_or(|at| now.duration_since(at) >= COMPOSING_FRAME);
        // The first subject is worth interrupting the cadence for: it is what turns a bare spinner
        // into a card. A better one waits its turn with the body, since finding it means parsing
        // the whole argument, and on every fragment of a long `write` that adds up.
        let frame = self.frame(name, arguments, self.named.is_none() || due, due)?;
        self.at = Some(now);
        Some(frame)
    }

    /// Whatever the cadence held back, for a call the model has finished writing.
    ///
    /// Sent before the call is handed over to run, because nothing after that brings its
    /// arguments: a `bash` whose `description` closed inside the last window would otherwise run
    /// under its command until the message commits, after the call is over.
    pub(super) fn last_frame(&mut self, name: &str, arguments: &str) -> Option<ComposingFrame> {
        self.frame(name, arguments, true, true)
    }

    /// The subject and body that changed since they were last sent, looking only where asked.
    fn frame(
        &mut self,
        name: &str,
        arguments: &str,
        rename: bool,
        refill: bool,
    ) -> Option<ComposingFrame> {
        let subject = rename.then(|| self.new_subject(name, arguments)).flatten();
        let body = refill.then(|| self.grown_body(name, arguments)).flatten();
        if subject.is_none() && body.is_none() {
            return None;
        }
        if let Some(subject) = &subject {
            self.named = Some(subject.clone());
        }
        if let Some(body) = &body {
            self.sent = (body.text.len(), body.total_lines);
        }
        Some(ComposingFrame { subject, body })
    }

    fn new_subject(&self, name: &str, arguments: &str) -> Option<String> {
        describe_partial_call(name, arguments, crate::ToolContext::working_dir())
            .filter(|subject| self.named.as_ref() != Some(subject))
    }

    fn grown_body(&self, name: &str, arguments: &str) -> Option<crate::activity::ToolCardBody> {
        partial_card_body(name, arguments, crate::ToolContext::working_dir())
            .filter(|body| (body.text.len(), body.total_lines) > self.sent)
    }
}
