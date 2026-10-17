use super::*;
use crate::compaction::*;
use crate::message::{ContentBlock, Message, Role};
use crate::session::{Entry, EntryKind};

#[test]
fn usable_window_caps_the_output_reserve() {
    // A big max_tokens must not eat the whole window: the reserve is capped at reserve_tokens.
    assert_eq!(usable_context_window(1_000_000, 128_000, 20_000), 980_000);
    // A small max_tokens caps the reserve at itself — never over-reserve.
    assert_eq!(usable_context_window(200_000, 4_096, 20_000), 195_904);
    // Unknown max_tokens (0) reserves the full cap.
    assert_eq!(usable_context_window(200_000, 0, 20_000), 180_000);
    // Degenerate: reserve exceeds the window -> 0, which the caller reads as "don't compact".
    assert_eq!(usable_context_window(10_000, 0, 20_000), 0);
}

/// `max_tokens` is a ceiling, not an allocation. A configured cap that overlaps the prompt is
/// latent rather than safe: llama.cpp and LM Studio clamp it silently, vLLM and OpenAI reject
/// the whole request — so the bug only appears when the endpoint changes.
#[test]
fn the_output_cap_never_claims_room_the_prompt_is_using() {
    let clamp = clamp_max_tokens_to_context;

    // Room to spare: the configured cap is exactly what goes out.
    assert_eq!(clamp(262_144, 10_000, 131_072), Some(131_072));

    // The reported case: a 242K prompt under a 262K window used to ship a 131K cap on top,
    // asking for 373K against a 262K window.
    let capped = clamp(262_144, 242_144, 131_072).expect("a cap");
    assert_eq!(capped, 262_144 - 242_144 - CONTEXT_SAFETY_TOKENS);
    assert!(242_144 + u64::from(capped) <= u64::from(262_144u32 - CONTEXT_SAFETY_TOKENS));

    // An unknown window can't be reasoned about, so nothing is guessed.
    assert_eq!(clamp(0, 500_000, 131_072), Some(131_072));
    // A configured 0 still means "omit the field", which is not the same as "ask for none".
    assert_eq!(clamp(262_144, 10_000, 0), None);
    assert_eq!(clamp(0, 10_000, 0), None);

    // A prompt that has eaten the window asks for a minimal reply rather than zero or a
    // negative, either of which is a hard rejection instead of a short answer.
    assert_eq!(clamp(8_192, 8_192, 32_000), Some(MIN_MAX_TOKENS));
    assert_eq!(clamp(8_192, 999_999, 32_000), Some(MIN_MAX_TOKENS));
}

/// The prompt figure the clamp runs on: measured where the provider told us, estimated only
/// for the tail it never saw.
#[test]
fn the_prompt_figure_prefers_what_the_provider_measured() {
    use crate::usage::{UsageSource, UsageTotals};

    let text = |chars: usize| Entry {
        id: EntryId::new(),
        parent_id: None,
        ts: 0,
        kind: EntryKind::Message(Message::user("x".repeat(chars))),
    };
    let usage = |prompt: u64, completion: u64| Entry {
        id: EntryId::new(),
        parent_id: None,
        ts: 0,
        kind: EntryKind::Usage {
            source: UsageSource::Agent,
            model: "test-model".into(),
            totals: UsageTotals {
                calls: 1,
                prompt_tokens: prompt,
                completion_tokens: completion,
                ..UsageTotals::default()
            },
        },
    };
    // `read` + 400 characters of description + `{}` = 406 characters, at four to the token.
    let tools = [ToolSchema {
        name: "read".into(),
        description: "d".repeat(400),
        parameters: serde_json::json!({}),
    }];
    assert_eq!(estimate_prefix_tokens(None, &tools), 101);
    assert_eq!(estimate_prefix_tokens(Some(&"s".repeat(8_000)), &tools), 2_101);
    assert_eq!(estimate_prefix_tokens(None, &[]), 0);

    // Nothing measured: the whole transcript, plus the prefix the transcript can't see. The
    // system prompt and the tool schemas are most of a fresh request, so leaving them out is
    // how a request that "fits" turns into a 400.
    let fresh = [text(4_000)];
    assert_eq!(prompt_tokens(&fresh, None, &[]), 1_000);
    assert_eq!(prompt_tokens(&fresh, Some(&"s".repeat(8_000)), &tools), 1_000 + 2_101);

    // Measured: the measurement already includes that call's prefix, so only what was appended
    // after it is estimated on top.
    let live = [text(4_000), usage(60_000, 500), text(8_000)];
    assert_eq!(prompt_tokens(&live, Some("system"), &tools), 60_500 + 2_000);
}

#[test]
fn fullness_percent_is_used_over_usable() {
    assert_eq!(context_fullness_percent(90_000, 180_000), 50.0);
    // Unknowns read as empty, never full.
    assert_eq!(context_fullness_percent(0, 180_000), 0.0);
    assert_eq!(context_fullness_percent(90_000, 0), 0.0);
}

/// Two units meet in the cut, and getting them wrong is what left compaction with nothing to
/// free: `keep_recent_tokens` is a real-token budget, and the walk that honors it counts char/4
/// estimates. On source, which tokenizes nearer 2.4 characters per token, a tail asked to be
/// 20k came out around 33k.
#[test]
fn the_kept_tail_is_measured_in_the_tokens_the_provider_charges_for() {
    // A transcript the estimate reads as 20k that the provider measured at 34k.
    assert!((estimate_calibration(Some(34_000), 20_000) - 1.7).abs() < 1e-9);
    // Nothing measured yet, so nothing to correct with.
    assert_eq!(estimate_calibration(None, 20_000), 1.0);
    assert_eq!(estimate_calibration(Some(0), 20_000), 1.0);
    assert_eq!(estimate_calibration(Some(34_000), 0), 1.0);
    // A ratio under the estimate would make the tail *bigger* than the setting asked for,
    // which is the failure this exists to fix — so the estimate is the floor.
    assert_eq!(estimate_calibration(Some(500), 20_000), 1.0);
    // And no single measurement gets to summarize the entire session.
    assert_eq!(estimate_calibration(Some(9_000_000), 20_000), 4.0);
}

#[test]
fn test_estimate_tokens() {
    let entries = vec![Entry {
        id: EntryId::new(),
        parent_id: None,
        ts: 0,
        kind: EntryKind::Message(Message {
            role: Role::User,
            content: vec![ContentBlock::Text("Hello world".to_string())],
        }),
    }];

    let tokens = estimate_tokens(&entries);
    // "Hello world" is 11 chars, so ~2-3 tokens.
    assert!(tokens >= 2);
    assert!(tokens <= 4);
}

/// The bug that emptied the context reading on resume, in miniature: a coding session's context is
/// tool results, tool calls and reasoning, and counting only `Text` blocks put a 70K-token
/// transcript at 956. Estimating *less than a tenth* of the truth is not conservatism — it is a
/// compaction check that never fires.
#[test]
fn the_estimate_counts_everything_that_reaches_the_model() {
    let entry = |content: Vec<ContentBlock>| Entry {
        id: EntryId::new(),
        parent_id: None,
        ts: 0,
        kind: EntryKind::Message(Message { role: Role::Assistant, content }),
    };
    let body = "x".repeat(4_000); // ~1000 tokens each
    let cases: Vec<(&str, ContentBlock)> = vec![
        ("text", ContentBlock::Text(body.clone())),
        (
            "reasoning",
            ContentBlock::Thinking {
                text: body.clone(),
                signature: None,
                id: None,
                encrypted: None,
                opaque: None,
            },
        ),
        ("redacted reasoning", ContentBlock::RedactedThinking { data: body.clone() }),
        (
            "tool call",
            ContentBlock::ToolCall {
                id: "call_1".into(),
                name: "read".into(),
                arguments: serde_json::json!({ "path": body.clone() }),
            },
        ),
        (
            "tool result",
            ContentBlock::ToolResult {
                call_id: "call_1".into(),
                content: vec![ToolOutput::Text(body.clone())],
                is_error: false,
            },
        ),
    ];
    for (label, block) in cases {
        let tokens = estimate_tokens(&[entry(vec![block])]);
        assert!(tokens >= 900, "{label} contributed only {tokens} tokens");
        assert!(tokens <= 1_100, "{label} inflated 4,000 characters to {tokens} tokens");
    }
    // An image has no characters to count, so it takes the flat per-image figure.
    let image = ContentBlock::Image { data: "AAAA".into(), mime: "image/png".into(), name: None };
    assert_eq!(estimate_tokens(&[entry(vec![image])]), IMAGE_TOKEN_ESTIMATE);

    // A summary is the whole of what a compacted conversation still occupies, so an entry that
    // counted as nothing would report a compacted session as an empty one.
    assert_eq!(
        estimate_tokens(&[Entry {
            id: EntryId::new(),
            parent_id: None,
            ts: 0,
            kind: EntryKind::Compaction {
                summary: "s".repeat(4_000),
                first_kept: EntryId::new(),
                tokens_before: 90_000,
            },
        }]),
        1_000
    );

    // The rate itself, pinned, over both entry- and message-shaped input. The estimator is
    // load-bearing twice over — it decides when to compact, and through `prompt_tokens` how
    // much output a request may ask for — so a rate that is out by a factor drives every
    // request to the one-token floor.
    assert_eq!(estimate_tokens(&[entry(vec![ContentBlock::Text(body.clone())])]), 1_000);
    assert_eq!(
        estimate_message_tokens(&[
            Message::user(body.clone()),
            Message { role: Role::Assistant, content: vec![ContentBlock::Text(body)] },
        ]),
        2_000
    );
    assert_eq!(estimate_message_tokens(&[]), 0);
}

/// Resume prefers the measurement over the guess, and knows which measurements are meaningless.
#[test]
fn observed_occupancy_reads_the_last_agent_call_and_stops_at_a_compaction() {
    use crate::usage::{UsageSource, UsageTotals};

    let usage = |source: UsageSource, calls: u64, prompt: u64, completion: u64| Entry {
        id: EntryId::new(),
        parent_id: None,
        ts: 0,
        kind: EntryKind::Usage {
            source,
            model: "test-model".into(),
            totals: UsageTotals {
                calls,
                prompt_tokens: prompt,
                completion_tokens: completion,
                ..UsageTotals::default()
            },
        },
    };
    let compaction = || Entry {
        id: EntryId::new(),
        parent_id: None,
        ts: 0,
        kind: EntryKind::Compaction {
            summary: "summarized".into(),
            first_kept: EntryId::new(),
            tokens_before: 90_000,
        },
    };

    // The newest agent call wins, and it is prompt + completion — what the window actually held.
    assert_eq!(
        last_observed_context_tokens(&[
            usage(UsageSource::Agent, 1, 10_000, 100),
            usage(UsageSource::Agent, 1, 69_897, 415),
        ]),
        Some(70_312)
    );

    // A record from before a compaction describes a context that no longer exists.
    assert_eq!(
        last_observed_context_tokens(&[usage(UsageSource::Agent, 1, 90_000, 500), compaction(),]),
        None,
        "stale occupancy would re-trigger compaction on a session that just compacted"
    );
    // ...but a call made *after* the compaction is current again.
    assert_eq!(
        last_observed_context_tokens(&[
            usage(UsageSource::Agent, 1, 90_000, 500),
            compaction(),
            usage(UsageSource::Agent, 1, 12_000, 200),
        ]),
        Some(12_200)
    );

    // A batched entry's summed prompt tokens are not an occupancy figure, and another source's
    // call measures its own context, not the turn's.
    assert_eq!(last_observed_context_tokens(&[usage(UsageSource::Agent, 4, 200_000, 900)]), None);
    assert_eq!(last_observed_context_tokens(&[usage(UsageSource::Review, 1, 50_000, 100)]), None);
    assert_eq!(last_observed_context_tokens(&[]), None);

    // A call that measured nothing measured nothing — it is not an empty context. Reading it
    // as one would report a full window as fresh and hand the next request the whole output
    // cap on top of it.
    assert_eq!(
        last_observed_context_tokens(&[
            usage(UsageSource::Agent, 1, 80_000, 400),
            usage(UsageSource::Agent, 1, 0, 0),
        ]),
        Some(80_400)
    );
}

/// A `Usage` entry is a cost record, not conversation — only messages reach the model, so only
/// messages may count toward what a request will occupy.
///
/// Load-bearing beyond the obvious: it is why `prompt_tokens` may estimate the entries *after*
/// the measurement it starts from without having to care whether the measurement itself is
/// included.
#[test]
fn usage_entries_are_inert_for_estimation() {
    let ledger = Entry {
        id: EntryId::new(),
        parent_id: None,
        ts: 0,
        kind: EntryKind::Usage {
            source: crate::usage::UsageSource::Agent,
            model: "a-model-with-a-long-name".to_string(),
            totals: crate::usage::UsageTotals { calls: 1, prompt_tokens: 90_000, ..<_>::default() },
        },
    };
    assert_eq!(entry_to_token_estimate(&ledger), 0);
    assert_eq!(estimate_tokens(std::slice::from_ref(&ledger)), 0);
}
