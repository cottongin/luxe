use super::*;
use crate::compaction::*;
use crate::message::{ContentBlock, Message, Role, ToolOutput};
use crate::session::{Entry, EntryKind};

#[test]
fn find_compaction_point_tracks_the_real_window() {
    // ~160K estimated tokens across four entries (160K chars / 4 each).
    let chunk = "x".repeat(160_000);
    let entries: Vec<Entry> = (0..4)
        .map(|_| Entry {
            id: EntryId::new(),
            parent_id: None,
            ts: 0,
            kind: EntryKind::Message(Message::user(chunk.clone())),
        })
        .collect();
    let settings = CompactionSettings { keep_recent_tokens: 100, ..Default::default() };
    let budget = |window, max| ContextBudget {
        context_window: window,
        max_tokens: max,
        observed_tokens: None,
        ..Default::default()
    };

    // 200K window / 32K max: usable 180K, trigger 162K. 160K < 162K -> no compaction.
    assert!(matches!(
        find_compaction_point(
            &entries,
            &settings,
            budget(200_000, 32_000),
            CompactionTrigger::Threshold
        ),
        Err(CompactionError::Cancelled)
    ));
    // 128K window / 32K max: usable 108K, trigger ~97K. 160K over -> compaction planned.
    assert!(find_compaction_point(
        &entries,
        &settings,
        budget(128_000, 32_000),
        CompactionTrigger::Threshold
    )
    .is_ok());
    // Unknown window (0): never compact blind.
    assert!(matches!(
        find_compaction_point(&entries, &settings, budget(0, 32_000), CompactionTrigger::Threshold),
        Err(CompactionError::Cancelled)
    ));

    // The two budgets stay decoupled. When a conversation gets summarized is
    // `usable_context_window`'s call alone; what one request may ask for is
    // `clamp_max_tokens_to_context`'s. The 200K/32K case above is the pin — fold the request's
    // safety margin into the reserve and its threshold falls from 162K to ~158K, which starts
    // compacting a 160K transcript that had room to spare.
    assert_eq!(usable_context_window(200_000, 32_000, settings.reserve_tokens), 180_000);
}

/// The prefix occupies the window too, and a cap that forgot it left compaction nothing to free
/// on any model whose schemas are a real share of its context: keep half the window while the
/// system prompt and tools hold another quarter, and a quarter is all there is to cut from.
#[test]
fn the_kept_tail_leaves_room_for_the_prompt_that_sits_above_it() {
    // Six messages the estimate reads as 10k each.
    let entries = user_entries(6, 40_000);
    let settings = CompactionSettings { keep_recent_tokens: 20_000, ..Default::default() };
    // Usable 39k, so the tail may hold 19.5k — of which the prefix has already taken 10k.
    let budget = ContextBudget {
        context_window: 45_000,
        max_tokens: 6_000,
        observed_tokens: Some(42_000),
        prefix_tokens: 10_000,
    };

    let plan = find_compaction_point(&entries, &settings, budget, CompactionTrigger::Threshold)
        .expect("a full window yields a cut");
    // 9.5k of tail is one message; forget the prefix and it is two, and the window stays full.
    assert_eq!(plan.messages_to_summarize.len(), 5);

    // The prefix is an estimate like everything else, so it is scaled by the same calibration
    // before being subtracted — a 6k estimate on a transcript measured at twice its guess is
    // 12k of real window gone, and taking it at face value gives the tail back room that is
    // not there.
    let plan = find_compaction_point(
        &user_entries(8, 10_000),
        &settings,
        ContextBudget {
            context_window: 60_000,
            max_tokens: 8_000,
            observed_tokens: Some(52_000),
            prefix_tokens: 6_000,
        },
        CompactionTrigger::Threshold,
    )
    .expect("a full window yields a cut");
    assert_eq!(plan.messages_to_summarize.len(), 5);
}

/// The other half of the same arithmetic, isolated: with the cap far from binding, the walk
/// still has to stop where `keep_recent_tokens` of *real* tokens is, not char/4 of them.
#[test]
fn a_calibrated_walk_stops_where_the_real_tokens_run_out() {
    // Eight messages the estimate reads as 2.5k each, measured at 1.7x that.
    let entries = user_entries(8, 10_000);
    let settings = CompactionSettings { keep_recent_tokens: 8_000, ..Default::default() };
    let budget = ContextBudget {
        context_window: 50_000,
        max_tokens: 6_000,
        observed_tokens: Some(42_500),
        prefix_tokens: 5_000,
    };

    let plan = find_compaction_point(&entries, &settings, budget, CompactionTrigger::Threshold)
        .expect("a full window yields a cut");
    // 8k real is 4.7k estimated, which is two messages. Uncalibrated it reads as four, and the
    // tail kept is half again as large as the setting asked for.
    assert_eq!(plan.messages_to_summarize.len(), 6);
}

/// `keep_recent_tokens` is a fixed count and the window is not, so on a small model the default
/// 20K tail is larger than everything the model can hold. Capping it against the usable window
/// is what leaves a history worth summarizing; uncapped, the walk never accumulates a cut and
/// compaction silently never happens — on exactly the sessions that need it most.
#[test]
fn the_recent_tail_can_never_swallow_the_window_it_has_to_fit_in() {
    // Six ~2K-token entries, and a 16K window whose usable budget is 12K.
    let entries: Vec<Entry> = (0..6)
        .map(|_| Entry {
            id: EntryId::new(),
            parent_id: None,
            ts: 0,
            kind: EntryKind::Message(Message::user("x".repeat(8_000))),
        })
        .collect();
    let settings = CompactionSettings { keep_recent_tokens: 20_000, ..Default::default() };
    let budget = ContextBudget { context_window: 16_000, max_tokens: 4_000, ..Default::default() };

    let plan = find_compaction_point(&entries, &settings, budget, CompactionTrigger::Threshold)
        .expect("a full window has to yield a cut");
    assert!(
        !plan.messages_to_summarize.is_empty(),
        "a tail bigger than the window leaves nothing to summarize"
    );
    assert!(
        plan.messages_to_summarize.len() < entries.len(),
        "and it still has to keep a recent tail"
    );
}

#[test]
fn observed_tokens_override_the_estimate() {
    // Three ~40K-token entries => ~120K estimated, below the 162K trigger for a 200K window.
    let chunk = "x".repeat(160_000);
    let entries: Vec<Entry> = (0..3)
        .map(|_| Entry {
            id: EntryId::new(),
            parent_id: None,
            ts: 0,
            kind: EntryKind::Message(Message::user(chunk.clone())),
        })
        .collect();
    let settings = CompactionSettings { keep_recent_tokens: 100, ..Default::default() };
    let base = ContextBudget { context_window: 200_000, max_tokens: 32_000, ..Default::default() };

    // Estimate alone (~120K < 162K) doesn't fire.
    assert!(matches!(
        find_compaction_point(&entries, &settings, base, CompactionTrigger::Threshold),
        Err(CompactionError::Cancelled)
    ));
    // The provider's real usage (170K > 162K) flips the decision and still yields a real cut.
    let observed = ContextBudget { observed_tokens: Some(170_000), ..base };
    let plan =
        find_compaction_point(&entries, &settings, observed, CompactionTrigger::Threshold).unwrap();
    assert!(!plan.messages_to_summarize.is_empty());
}

/// The cut names the first entry the request keeps, so it may not name a tool result: the call
/// it answers would be on the summarized side, and an unpaired half is a hard 400 on both wire
/// formats rather than a degraded reply.
#[test]
fn a_cut_never_strands_a_tool_result_from_its_call() {
    let entry = |kind: EntryKind| Entry { id: EntryId::new(), parent_id: None, ts: 0, kind };
    let assistant_call = |body: &str| {
        entry(EntryKind::Message(Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Text(body.to_string()),
                ContentBlock::ToolCall {
                    id: "call-1".into(),
                    name: "read".into(),
                    arguments: serde_json::json!({}),
                },
            ],
        }))
    };
    let result = |body: &str| {
        entry(EntryKind::Message(Message {
            role: Role::Tool,
            content: vec![ContentBlock::ToolResult {
                call_id: "call-1".into(),
                content: vec![ToolOutput::Text(body.to_string())],
                is_error: false,
            }],
        }))
    };
    let user = |body: &str| entry(EntryKind::Message(Message::user(body.to_string())));

    // Sized so the token walk lands squarely on the tool result.
    let big = "x".repeat(40_000);
    let entries = vec![user(&big), assistant_call(&big), result(&big), user("the next thing")];
    let settings = CompactionSettings { keep_recent_tokens: 9_000, ..Default::default() };
    let plan = find_compaction_point(
        &entries,
        &settings,
        ContextBudget {
            context_window: 40_000,
            max_tokens: 4_000,
            observed_tokens: Some(39_000),
            ..Default::default()
        },
        CompactionTrigger::Threshold,
    )
    .expect("a plan");

    let kept = entries.iter().position(|entry| entry.id == plan.first_kept).expect("on path");
    assert!(
        !matches!(&entries[kept].kind,
            EntryKind::Message(message) if message.role == Role::Tool),
        "the cut moved past the tool result rather than orphaning its call"
    );
    assert_eq!(kept, 3, "forward to the next entry that can legally start a request");
}

/// The three edges of the trigger decision, each of which decides whether a provider call is
/// spent: exactly at the threshold, a measurement of zero, and a summary whose `first_kept` is
/// no longer on the path.
#[test]
fn the_trigger_edges_are_pinned() {
    let chunk = "x".repeat(160_000); // 40K tokens
    let entry = || Entry {
        id: EntryId::new(),
        parent_id: None,
        ts: 0,
        kind: EntryKind::Message(Message::user(chunk.clone())),
    };
    let entries: Vec<Entry> = (0..5).map(|_| entry()).collect();
    let settings = CompactionSettings { keep_recent_tokens: 100, ..Default::default() };
    // usable = 200_000 - min(32_000, 20_000) = 180_000; the trigger is 90% of that.
    let budget = |observed| ContextBudget {
        context_window: 200_000,
        max_tokens: 32_000,
        observed_tokens: observed,
        ..Default::default()
    };
    let plan = |observed| {
        find_compaction_point(&entries, &settings, budget(observed), CompactionTrigger::Threshold)
    };

    // Landing exactly on the threshold compacts — it is "at or over", not "over".
    assert!(plan(Some(162_000)).is_ok(), "162_000 is the threshold, and it counts");
    assert!(plan(Some(161_999)).is_err(), "one token under it does not");

    // A measurement of zero is not an empty context, it is no measurement — fall back to the
    // estimate (5 x 40K here), which is well over.
    assert!(plan(Some(0)).is_ok(), "a zero reading defers to the estimate, not to itself");

    // Manual compaction ignores the threshold entirely.
    assert!(find_compaction_point(&entries, &settings, budget(Some(1)), CompactionTrigger::Manual)
        .is_ok());

    // A small window cannot keep a fixed 20K tail and still have anything to summarize. Left
    // unclamped the walk never finds a cut, and compaction silently never happens — on the
    // sessions most in need of it.
    let cramped = CompactionSettings { keep_recent_tokens: 20_000, ..settings.clone() };
    let plan = find_compaction_point(
        &entries,
        &cramped,
        ContextBudget {
            context_window: 24_000,
            max_tokens: 4_000,
            observed_tokens: Some(23_000),
            ..Default::default()
        },
        CompactionTrigger::Threshold,
    );
    assert!(plan.is_ok(), "a 20K tail against a 20K usable window still compacts");

    // A previous compaction whose `first_kept` is no longer on the path (a truncated log)
    // resumes from just after that compaction rather than from the top of the session.
    let mut orphaned = entries.clone();
    orphaned.insert(
        2,
        Entry {
            id: EntryId::new(),
            parent_id: None,
            ts: 0,
            kind: EntryKind::Compaction {
                summary: "OLD".into(),
                first_kept: EntryId::new(),
                tokens_before: 1,
            },
        },
    );
    let (start, summary) = summarized_span_start(&orphaned);
    assert_eq!(start, 3, "the entry after the compaction, not the one before it");
    assert_eq!(summary.as_deref(), Some("OLD"));
}

/// A second compaction folds in only what the first one kept, wherever its own cut lands.
///
/// After the cut: the prior summary is inside the span, so the summarizer reads it and the new
/// summary replaces both. Before the cut: the prior summary stays on the kept side, still standing
/// for what it replaced, and the new one covers only the entries in between. Either way nothing
/// from before the first cut is read again, and nothing is lost.
#[test]
fn a_second_compaction_folds_in_only_what_the_first_one_kept() {
    let text = |body: &str| Entry {
        id: EntryId::new(),
        parent_id: None,
        ts: 0,
        kind: EntryKind::Message(Message::user(format!("{body}{}", " ".repeat(3_999)))),
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
        let text = |block: &ContentBlock| match block {
            ContentBlock::Text(text) => Some(text.trim().to_string()),
            _ => None,
        };
        messages.iter().flat_map(|message| &message.content).filter_map(text).collect()
    };
    // a..d at 1,000 tokens each, compacted at `c`, then two more turns
    let mut log: Vec<Entry> = ["a", "b", "c", "d"].into_iter().map(text).collect();
    log.push(compaction("FIRST", &log[2].id));
    log.extend(["e", "f"].into_iter().map(text));
    let plan = |keep_recent_tokens| {
        let settings = CompactionSettings { keep_recent_tokens, ..Default::default() };
        find_compaction_point(&log, &settings, ContextBudget::default(), CompactionTrigger::Manual)
            .expect("a plan")
    };
    let compacted_again = |plan: &CompactionPlan| {
        let mut twice = log.clone();
        twice.push(compaction("SECOND", &plan.first_kept));
        twice.push(text("g"));
        said(&context_messages(&twice))
    };

    // The cut lands after the prior summary: `c`, `d` and FIRST are folded into SECOND
    let after = plan(1_500);
    assert_eq!(after.first_kept, log[5].id);
    assert_eq!(after.previous_summary.as_deref(), Some("FIRST"));
    let read = serialize_for_summary(&after.messages_to_summarize);
    assert!(read.contains("[User]: c") && read.contains("[Compaction]: FIRST"), "{read}");
    assert!(!read.contains("[User]: a") && !read.contains("[User]: b"), "{read}");
    let context = compacted_again(&after);
    assert!(context[0].contains("SECOND") && !context[0].contains("FIRST"));
    assert_eq!(context[1..], ["e", "f", "g"]);

    // The cut lands before it: only `c` is folded, and FIRST rides on the kept side
    let before = plan(2_500);
    assert_eq!(before.first_kept, log[3].id);
    assert_eq!(before.previous_summary.as_deref(), Some("FIRST"));
    let read = serialize_for_summary(&before.messages_to_summarize);
    assert!(read.contains("[User]: c") && !read.contains("FIRST"), "{read}");
    let context = compacted_again(&before);
    // Oldest first: FIRST stands for a and b, SECOND for c, and only then what was kept
    assert!(context[0].contains("FIRST") && !context[0].contains("SECOND"), "{context:?}");
    assert!(context[1].contains("SECOND") && !context[1].contains("FIRST"), "{context:?}");
    assert_eq!(context[2..], ["d", "e", "f", "g"]);
}

#[test]
fn nothing_to_summarize_is_cancelled_not_a_panic() {
    // Occupancy trips the trigger (observed) but the transcript is tiny — must not index OOB.
    let entries = vec![Entry {
        id: EntryId::new(),
        parent_id: None,
        ts: 0,
        kind: EntryKind::Message(Message::user("hi".to_string())),
    }];
    let settings = CompactionSettings { keep_recent_tokens: 100, ..Default::default() };
    let budget = ContextBudget {
        context_window: 200_000,
        max_tokens: 32_000,
        observed_tokens: Some(170_000),
        ..Default::default()
    };
    assert!(matches!(
        find_compaction_point(&entries, &settings, budget, CompactionTrigger::Threshold),
        Err(CompactionError::Cancelled)
    ));
}

/// `count` user messages of `chars` each — a transcript whose estimate is exactly known.
fn user_entries(count: usize, chars: usize) -> Vec<Entry> {
    (0..count)
        .map(|_| Entry {
            id: EntryId::new(),
            parent_id: None,
            ts: 0,
            kind: EntryKind::Message(Message::user("x".repeat(chars))),
        })
        .collect()
}
