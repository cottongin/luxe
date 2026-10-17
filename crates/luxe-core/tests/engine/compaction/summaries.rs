//! Summaries — compaction replaces the history it summarized in the next request.

use luxe_core::session::{JsonlSessionStore, SessionStore};
use uuid::Uuid;

/// Plan, summarize, append — over a real parent chain, ending in the request the next turn sends.
///
/// The fixture used to append every entry with `parent: None`, a flat forest with no path through
/// it, and assert only that a `Compaction` entry existed afterwards. That is why compaction could
/// summarize, bill for the call, write the summary to disk and still send the entire transcript
/// for months without a red test.
#[tokio::test]
async fn compaction_replaces_the_history_it_summarized() {
    use luxe_core::compaction::CompactionSettings;
    use luxe_core::session::EntryKind;

    let tmp_dir = std::env::temp_dir().join(format!("luxe-test-compaction-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&tmp_dir).unwrap();
    let mut store = JsonlSessionStore::new(tmp_dir.clone());
    let session_id = store.create("/tmp/test").unwrap();

    // A real chain. For a 128K window / 32K max the usable budget is 128K - min(32K, 16_384) =
    // 111_616 tokens and compaction fires at 90% of that (~100K); at ~1 token / 4 chars, 100
    // entries of ~5000 chars clears it.
    let long_text = "a".repeat(5000);
    let mut parent = luxe_core::session::latest_leaf(&store.load(&session_id).unwrap());
    for index in 0..100 {
        let kind = match index % 2 {
            0 => EntryKind::Message(luxe_core::message::Message::user(format!(
                "Message {index} {long_text}"
            ))),
            _ => EntryKind::Message(luxe_core::message::Message::assistant(format!(
                "Response {index} {long_text}"
            ))),
        };
        parent = Some(store.append(&session_id, parent, kind).unwrap());
    }
    let leaf = parent.expect("a leaf");

    let settings = CompactionSettings {
        enabled: true,
        reserve_tokens: 16_384,
        keep_recent_tokens: 5_000,
        compact_at_fraction: 0.9,
        prune_protect_tokens: 40_000,
        prune_min_tokens: 20_000,
        prune_at_fraction: 0.8,
        summarizer: None,
    };
    let budget = luxe_core::compaction::ContextBudget {
        context_window: 128_000,
        max_tokens: 32_000,
        observed_tokens: None,
        ..Default::default()
    };

    let entries = store.active_path(&session_id, &leaf).unwrap();
    let plan = luxe_core::compaction::find_compaction_point(
        &entries,
        &settings,
        budget,
        luxe_core::compaction::CompactionTrigger::Threshold,
    )
    .unwrap();
    assert!(!plan.messages_to_summarize.is_empty());
    assert!(!luxe_core::compaction::serialize_for_summary(&plan.messages_to_summarize).is_empty());

    let formatted = luxe_core::compaction::format_structured_summary(
        "## Goal\nTest compaction.\n\n## Next Steps\nContinue testing.",
        &plan.file_ops,
    );
    assert!(formatted.contains("Test compaction"));

    let compacted = store
        .compact(&session_id, Some(leaf), &formatted, &plan.first_kept, plan.tokens_before)
        .unwrap();

    // The compaction is the tip, and the whole session is still reachable through it — nothing is
    // deleted, so a later `/tree` or an audit can still read what was summarized.
    let path = store.active_path(&session_id, &compacted).unwrap();
    assert_eq!(path.len(), 102, "header + 100 messages + compaction");

    // But the request built from that path carries the summary in place of what it replaced.
    let messages = luxe_core::compaction::context_messages(&path);
    let rendered: Vec<String> = messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            luxe_core::message::ContentBlock::Text(text) => Some(text.clone()),
            _ => None,
        })
        .collect();

    assert!(messages.len() < 20, "the summarized span is gone: {} messages", messages.len());
    assert!(rendered[0].contains("Test compaction"), "the summary leads: {}", &rendered[0][..80]);
    assert!(rendered[0].contains("<summary>"), "and says what it is");
    assert!(
        !rendered[1..].iter().any(|text| text.starts_with("Message 0 ")),
        "the oldest message was summarized away"
    );
    assert!(
        rendered.iter().any(|text| text.starts_with("Response 99 ")),
        "the newest message survives verbatim"
    );

    // And the occupancy the next threshold check reads is the smaller one it produced.
    let before = luxe_core::compaction::estimate_tokens(&path);
    let after = luxe_core::compaction::context_tokens(&path);
    assert!(after * 4 < before, "context shrank from {before} to {after} tokens");

    std::fs::remove_dir_all(&tmp_dir).unwrap();
}
