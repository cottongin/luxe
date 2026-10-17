//! Empty blocks — nothing leaves for a provider as an empty content block.
//!
//! Anthropic rejects a text block that is empty or whitespace-only with
//! `400 messages: text content blocks must be non-empty`, and the offending block is *history*, so
//! it replays on every later request: one of them ends the session and every resume of it. This is
//! the same failure mode [`crate::tool_pairing`] exists for, arriving through a different door.
//!
//! Three doors, in fact, and two of them are ordinary:
//!
//! - An assistant reply the stream dropped before it produced anything. The turn commits nothing
//!   for one, but sessions written by older builds still carry an empty text block.
//! - A message whose text is empty because the *attachment* is the message — an image pasted with
//!   no caption.
//! - A tool whose result has no output at all, which reaches the wire as an empty string.
//!
//! [`scrub`] fixes all three on the **model-facing copy**, at request assembly, next to
//! [`crate::tool_pairing::repair`]. The durable session log is never rewritten: it is the record of
//! what happened, and what happened is that the model said nothing.
//!
//! An empty tool result is filled rather than dropped, because dropping it would strand the call it
//! answers — trading this 400 for the other one.

use crate::message::{ContentBlock, Message, ToolOutput};

/// What a result says when the tool that ran produced no output at all.
///
/// One owner for the words, because two places write them: dispatch, for a tool that returns
/// nothing now, and [`scrub`], for one that did so in an older recorded session.
pub(crate) const NO_OUTPUT: &str = "(no output)";

/// What one [`scrub`] pass had to change.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Scrub {
    /// Blocks dropped for carrying nothing a provider will accept.
    pub(crate) blocks_dropped: usize,
    /// Messages dropped because the scrub left them with no content.
    pub(crate) messages_dropped: usize,
    /// Tool results whose missing output was given a stand-in.
    pub(crate) results_filled: usize,
}

impl Scrub {
    pub(crate) fn is_clean(&self) -> bool {
        self.blocks_dropped == 0 && self.messages_dropped == 0 && self.results_filled == 0
    }
}

/// Remove every empty content block from `messages`, in place.
///
/// A sendable transcript is left byte-identical, and the pass is idempotent — both matter more than
/// they look. The outgoing copy is an append to the last request's, so a transform that rewrote the
/// prefix, or rewrote it differently from one turn to the next, would re-bill the whole
/// conversation at the cache *write* rate.
pub(crate) fn scrub(messages: &mut Vec<Message>) -> Scrub {
    let mut scrub = Scrub::default();
    for message in messages.iter_mut() {
        for block in &mut message.content {
            let ContentBlock::ToolResult { content, .. } = block else { continue };
            let before = content.len();
            content.retain(|output| match output {
                ToolOutput::Text(text) => !text.trim().is_empty(),
                ToolOutput::Image { .. } => true,
            });
            scrub.blocks_dropped += before - content.len();
            if content.is_empty() {
                content.push(ToolOutput::Text(NO_OUTPUT.to_string()));
                scrub.results_filled += 1;
            }
        }
        let before = message.content.len();
        message.content.retain(|block| match block {
            ContentBlock::Text(text) => !text.trim().is_empty(),
            _ => true,
        });
        scrub.blocks_dropped += before - message.content.len();
    }
    let before = messages.len();
    // An emptied message is its own provider error, and nothing here can strand a tool call by
    // going: a message holding one is never empty.
    messages.retain(|message| !message.content.is_empty());
    scrub.messages_dropped = before - messages.len();
    scrub
}

#[cfg(test)]
mod tests;
