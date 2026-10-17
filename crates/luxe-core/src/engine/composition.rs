//! Context composition — measuring the request `/context` reports on.
//!
//! One pass over the three things a request is made of: the system prompt (in the sections that
//! assembled it), the tool block (as the JSON that ships), and the active transcript. Everything is
//! counted in characters here; turning characters into tokens is
//! [`crate::context_report::CompositionBuilder`]'s job, and it normalizes against the occupancy the
//! provider reported so the rows sum to the reading.
//!
//! Nothing here sends anything. The measurement describes the request the next turn would make,
//! which for the transcript is the request it *did* make, and for the prompt is the one the session
//! sends, or would write from the files now if it has not sent one yet.

use super::*;
use crate::context::PromptSection;
use crate::context_report::{
    CompositionBuilder, ContextComposition, ContextDetail, ContextPart, McpFootprint,
};

/// Named items kept per part. Enough to find the thing worth cutting, few enough to read.
const MAX_DETAIL_ROWS: usize = 6;

/// How much of a tool call's arguments a detail label carries.
const DETAIL_ARG_CHARS: usize = 48;

impl Engine {
    /// Measures what the next request would carry, and what to do about it.
    ///
    /// Reads the same sources a turn does and sends nothing.
    pub(crate) async fn measure_context(&self) -> Result<ContextComposition, EngineError> {
        let model = self.effective_model().await;
        let mut builder = CompositionBuilder::new();

        for section in self.measure_prompt().await? {
            builder.add(part_for(section.section), chars(&section.text));
        }

        let entries = self.active_entries().await;
        let activated = crate::tool_activation::activated_in(&entries);
        let schemas = self.config.tool_registry.request_schemas(&activated);
        let mut tool_detail: Vec<ContextDetail> = schemas
            .iter()
            .map(|schema| ContextDetail {
                label: schema.name.clone(),
                tokens: schema_chars(schema),
            })
            .collect();
        builder.add(ContextPart::ToolSchemas, tool_detail.iter().map(|item| item.tokens).sum());
        rank(&mut tool_detail);
        builder.detail(ContextPart::ToolSchemas, tool_detail);

        // What the next request would carry, which after a compaction is the summary plus the tail
        // it kept — not the whole path. Reporting the path would show a window the request never
        // fills and put the rows at odds with the reading above them.
        let conversation = measure_conversation(crate::compaction::context_entries(&entries));
        builder.add(ContextPart::Conversation, conversation.chars);
        builder.detail(ContextPart::Conversation, conversation.detail);

        let measured = crate::compaction::last_observed_context_tokens(&entries);
        let (slices, used_tokens, calibration) = builder.resolve(measured);

        let usage = self.usage_snapshot();
        let mut composition = ContextComposition {
            model: model.display().to_string(),
            context_window: model.context_window,
            used_tokens,
            calibration,
            slices,
            findings: Vec::new(),
            cache_hit_pct: usage.total.cache_hit_pct(),
            cache_write_pct: usage.total.cache_write_pct(),
            mcp: self.mcp_footprint(&schemas),
        };
        let cwd = self.instruction_dir();
        composition
            .derive_findings(usage.total.calls, &super::prompt::truncated_instruction_files(&cwd));
        Ok(composition)
    }

    /// The active path's entries, or an empty transcript when the session has no leaf yet.
    async fn active_entries(&self) -> Vec<crate::session::Entry> {
        let session = self.current_session_id().await;
        let leaf = self.current_leaf.lock().await.clone();
        let Ok(store) = self.config.session_store.read() else {
            return Vec::new();
        };
        match leaf {
            Some(leaf) => store.active_path(&session, &leaf).unwrap_or_default(),
            None => Vec::new(),
        }
    }

    /// What MCP actually costs: the servers behind the one gateway tool, and that tool's size.
    ///
    /// `None` when no server is enabled — a report should not carry a row about a subsystem the
    /// reader is not using.
    fn mcp_footprint(&self, schemas: &[crate::provider::ToolSchema]) -> Option<McpFootprint> {
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let servers = crate::mcp_config::McpConfigStore::for_repo(&cwd)
            .list()
            .ok()?
            .into_iter()
            .filter(|server| server.enabled)
            .count();
        if servers == 0 {
            return None;
        }
        let gateway = schemas.iter().find(|schema| schema.name == "mcp").map(schema_chars)?;
        Some(McpFootprint {
            servers,
            // Characters, converted the way the tool block is: this figure sits beside those rows.
            gateway_tokens: (gateway as f64 / ContextPart::ToolSchemas.weight()).round() as u64,
        })
    }
}

/// Which part of the report a prompt section bills to.
fn part_for(section: PromptSection) -> ContextPart {
    match section {
        PromptSection::Harness => ContextPart::Harness,
        PromptSection::Instructions => ContextPart::Instructions,
        PromptSection::Skills => ContextPart::Skills,
        // The deferred catalog is the other half of the tool block — lazy tools move cost from the
        // schemas into it, so billing it anywhere else would make trimming tools look free. Folded
        // in rather than given a row, because `tools.disabled` shrinks both and one row is what a
        // reader acts on.
        PromptSection::DeferredTools => ContextPart::ToolSchemas,
        PromptSection::Appended => ContextPart::Appended,
    }
}

/// What one tool's schema occupies on the wire, as the JSON it serializes to.
fn schema_chars(schema: &crate::provider::ToolSchema) -> u64 {
    let parameters =
        serde_json::to_string(&schema.parameters).map(|json| chars(&json)).unwrap_or(0);
    // The envelope every wire format wraps a tool in — name, description, the JSON punctuation
    // around them. Approximate by construction; the alternative is one measurement per provider
    // shape for a difference of a few dozen characters per tool.
    chars(&schema.name) + chars(&schema.description) + parameters
}

/// The conversation's characters, and the largest results inside it.
struct Conversation {
    chars: u64,
    detail: Vec<ContextDetail>,
}

/// Measures the transcript: total characters, plus the tool results worth naming.
///
/// Results are attributed to the call that produced them, because "`read` of `view.rs` returned
/// 12k" is actionable and "a tool result was large" is not. A call whose result never arrived
/// contributes its own characters and no detail row.
fn measure_conversation<'a>(
    entries: impl IntoIterator<Item = &'a crate::session::Entry>,
) -> Conversation {
    let mut total = 0u64;
    let mut calls: HashMap<String, String> = HashMap::new();
    let mut detail: HashMap<String, u64> = HashMap::new();

    for entry in entries {
        let message = match &entry.kind {
            EntryKind::Message(message) => message,
            // A compaction summary is what the messages it replaced became, so it is conversation.
            EntryKind::Compaction { summary, .. } => {
                total += chars(summary);
                continue;
            }
            _ => continue,
        };
        for block in &message.content {
            total += block_chars(block);
            match block {
                ContentBlock::ToolCall { id, name, arguments } => {
                    calls.insert(id.clone(), call_label(name, arguments));
                }
                ContentBlock::ToolResult { call_id, .. } => {
                    let label =
                        calls.get(call_id).cloned().unwrap_or_else(|| "a tool result".to_string());
                    *detail.entry(label).or_default() += block_chars(block);
                }
                _ => {}
            }
        }
    }

    let mut detail: Vec<ContextDetail> =
        detail.into_iter().map(|(label, tokens)| ContextDetail { label, tokens }).collect();
    rank(&mut detail);
    Conversation { chars: total, detail }
}

/// `name(first argument)` — enough to recognize the call in a list.
fn call_label(name: &str, arguments: &serde_json::Value) -> String {
    let inner = arguments
        .as_object()
        .and_then(|map| map.values().find_map(serde_json::Value::as_str))
        .map(|value| super::prompt::clip(value, DETAIL_ARG_CHARS))
        .unwrap_or_default();
    if inner.is_empty() {
        format!("`{name}`")
    } else {
        format!("`{name}` {inner}")
    }
}

/// Characters one content block puts on the wire.
///
/// Mirrors [`crate::compaction`]'s breadth deliberately: counting only text is the mistake that
/// once reported a 69,897-token transcript as 956, and a report that repeats it would put the
/// conversation row at a fraction of its real size.
fn block_chars(block: &ContentBlock) -> u64 {
    match block {
        ContentBlock::Text(text) => chars(text),
        ContentBlock::Thinking { text, signature, id, encrypted, opaque } => {
            chars(text)
                + [signature, id, encrypted, opaque]
                    .into_iter()
                    .flatten()
                    .map(|value| chars(value))
                    .sum::<u64>()
        }
        ContentBlock::RedactedThinking { data } => chars(data),
        ContentBlock::ToolCall { name, arguments, .. } => {
            chars(name) + arguments.to_string().chars().count() as u64
        }
        ContentBlock::ToolResult { content, .. } => content.iter().map(output_chars).sum(),
        // An image is the one thing characters cannot describe, so it takes the flat figure
        // compaction already uses, converted back into the units this pass counts in.
        ContentBlock::Image { .. } => {
            (crate::compaction::IMAGE_TOKEN_ESTIMATE as f64 * ContextPart::Conversation.weight())
                as u64
        }
    }
}

/// Characters one piece of tool output puts on the wire.
fn output_chars(output: &crate::message::ToolOutput) -> u64 {
    match output {
        crate::message::ToolOutput::Text(text) => chars(text),
        crate::message::ToolOutput::Image { .. } => {
            (crate::compaction::IMAGE_TOKEN_ESTIMATE as f64 * ContextPart::Conversation.weight())
                as u64
        }
    }
}

/// Largest first, then bounded — a detail list is for finding the outlier, not for auditing.
fn rank(detail: &mut Vec<ContextDetail>) {
    detail.sort_by(|left, right| {
        right.tokens.cmp(&left.tokens).then_with(|| left.label.cmp(&right.label))
    });
    detail.truncate(MAX_DETAIL_ROWS);
}

/// Characters, not bytes: a multi-byte path would otherwise inflate its own row.
fn chars(text: &str) -> u64 {
    text.chars().count() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{Message, Role, ToolOutput};

    fn entry(blocks: Vec<ContentBlock>) -> crate::session::Entry {
        crate::session::Entry {
            id: crate::session::EntryId::new(),
            parent_id: None,
            ts: 0,
            kind: EntryKind::Message(Message { role: Role::Assistant, content: blocks }),
        }
    }

    /// The bug this pass must not repeat: counting only text under-reports a transcript that is
    /// mostly tool results by orders of magnitude.
    #[test]
    fn every_block_kind_is_counted() {
        let entries = vec![entry(vec![
            ContentBlock::Text("a".repeat(10)),
            ContentBlock::Thinking {
                text: "b".repeat(20),
                signature: Some("c".repeat(5)),
                id: None,
                encrypted: None,
                opaque: None,
            },
            ContentBlock::ToolCall {
                id: "1".into(),
                name: "read".into(),
                arguments: serde_json::json!({ "path": "src/lib.rs" }),
            },
            ContentBlock::ToolResult {
                call_id: "1".into(),
                content: vec![ToolOutput::Text("d".repeat(100))],
                is_error: false,
            },
        ])];
        let measured = measure_conversation(&entries);
        assert!(measured.chars > 135, "text alone would be 10: {}", measured.chars);
    }

    /// A result is named by the call that produced it, so the row says what to narrow.
    #[test]
    fn a_large_result_is_attributed_to_its_call() {
        let entries = vec![entry(vec![
            ContentBlock::ToolCall {
                id: "abc".into(),
                name: "read".into(),
                arguments: serde_json::json!({ "path": "crates/luxe-tui/src/view.rs" }),
            },
            ContentBlock::ToolResult {
                call_id: "abc".into(),
                content: vec![ToolOutput::Text("x".repeat(4_000))],
                is_error: false,
            },
        ])];
        let measured = measure_conversation(&entries);
        assert_eq!(measured.detail.len(), 1);
        assert!(
            measured.detail[0].label.contains("read")
                && measured.detail[0].label.contains("view.rs"),
            "{}",
            measured.detail[0].label
        );
        assert_eq!(measured.detail[0].tokens, 4_000);
    }

    /// An orphaned result still costs what it costs; it just cannot be named.
    #[test]
    fn a_result_with_no_call_is_still_counted() {
        let entries = vec![entry(vec![ContentBlock::ToolResult {
            call_id: "gone".into(),
            content: vec![ToolOutput::Text("y".repeat(500))],
            is_error: false,
        }])];
        let measured = measure_conversation(&entries);
        assert_eq!(measured.chars, 500);
        assert_eq!(measured.detail[0].label, "a tool result");
    }

    /// Repeated calls to the same target roll up, so a loop shows as one large row rather than
    /// twenty small ones that individually look fine.
    #[test]
    fn repeated_calls_to_the_same_target_accumulate() {
        let mut blocks = Vec::new();
        for index in 0..3 {
            blocks.push(ContentBlock::ToolCall {
                id: index.to_string(),
                name: "grep".into(),
                arguments: serde_json::json!({ "pattern": "fn main" }),
            });
            blocks.push(ContentBlock::ToolResult {
                call_id: index.to_string(),
                content: vec![ToolOutput::Text("z".repeat(1_000))],
                is_error: false,
            });
        }
        let measured = measure_conversation(&[entry(blocks)]);
        assert_eq!(measured.detail.len(), 1);
        assert_eq!(measured.detail[0].tokens, 3_000);
    }

    /// A compaction summary is what the messages it replaced became.
    #[test]
    fn a_compaction_summary_counts_as_conversation() {
        let entries = vec![crate::session::Entry {
            id: crate::session::EntryId::new(),
            parent_id: None,
            ts: 0,
            kind: EntryKind::Compaction {
                summary: "s".repeat(800),
                first_kept: crate::session::EntryId::new(),
                tokens_before: 0,
            },
        }];
        assert_eq!(measure_conversation(&entries).chars, 800);
    }

    /// Detail lists are for spotting the outlier, so they stay short.
    #[test]
    fn detail_rows_are_ranked_and_bounded() {
        let mut detail: Vec<ContextDetail> = (0..20)
            .map(|index| ContextDetail { label: format!("t{index}"), tokens: index * 10 })
            .collect();
        rank(&mut detail);
        assert_eq!(detail.len(), MAX_DETAIL_ROWS);
        assert_eq!(detail[0].tokens, 190);
        assert!(detail.windows(2).all(|pair| pair[0].tokens >= pair[1].tokens));
    }

    /// Every section bills to the row whose lever changes it.
    #[test]
    fn prompt_sections_map_onto_actionable_parts() {
        assert_eq!(part_for(PromptSection::Harness), ContextPart::Harness);
        assert_eq!(part_for(PromptSection::Instructions), ContextPart::Instructions);
        assert_eq!(part_for(PromptSection::Skills), ContextPart::Skills);
        assert_eq!(
            part_for(PromptSection::DeferredTools),
            ContextPart::ToolSchemas,
            "a deferred tool's catalog line is still what tooling costs"
        );
        assert_eq!(part_for(PromptSection::Appended), ContextPart::Appended);
    }
}
