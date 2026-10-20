//! Context — the entries and messages a request is built from, after the newest cut.

use crate::message::Message;
use crate::session::Entry;
use std::borrow::Borrow;

/// How a compaction summary introduces itself to the model.
///
/// It has to read as a *report about* a conversation, not as a turn in one — an unframed summary
/// gets answered rather than absorbed. The delimiters matter for the same reason: the model needs
/// to see where someone else's account of the history stops and the live conversation starts.
const SUMMARY_PREFIX: &str =
    "The conversation history before this point was compacted into the following summary:\n<summary>\n";
const SUMMARY_SUFFIX: &str = "\n</summary>";

/// The entries a request is actually built from.
///
/// A session log is append-only, so compacting does not delete anything: the summarized messages
/// stay on the parent chain and the [`EntryKind::Compaction`](crate::session::EntryKind::Compaction)
/// entry is appended after them, naming the first entry it kept. This is the cut — the newest
/// compaction's summary stands in front, then the entries it kept, then everything committed
/// since. Without it a compacted session sends the whole transcript *plus* a summary of it, which
/// is how compaction comes to cost tokens instead of saving them.
///
/// Only the newest compaction on the path replaces anything. An older one is either inside the span
/// this one replaced or, when the newer cut landed before it, on the kept side, still standing for
/// what it summarized. That one reads first: it covers history older than the newest summary and
/// everything kept after it. A `first_kept` that isn't on the path (a hand-edited or partially
/// written log) degrades to summary-plus-everything-after rather than dropping the conversation.
pub fn context_entries<E: Borrow<Entry>>(entries: &[E]) -> Vec<&Entry> {
    let entries: Vec<&Entry> = entries.iter().map(Borrow::borrow).collect();
    let Some(cut) = entries
        .iter()
        .rposition(|entry| matches!(entry.kind, crate::session::EntryKind::Compaction { .. }))
    else {
        return entries;
    };
    let crate::session::EntryKind::Compaction { first_kept, .. } = &entries[cut].kind else {
        return entries;
    };

    let kept_from = entries[..cut].iter().position(|entry| entry.id == *first_kept);
    let mut context = Vec::with_capacity(entries.len() - cut + 1);
    let kept = kept_from.map_or(&[][..], |at| &entries[at..cut]);
    // Summaries first, oldest first. A compaction on the kept side was appended after the span it
    // summarized, so left where the log has it, it would read after messages that came later than
    // what it stands for.
    let is_summary =
        |entry: &&Entry| matches!(entry.kind, crate::session::EntryKind::Compaction { .. });
    context.extend(kept.iter().copied().filter(is_summary));
    context.push(entries[cut]);
    context.extend(kept.iter().copied().filter(|entry| !is_summary(entry)));
    context.extend(entries[cut + 1..].iter().copied());
    context
}

/// The model-facing message list for a transcript: [`context_entries`], with the compaction
/// summary rendered as the user message that replaces what it summarized.
///
/// The one place a transcript becomes a request, so the cut cannot be applied in some callers and
/// forgotten in others.
pub fn context_messages<E: Borrow<Entry>>(entries: &[E]) -> Vec<Message> {
    context_entries(entries)
        .into_iter()
        .filter_map(|entry| match &entry.kind {
            crate::session::EntryKind::Message(message) => Some(message.clone()),
            crate::session::EntryKind::Compaction { summary, .. } => {
                Some(Message::user(format!("{SUMMARY_PREFIX}{summary}{SUMMARY_SUFFIX}")))
            }
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests;
