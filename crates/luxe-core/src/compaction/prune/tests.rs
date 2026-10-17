use super::*;
use crate::compaction::*;
use crate::message::{ContentBlock, Message, Role};
use crate::session::{Entry, EntryKind};

fn tool_msg(call_id: &str, text_tokens: usize, is_error: bool) -> Entry {
    Entry {
        id: EntryId::new(),
        parent_id: None,
        ts: 0,
        kind: EntryKind::Message(Message {
            role: Role::Tool,
            content: vec![ContentBlock::ToolResult {
                call_id: call_id.to_string(),
                content: vec![ToolOutput::Text("x".repeat(text_tokens * 4))],
                is_error,
            }],
        }),
    }
}

fn msg_of(entry: &Entry) -> Message {
    match &entry.kind {
        EntryKind::Message(m) => m.clone(),
        _ => unreachable!(),
    }
}

/// A window with no room left, so a prune test exercises pruning rather than the gate.
fn under_pressure() -> ContextPressure {
    ContextPressure { peak_tokens: 100_000, usable_window: 100_000 }
}

#[test]
fn prune_protects_recent_and_shrinks_old_tool_output() {
    let settings =
        CompactionSettings { prune_protect_tokens: 40, prune_min_tokens: 20, ..Default::default() };
    // oldest -> newest. Walking back, the tail totalling ~protect (c + b) stays verbatim; a is
    // older than the protected window and gets pruned.
    let mut messages =
        [tool_msg("a", 1000, false), tool_msg("b", 1000, true), tool_msg("c", 30, false)]
            .iter()
            .map(msg_of)
            .collect::<Vec<_>>();

    prune_tool_outputs(&mut messages, &settings, under_pressure());

    match &messages[0].content[0] {
        ContentBlock::ToolResult { call_id, content, is_error } => {
            assert_eq!(call_id, "a", "pairing id preserved");
            assert!(!is_error);
            match &content[0] {
                ToolOutput::Text(t) => assert!(t.contains("elided to save context")),
                _ => panic!("expected placeholder text"),
            }
        }
        _ => panic!("expected tool result"),
    }
    // b (still within the protected tail) is untouched, error flag intact.
    match &messages[1].content[0] {
        ContentBlock::ToolResult { content, is_error, .. } => {
            assert!(is_error);
            match &content[0] {
                ToolOutput::Text(t) => assert_eq!(t.len(), 4000),
                _ => panic!(),
            }
        }
        _ => panic!(),
    }
}

#[test]
fn prune_skips_when_gain_is_below_minimum() {
    // Same shape, but the minimum is huge: not worth churning the prompt cache.
    let settings = CompactionSettings {
        prune_protect_tokens: 40,
        prune_min_tokens: 100_000,
        ..Default::default()
    };
    let mut messages =
        [tool_msg("a", 1000, false), tool_msg("b", 1000, false), tool_msg("c", 30, false)]
            .iter()
            .map(msg_of)
            .collect::<Vec<_>>();

    prune_tool_outputs(&mut messages, &settings, under_pressure());
    match &messages[0].content[0] {
        ContentBlock::ToolResult { content, .. } => match &content[0] {
            ToolOutput::Text(t) => assert_eq!(t.len(), 4000, "nothing pruned below minimum"),
            _ => panic!(),
        },
        _ => panic!(),
    }
}

#[test]
fn prune_elides_old_images_too() {
    let settings =
        CompactionSettings { prune_protect_tokens: 40, prune_min_tokens: 20, ..Default::default() };
    let old_image = Message {
        role: Role::Tool,
        content: vec![ContentBlock::ToolResult {
            call_id: "shot".to_string(),
            content: vec![ToolOutput::Image {
                data: "base64==".to_string(),
                mime: "image/png".to_string(),
                name: None,
            }],
            is_error: false,
        }],
    };
    // Oldest = an image; a big recent text result fills the protected tail.
    let mut messages = vec![old_image, msg_of(&tool_msg("recent", 1000, false))];

    prune_tool_outputs(&mut messages, &settings, under_pressure());

    match &messages[0].content[0] {
        ContentBlock::ToolResult { content, .. } => match &content[0] {
            ToolOutput::Text(t) => assert!(t.contains("elided"), "stale image elided to text"),
            _ => panic!("image should be replaced by a text placeholder"),
        },
        _ => panic!(),
    }
}

#[test]
fn prune_is_a_noop_when_compaction_disabled() {
    let settings = CompactionSettings {
        enabled: false,
        prune_protect_tokens: 40,
        prune_min_tokens: 20,
        ..Default::default()
    };
    let mut messages = [tool_msg("a", 1000, false), tool_msg("b", 1000, false)]
        .iter()
        .map(msg_of)
        .collect::<Vec<_>>();
    prune_tool_outputs(&mut messages, &settings, under_pressure());
    match &messages[0].content[0] {
        ContentBlock::ToolResult { content, .. } => match &content[0] {
            ToolOutput::Text(t) => assert_eq!(t.len(), 4000),
            _ => panic!(),
        },
        _ => panic!(),
    }
}

/// The defect this whole gate exists for: a session at 29% of a 1M window spent 71% of its
/// cost eliding tool output it had all the room in the world for.
#[test]
fn a_roomy_window_is_never_pruned() {
    let settings =
        CompactionSettings { prune_protect_tokens: 40, prune_min_tokens: 20, ..Default::default() };
    let mut messages = [tool_msg("a", 1000, false), tool_msg("b", 1000, false)]
        .iter()
        .map(msg_of)
        .collect::<Vec<_>>();
    let roomy = ContextPressure { peak_tokens: 290_000, usable_window: 980_000 };
    assert!(roomy.fraction() < settings.prune_at_fraction);

    prune_tool_outputs(&mut messages, &settings, roomy);

    let elided = |messages: &[Message]| match &messages[0].content[0] {
        ContentBlock::ToolResult { content, .. } => match &content[0] {
            ToolOutput::Text(text) => text.contains("elided"),
            _ => panic!("expected text"),
        },
        _ => panic!("expected a tool result"),
    };
    assert!(!elided(&messages), "nothing is elided with room left");

    // The gate is "at or above", so landing exactly on it prunes. Pinned because the other
    // reading costs a whole conversation's worth of cache writes one call later.
    let exactly = ContextPressure { peak_tokens: 80_000, usable_window: 100_000 };
    assert!((exactly.fraction() - settings.prune_at_fraction).abs() < f64::EPSILON);
    prune_tool_outputs(&mut messages, &settings, exactly);
    assert!(elided(&messages), "a window at the gate is a window worth reclaiming");
}

/// The expensive half of the same defect, and the invariant that replaces it.
///
/// Every provider that caches does it by exact prefix match, so the transcript a session sends
/// has to be an *append* to the one it sent last time. The old sliding window moved the cut by
/// one result per turn, which rewrote the middle of the array on all 31 of the turns below and
/// re-billed the whole conversation as a cache write each time — no error, no warning, just a
/// four-fold invoice.
#[test]
fn the_outgoing_transcript_is_append_only_across_turns() {
    let settings = CompactionSettings {
        prune_protect_tokens: 40_000,
        prune_min_tokens: 20_000,
        ..Default::default()
    };
    let mut history = vec![Message::user("start")];
    let mut previous: Vec<String> = Vec::new();
    let mut rewrites = 0;

    for turn in 0..60 {
        history.push(msg_of(&tool_msg(&format!("call-{turn}"), 2_000, false)));
        let mut outgoing = history.clone();
        // Pressure rises with the transcript, and crosses the gate part-way through.
        let pressure =
            ContextPressure { peak_tokens: 2_000 * (turn as u64 + 1), usable_window: 100_000 };
        prune_tool_outputs(&mut outgoing, &settings, pressure);

        let rendered: Vec<String> = outgoing.iter().map(|m| format!("{m:?}")).collect::<Vec<_>>();
        if previous.iter().zip(rendered.iter()).any(|(before, after)| before != after) {
            rewrites += 1;
        }
        previous = rendered;
    }

    // 120k of tool output in all; the gate opens at 80k, and above it the cut moves one
    // 20k step at a time — the opening cut plus two steps. Not the 31 the sliding window took.
    assert_eq!(rewrites, 3, "the prefix was rewritten {rewrites} times in 60 turns");
}

/// The gate reads a high-water mark, not current occupancy — otherwise eliding lowers the
/// figure that authorized it, the next request keeps everything back, and the two states
/// alternate, rewriting the transcript on every flip.
#[test]
fn pressure_is_the_fullest_the_window_has_been_this_epoch() {
    let usage = |prompt: u32| Entry {
        id: EntryId::new(),
        parent_id: None,
        ts: 0,
        kind: EntryKind::Usage {
            source: crate::usage::UsageSource::Agent,
            model: "test".to_string(),
            totals: crate::usage::UsageTotals {
                calls: 1,
                prompt_tokens: u64::from(prompt),
                ..Default::default()
            },
        },
    };
    let tagged = |prompt: u32, source, calls| {
        let mut entry = usage(prompt);
        if let EntryKind::Usage { source: tag, totals, .. } = &mut entry.kind {
            *tag = source;
            totals.calls = calls;
        }
        entry
    };
    let measure =
        |entries: &[Entry]| ContextPressure::measure(entries, None, &[], 100_000).peak_tokens;

    // A later, smaller measurement is what a prune looks like; the mark holds.
    assert_eq!(measure(&[usage(40_000), usage(90_000), usage(52_000)]), 90_000);
    // Only the agent's own single-call records are occupancy: a review pass measures a prompt
    // of its own, and a batched flush sums several. Neither describes this conversation.
    assert_eq!(
        measure(&[
            usage(40_000),
            tagged(800_000, crate::usage::UsageSource::Review, 1),
            tagged(700_000, crate::usage::UsageSource::Agent, 4),
        ]),
        40_000,
    );
    // A record of zero is not a measurement — taking it at its word would report an empty
    // window for a transcript that plainly is not one — so the estimate answers instead.
    let said = Entry {
        id: EntryId::new(),
        parent_id: None,
        ts: 0,
        kind: EntryKind::Message(Message::user("x".repeat(8_000))),
    };
    assert_eq!(measure(&[said, tagged(0, crate::usage::UsageSource::Agent, 1)]), 2_000);
    // A compaction starts a new epoch, and the window it emptied is not evidence of pressure.
    let compacted = vec![
        usage(90_000),
        Entry {
            id: EntryId::new(),
            parent_id: None,
            ts: 0,
            kind: EntryKind::Compaction {
                summary: "s".repeat(400),
                first_kept: EntryId::new(),
                tokens_before: 90_000,
            },
        },
        usage(12_000),
    ];
    assert_eq!(measure(&compacted), 12_000);
}

/// The cut is measured from the oldest end so a given result's verdict never flips; measured
/// from the newest it slides every time anything is appended.
#[test]
fn the_elision_boundary_moves_in_whole_steps() {
    let settings = CompactionSettings {
        prune_protect_tokens: 1_000,
        prune_min_tokens: 500,
        ..Default::default()
    };
    let boundary = |tool_tokens: usize| {
        let messages = vec![msg_of(&tool_msg("one", tool_tokens, false))];
        prune_boundary(&messages, &settings)
    };
    assert_eq!(boundary(1_200), 0, "a part-step over the protected tail is not worth a rewrite");
    assert_eq!(boundary(1_600), 500);
    assert_eq!(boundary(1_999), 500, "and it holds there until the next whole step");
    assert_eq!(boundary(2_100), 1_000);
}
