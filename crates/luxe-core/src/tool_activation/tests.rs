use super::*;
use crate::session::EntryId;

fn activate(id: &str, names: &[&str]) -> Message {
    Message {
        role: Role::Assistant,
        content: vec![ContentBlock::ToolCall {
            id: id.to_string(),
            name: ACTIVATE_TOOLS_NAME.to_string(),
            arguments: serde_json::json!({ "names": names }),
        }],
    }
}

fn entry(id: &str, kind: EntryKind) -> Entry {
    Entry { id: EntryId(id.to_string()), parent_id: None, ts: 0, kind }
}

fn message(id: &str, message: Message) -> Entry {
    entry(id, EntryKind::Message(message))
}

#[test]
fn requested_names_keep_order_and_drop_repeats_and_non_strings() {
    let args = serde_json::json!({ "names": ["debug", 3, "ast_grep", "debug", null] });
    assert_eq!(requested_names(&args), vec!["debug", "ast_grep"]);
    assert!(requested_names(&serde_json::json!({ "names": "debug" })).is_empty());
    assert!(requested_names(&serde_json::json!({})).is_empty());
}

/// Only a call that succeeded loaded anything. An error, or a call cut off before its result
/// was recorded, leaves the tool where it was.
#[test]
fn only_a_successful_activation_counts() {
    let messages = vec![
        activate("a", &["debug"]),
        Message::tool_result("a", "Activated: debug.", false),
        activate("b", &["nope"]),
        Message::tool_result("b", "Error: not activatable", true),
        activate("c", &["screenshot"]),
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolCall {
                id: "d".into(),
                name: "read".into(),
                arguments: serde_json::json!({ "names": ["web_fetch"] }),
            }],
        },
        Message::tool_result("d", "ok", false),
    ];
    assert_eq!(activated_tools(&messages), BTreeSet::from(["debug".to_string()]));
}

fn compacted_path() -> Vec<Entry> {
    vec![
        message("u1", Message::user("start")),
        message("a1", activate("call-1", &["module_report", "debug"])),
        message("t1", Message::tool_result("call-1", "Activated: debug, module_report.", false)),
        message("u2", Message::user("kept from here")),
        message("a2", activate("call-2", &["ast_grep"])),
        message("t2", Message::tool_result("call-2", "Activated: ast_grep.", false)),
        entry(
            "c1",
            EntryKind::Compaction {
                summary: "what happened".into(),
                first_kept: EntryId("u2".into()),
                tokens_before: 0,
            },
        ),
    ]
}

/// The tools the summary replaced the activation of come back right after it, and the ones
/// still activated in the kept tail are left to the tail.
#[test]
fn a_compaction_restores_what_it_replaced() {
    let entries = compacted_path();
    let mut messages = crate::compaction::context_messages(&entries);
    let before = messages.len();
    restore_after_compaction(&entries, &mut messages);

    assert_eq!(messages.len(), before + 2);
    assert!(messages[0].text().contains("what happened"), "the summary still leads");
    let ContentBlock::ToolCall { id, name, arguments } = &messages[1].content[0] else {
        panic!("expected the synthetic call, got {:?}", messages[1]);
    };
    assert_eq!((id.as_str(), name.as_str()), ("restore-c1", ACTIVATE_TOOLS_NAME));
    assert_eq!(arguments, &serde_json::json!({ "names": ["debug", "module_report"] }));
    assert_eq!(messages[2].role, Role::Tool);
    assert_eq!(messages[3].text(), "kept from here", "and the kept tail follows");
    assert_eq!(
        activated_tools(&messages),
        BTreeSet::from(["ast_grep".into(), "debug".into(), "module_report".into()]),
        "so the outgoing copy loads everything the whole path did"
    );
}

/// A newer cut that lands before an older summary leaves it on the kept side, where it reads
/// first; the restore belongs after the newest summary, not wedged between the two.
#[test]
fn the_restore_follows_the_newest_summary_when_an_older_one_leads() {
    let compaction = |id: &str, first_kept: &str, summary: &str| {
        entry(
            id,
            EntryKind::Compaction {
                summary: summary.into(),
                first_kept: EntryId(first_kept.into()),
                tokens_before: 0,
            },
        )
    };
    let mut entries = compacted_path();
    entries[6] = compaction("c1", "u2", "OLDER");
    entries.push(message("u3", Message::user("after")));
    entries.push(compaction("c2", "a2", "NEWER"));
    let mut messages = crate::compaction::context_messages(&entries);
    restore_after_compaction(&entries, &mut messages);

    assert!(messages[0].text().contains("OLDER"), "{:?}", messages[0]);
    assert!(messages[1].text().contains("NEWER"), "{:?}", messages[1]);
    let ContentBlock::ToolCall { id, .. } = &messages[2].content[0] else {
        panic!("expected the synthetic call after both summaries, got {:?}", messages[2]);
    };
    assert_eq!(id, "restore-c2");
    assert_eq!(messages[3].role, Role::Tool);
    assert!(
        matches!(&messages[4].content[0], ContentBlock::ToolCall { id, .. } if id == "call-2"),
        "and the kept tail follows: {:?}",
        messages[4]
    );
}

/// A cut whose first kept entry is not on the path replaces everything before it, so everything
/// those entries loaded comes back behind the newest summary.
#[test]
fn a_cut_that_names_nothing_on_the_path_still_restores() {
    let mut entries = compacted_path();
    entries.push(message("u3", Message::user("after")));
    entries.push(entry(
        "c2",
        EntryKind::Compaction {
            summary: "newest".into(),
            first_kept: EntryId("gone".into()),
            tokens_before: 0,
        },
    ));
    let mut messages = crate::compaction::context_messages(&entries);
    restore_after_compaction(&entries, &mut messages);

    assert!(messages[0].text().contains("newest"), "{:?}", messages[0]);
    let ContentBlock::ToolCall { id, arguments, .. } = &messages[1].content[0] else {
        panic!("expected the synthetic call behind the summary, got {:?}", messages[1]);
    };
    assert_eq!(id, "restore-c2");
    assert_eq!(
        arguments,
        &serde_json::json!({ "names": ["ast_grep", "debug", "module_report"] }),
        "everything before the cut was replaced"
    );
}

/// The exchange is a function of what the compaction froze, so later turns don't move it:
/// that's what keeps the cache's prefix through it.
#[test]
fn the_restore_holds_still_as_the_session_grows() {
    let mut entries = compacted_path();
    let mut first = crate::compaction::context_messages(&entries);
    restore_after_compaction(&entries, &mut first);

    entries.push(message("a3", activate("call-3", &["web_fetch"])));
    entries.push(message("t3", Message::tool_result("call-3", "Activated: web_fetch.", false)));
    let mut later = crate::compaction::context_messages(&entries);
    restore_after_compaction(&entries, &mut later);

    assert_eq!(later[..first.len()], first[..], "an append, not an edit");
}

#[test]
fn nothing_to_restore_leaves_the_transcript_alone() {
    let plain = vec![message("u1", Message::user("hi")), message("a1", Message::assistant("yo"))];
    let mut messages = crate::compaction::context_messages(&plain);
    restore_after_compaction(&plain, &mut messages);
    assert_eq!(messages.len(), 2, "no compaction");

    let quiet = vec![
        message("u1", Message::user("hi")),
        message("u2", Message::user("kept")),
        entry(
            "c1",
            EntryKind::Compaction {
                summary: "s".into(),
                first_kept: EntryId("u2".into()),
                tokens_before: 0,
            },
        ),
    ];
    let mut messages = crate::compaction::context_messages(&quiet);
    let before = messages.clone();
    restore_after_compaction(&quiet, &mut messages);
    assert_eq!(messages, before, "a compaction that replaced no activation");
}
