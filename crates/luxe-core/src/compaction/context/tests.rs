use super::*;
use crate::compaction::*;
use crate::message::{ContentBlock, Message, Role};
use crate::session::{Entry, EntryKind};

/// A summary stands in front of the conversation it replaced, and only the newest one counts —
/// an older compaction is inside the span this one already covers.
#[test]
fn the_context_is_the_summary_plus_what_it_kept_plus_what_came_after() {
    let text = |body: &str| Entry {
        id: EntryId::new(),
        parent_id: None,
        ts: 0,
        kind: EntryKind::Message(Message::user(body.to_string())),
    };
    let compaction = |summary: &str, first_kept: &EntryId| Entry {
        id: EntryId::new(),
        parent_id: None,
        ts: 0,
        kind: EntryKind::Compaction {
            summary: summary.to_string(),
            first_kept: first_kept.clone(),
            tokens_before: 1_000,
        },
    };
    let said = |messages: &[Message]| -> Vec<String> {
        messages
            .iter()
            .flat_map(|message| &message.content)
            .filter_map(|block| match block {
                ContentBlock::Text(text) => Some(text.clone()),
                _ => None,
            })
            .collect()
    };

    let path: Vec<Entry> = vec![text("a"), text("b"), text("c"), text("d")];
    // No compaction: the path is the context, unchanged.
    assert_eq!(said(&context_messages(&path)), ["a", "b", "c", "d"]);

    // Compacted at `c`, then two more turns.
    let mut compacted = path.clone();
    compacted.push(compaction("FIRST", &path[2].id));
    compacted.push(text("e"));
    let messages = context_messages(&compacted);
    assert_eq!(said(&messages)[1..], ["c", "d", "e"], "a and b were summarized away");
    assert!(said(&messages)[0].contains("FIRST"));
    assert!(said(&messages)[0].contains("<summary>"), "framed as a report, not a turn");
    assert_eq!(messages[0].role, Role::User);

    // A second compaction supersedes the first — the first is inside the span it replaced.
    let mut twice = compacted.clone();
    twice.push(compaction("SECOND", &twice[5].id));
    let messages = context_messages(&twice);
    assert_eq!(said(&messages).len(), 2);
    assert!(said(&messages)[0].contains("SECOND"));
    assert!(!said(&messages)[0].contains("FIRST"), "only the newest cut applies");
    assert_eq!(said(&messages)[1], "e");

    // A `first_kept` that isn't on the path (a truncated or hand-edited log) degrades to the
    // summary plus everything after it, rather than dropping the conversation.
    let mut orphaned = path.clone();
    orphaned.push(compaction("ORPHAN", &EntryId::new()));
    orphaned.push(text("f"));
    assert_eq!(said(&context_messages(&orphaned))[1..], ["f"]);
}

/// A summary the newer cut left on the kept side is read before the newer one, and before anything
/// kept: it stands for older history than either.
///
/// The break this catches: it stayed where the log had appended it, so a request read the newer
/// summary, then the kept messages, then the older summary — history out of order.
#[test]
fn an_older_summary_on_the_kept_side_leads() {
    let text = |body: &str| Entry {
        id: EntryId::new(),
        parent_id: None,
        ts: 0,
        kind: EntryKind::Message(Message::user(body.to_string())),
    };
    let compaction = |summary: &str, first_kept: &EntryId| Entry {
        id: EntryId::new(),
        parent_id: None,
        ts: 0,
        kind: EntryKind::Compaction {
            summary: summary.to_string(),
            first_kept: first_kept.clone(),
            tokens_before: 1_000,
        },
    };
    let mut log: Vec<Entry> = ["a", "b", "c", "d"].into_iter().map(text).collect();
    log.push(compaction("OLDER", &log[2].id.clone()));
    log.push(text("e"));
    log.push(compaction("NEWER", &log[3].id.clone()));
    log.push(text("f"));

    let said: Vec<String> = context_messages(&log)
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert!(said[0].contains("OLDER") && !said[0].contains("NEWER"), "{said:?}");
    assert!(said[1].contains("NEWER") && !said[1].contains("OLDER"), "{said:?}");
    assert_eq!(said[2..], ["d", "e", "f"]);
}
