//! Context report — what occupies the window, measured rather than guessed.
//!
//! The engine assembles a request from a system prompt, a tool block, and a transcript. Occupancy
//! is already reported as one percentage; this is that percentage resolved into its parts, so the
//! two questions a full window raises — *what do I cut* and *why did it fill so fast* — have an
//! answer that names a lever instead of a number.
//!
//! # How the arithmetic stays honest
//!
//! Three facts, combined in one direction:
//!
//! - **The total is measured.** [`crate::compaction::last_observed_context_tokens`] reads the last
//!   agent usage record after the newest compaction — the occupancy the provider billed for.
//! - **The parts are measured in characters**, at assembly, from the strings that go on the wire.
//! - **One calibration turns characters into tokens**, taken from this session's own calls: the
//!   request's character count against the `prompt_tokens` the provider returned for it.
//!
//! Categories then carry a `weight`, because JSON tokenizes at roughly three characters per token
//! and prose at four; a single ratio understates the tool block by about a third, and the tool block
//! is both the largest fixed category and the one with a lever attached. Finally every part is
//! **normalized to the measured total**, so the breakdown sums to the truth by construction and can
//! never contradict the reading beside it.
//!
//! A session with no completed call has nothing measured to normalize against, so
//! [`Calibration::Estimated`] says so and the surface repeats it. That is the only mode in which
//! the report hedges.

use serde::{Deserialize, Serialize};

/// Characters per token for prose — English at roughly four, the figure every harness uses.
const PROSE_CHARS_PER_TOKEN: f64 = 4.0;

/// Characters per token for serialized JSON, which is punctuation-dense and tokenizes shorter.
const JSON_CHARS_PER_TOKEN: f64 = 3.0;

/// A part of the window, grouped by the lever that changes it.
///
/// Not by where the text came from: `## Environment` and `## Role` are assembled separately and are
/// one row here, because no user setting moves either without moving the other. Ordering is
/// declaration order and is only a tiebreak — the surface ranks by size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextPart {
    /// Messages, tool calls, tool results, reasoning, images.
    Conversation,
    /// The JSON tool block that ships with every request.
    ToolSchemas,
    /// The `AGENTS.md` chain.
    Instructions,
    /// The always-on harness prompt: role, communication, standards, memory, discipline,
    /// environment, tool guidance.
    Harness,
    /// The discoverable skill index.
    Skills,
    /// The one-time resume briefing.
    Appended,
}

impl ContextPart {
    /// Every part, in declaration order.
    pub const ALL: [Self; 6] = [
        Self::Conversation,
        Self::ToolSchemas,
        Self::Instructions,
        Self::Harness,
        Self::Skills,
        Self::Appended,
    ];

    /// The row label.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Conversation => "conversation",
            Self::ToolSchemas => "tool schemas",
            Self::Instructions => "instructions",
            Self::Harness => "harness",
            Self::Skills => "skills",
            Self::Appended => "appended",
        }
    }

    /// Characters per token for this part's content.
    ///
    /// Only the tool block is JSON end to end. Tool *results* inside the conversation are often
    /// JSON too, but they arrive mixed with prose and diffs, so claiming a JSON ratio for the whole
    /// transcript would trade one bias for a larger one.
    pub const fn weight(self) -> f64 {
        match self {
            Self::ToolSchemas => JSON_CHARS_PER_TOKEN,
            _ => PROSE_CHARS_PER_TOKEN,
        }
    }
}

/// Where the token figures came from, so the surface can say it in one line.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Calibration {
    /// The provider reported occupancy for a request whose size we measured.
    ///
    /// `chars_per_token` is that ratio, carried for display; the split is derived from it and then
    /// normalized to the measured total.
    Measured { chars_per_token: f64 },
    /// Nothing has been billed yet, so the whole report is the fallback ratio.
    Estimated,
}

impl Calibration {
    /// Whether the total behind this report is a measurement.
    pub fn is_measured(&self) -> bool {
        matches!(self, Self::Measured { .. })
    }
}

/// A named item inside a part — one instruction file, one tool, one oversized tool result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextDetail {
    /// What it is: a path, a tool name, a call summary.
    pub label: String,
    /// Its share of the window, in tokens.
    pub tokens: u64,
}

/// One part of the window and what is inside it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextSlice {
    /// Which part.
    pub part: ContextPart,
    /// Its share of the window, in tokens.
    pub tokens: u64,
    /// Named items inside it, largest first and bounded. Empty when naming them adds nothing.
    pub detail: Vec<ContextDetail>,
}

/// How loudly a finding should read.
///
/// Ordered least to most severe, which is what sorts the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingLevel {
    /// Worth knowing; no deadline.
    Note,
    /// Acting on it soon avoids a worse turn.
    Warn,
    /// The next long turn is the one that breaks.
    Alert,
}

/// One thing worth doing about the window, and the lever that does it.
///
/// A finding with no lever is a complaint, so `lever` is not optional in spirit even where the
/// wording carries it: every constructor names a setting, a command, or a file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextFinding {
    /// How loudly it reads.
    pub level: FindingLevel,
    /// What is true, in one line.
    pub text: String,
}

/// The whole report: the window, its parts, and what to do about it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextComposition {
    /// The model the figures describe.
    pub model: String,
    /// The model's context window in tokens, or zero when the catalog does not publish one.
    pub context_window: u32,
    /// Occupancy in tokens: measured where [`Calibration::Measured`], estimated otherwise.
    pub used_tokens: u64,
    /// Where the figures came from.
    pub calibration: Calibration,
    /// The parts, largest first. A part with nothing in it is omitted.
    pub slices: Vec<ContextSlice>,
    /// What to do about it, most severe first. Empty when there is nothing to say.
    pub findings: Vec<ContextFinding>,
    /// Share of input tokens served from cache this session, when any call has reported one.
    ///
    /// Carried for the prefix-churn finding, not for display: what the cache saves is spend, and
    /// `/cost` owns spend. Printing the same number on two surfaces is how two surfaces come to
    /// disagree.
    pub cache_hit_pct: Option<f32>,
    /// Share of input tokens re-written into the cache this session.
    ///
    /// The other half of the same question, and the half a hit rate cannot answer: a prefix that
    /// is rewritten every turn still reads most of itself back, so the hit rate stays high while
    /// the bill is almost entirely writes.
    #[serde(default)]
    pub cache_write_pct: Option<f32>,
    /// Enabled MCP servers, and the tokens their single gateway tool costs.
    ///
    /// A line rather than a part: Luxe exposes every server through one gateway tool, so the
    /// category the rest of the field warns about is always near zero here — and a row that always
    /// reads zero teaches nothing.
    pub mcp: Option<McpFootprint>,
}

/// What MCP actually costs the window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpFootprint {
    /// Enabled servers.
    pub servers: usize,
    /// Tokens the gateway tool's schema occupies.
    pub gateway_tokens: u64,
}

impl ContextComposition {
    /// Tokens left before the window is full, or `None` when the window size is unknown.
    pub fn free_tokens(&self) -> Option<u64> {
        (self.context_window > 0)
            .then(|| u64::from(self.context_window).saturating_sub(self.used_tokens))
    }

    /// Occupancy as a percentage, or `None` when the window size is unknown.
    ///
    /// A local model whose catalog entry publishes no window gets no percentage rather than a
    /// made-up one.
    pub fn used_pct(&self) -> Option<f32> {
        (self.context_window > 0)
            .then(|| (self.used_tokens as f64 / f64::from(self.context_window) * 100.0) as f32)
    }

    /// One part's share of the *used* window, as a percentage.
    pub fn share_pct(&self, slice: &ContextSlice) -> f32 {
        if self.used_tokens == 0 {
            return 0.0;
        }
        (slice.tokens as f64 / self.used_tokens as f64 * 100.0) as f32
    }
}

/// Accumulates measured character counts, then resolves them into a report.
///
/// Separated from the engine so the arithmetic is testable without a provider, a session store, or
/// a filesystem — which is what lets the normalization rule below have a test that fails when it
/// stops holding.
#[derive(Debug, Default)]
pub struct CompositionBuilder {
    parts: Vec<(ContextPart, u64, Vec<ContextDetail>)>,
}

impl CompositionBuilder {
    /// An empty builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Folds `chars` into `part`, summing when the part is already present.
    pub fn add(&mut self, part: ContextPart, chars: u64) {
        match self.parts.iter_mut().find(|(existing, _, _)| *existing == part) {
            Some((_, total, _)) => *total += chars,
            None => self.parts.push((part, chars, Vec::new())),
        }
    }

    /// Attaches named items to `part`, replacing anything already recorded for it.
    ///
    /// Details are measured in characters like everything else; they are converted with the same
    /// ratio as their part, so a detail can never claim more tokens than the row above it.
    pub fn detail(&mut self, part: ContextPart, detail: Vec<ContextDetail>) {
        if let Some((_, _, existing)) =
            self.parts.iter_mut().find(|(existing, _, _)| *existing == part)
        {
            *existing = detail;
        }
    }

    /// Resolves characters into tokens against a measured total.
    ///
    /// `measured` is the occupancy the provider reported, when there is one. The per-part estimate
    /// is `chars / weight`; the estimates are then scaled so they sum to `measured`, which is what
    /// makes the rows add up to the reported occupancy rather than to an arithmetic of their own.
    pub fn resolve(self, measured: Option<u64>) -> (Vec<ContextSlice>, u64, Calibration) {
        let estimates: Vec<(ContextPart, f64, Vec<ContextDetail>)> = self
            .parts
            .into_iter()
            .map(|(part, chars, detail)| (part, chars as f64 / part.weight(), detail))
            .collect();
        let estimated_total: f64 = estimates.iter().map(|(_, tokens, _)| tokens).sum();

        let (scale, used, calibration) = match measured {
            Some(measured) if measured > 0 && estimated_total > 0.0 => {
                let scale = measured as f64 / estimated_total;
                // Report the ratio the run actually exhibited, not the prose constant: it is the
                // one number that says how far the model's tokenizer sits from the assumption.
                let chars_per_token = PROSE_CHARS_PER_TOKEN / scale;
                (scale, measured, Calibration::Measured { chars_per_token })
            }
            _ => (1.0, estimated_total.round() as u64, Calibration::Estimated),
        };

        let mut slices: Vec<ContextSlice> = estimates
            .into_iter()
            .map(|(part, tokens, detail)| ContextSlice {
                part,
                tokens: (tokens * scale).round() as u64,
                detail: detail
                    .into_iter()
                    .map(|item| ContextDetail {
                        label: item.label,
                        tokens: (item.tokens as f64 / part.weight() * scale).round() as u64,
                    })
                    .collect(),
            })
            .filter(|slice| slice.tokens > 0)
            .collect();
        slices.sort_by(|left, right| {
            right.tokens.cmp(&left.tokens).then_with(|| left.part.cmp(&right.part))
        });
        (slices, used, calibration)
    }
}

/// Formats a token count for a report row (`1.2k`, `1.2M`), reusing the ledger's scale.
pub use crate::usage::format_tokens;

/// Occupancy past which the next long turn is the one that compacts.
const FULL_PCT: f32 = 90.0;
/// Occupancy past which it is worth saying something.
const TIGHT_PCT: f32 = 75.0;
/// Share of the window a conversation may hold before summarizing it is the obvious move.
const CONVERSATION_HEAVY_PCT: f32 = 50.0;
/// Share of the *fixed* overhead the tool block may hold before trimming it is worth naming.
const TOOL_BLOCK_HEAVY_PCT: f32 = 25.0;
/// Share of the window one tool result may hold before it is worth naming the call.
const BIG_RESULT_PCT: f32 = 5.0;
/// Cache hit rate under which the cached prefix is being thrown away rather than reused.
const CACHE_COLD_PCT: f32 = 20.0;
/// Calls needed before a cache rate means anything — the first one has nothing to reuse.
const CACHE_MIN_CALLS: u64 = 3;
/// Share of input a session may re-write into the cache before something is wrong.
///
/// A healthy session writes one turn's delta per call, so over `n` calls the written share tends
/// to `2/n` — single digits by the twentieth. Anything near a quarter means whole prefixes are
/// being written again and again.
const CACHE_REWRITE_PCT: f32 = 15.0;
/// Calls needed before a written share means anything, since a cold start is nearly all writes.
const CACHE_REWRITE_MIN_CALLS: u64 = 20;

impl ContextComposition {
    /// Derives the findings from the figures already in the report.
    ///
    /// Every rule names a lever the reader actually has, and fires only when it is true. A finding
    /// that is always present is furniture, and furniture is what makes a reader stop reading.
    pub fn derive_findings(&mut self, calls: u64, truncated_instructions: &[String]) {
        let mut findings = Vec::new();
        self.window_findings(&mut findings);
        self.conversation_findings(&mut findings);
        self.tool_schema_findings(&mut findings);
        for file in truncated_instructions {
            findings.push(ContextFinding {
                level: FindingLevel::Note,
                text: format!(
                    "{file} was cut at the per-file ceiling, so the model is reading part of it. \
                     Shorten it, or move the detail somewhere the agent opens on demand."
                ),
            });
        }
        self.cache_findings(calls, &mut findings);

        // Most severe first: the reader should meet the deadline before the housekeeping.
        findings.sort_by_key(|finding| std::cmp::Reverse(finding.level));
        self.findings = findings;
    }

    fn slice_of(&self, part: ContextPart) -> Option<&ContextSlice> {
        self.slices.iter().find(|slice| slice.part == part)
    }

    /// How full the window is.
    fn window_findings(&self, findings: &mut Vec<ContextFinding>) {
        if let Some(used) = self.used_pct() {
            if used >= FULL_PCT {
                findings.push(ContextFinding {
                    level: FindingLevel::Alert,
                    text: format!(
                        "The window is {used:.0}% full. Compaction will fire on the next long \
                         turn; `/compact` now decides what it keeps."
                    ),
                });
            } else if used >= TIGHT_PCT {
                findings.push(ContextFinding {
                    level: FindingLevel::Warn,
                    text: format!("The window is {used:.0}% full. `/compact` frees the tail."),
                });
            }
        }
    }

    /// What the conversation weighs, and its single largest result.
    fn conversation_findings(&self, findings: &mut Vec<ContextFinding>) {
        if let Some(conversation) = self.slice_of(ContextPart::Conversation) {
            let share = self.share_pct(conversation);
            if share >= CONVERSATION_HEAVY_PCT {
                findings.push(ContextFinding {
                    level: FindingLevel::Note,
                    text: format!(
                        "The conversation is {share:.0}% of what is in the window. `/compact` \
                         summarizes it; `/new` starts clean when the work has moved on."
                    ),
                });
            }
            if let Some(biggest) = conversation.detail.first() {
                let share = self.window_share(biggest.tokens);
                if share >= BIG_RESULT_PCT {
                    findings.push(ContextFinding {
                        level: FindingLevel::Note,
                        text: format!(
                            "{} is {} on its own — the largest single result. Narrow the call and \
                             it stays narrow for the rest of the session.",
                            biggest.label,
                            format_tokens(biggest.tokens)
                        ),
                    });
                }
            }
        }
    }

    /// What the tool schemas cost every request before anyone types.
    fn tool_schema_findings(&self, findings: &mut Vec<ContextFinding>) {
        if let Some(tools) = self.slice_of(ContextPart::ToolSchemas) {
            let fixed: u64 = self
                .slices
                .iter()
                .filter(|slice| slice.part != ContextPart::Conversation)
                .map(|slice| slice.tokens)
                .sum();
            let share =
                if fixed == 0 { 0.0 } else { tools.tokens as f64 / fixed as f64 * 100.0 } as f32;
            if share >= TOOL_BLOCK_HEAVY_PCT {
                let named = tools
                    .detail
                    .iter()
                    .take(3)
                    .map(|item| format!("`{}`", item.label))
                    .collect::<Vec<_>>()
                    .join(", ");
                let largest =
                    if named.is_empty() { String::new() } else { format!(" Largest: {named}.") };
                findings.push(ContextFinding {
                    level: FindingLevel::Note,
                    text: format!(
                        "Tool schemas are {share:.0}% of the fixed overhead — what every request \
                         pays before you type.{largest} `tools.disabled` removes the ones this \
                         project never uses."
                    ),
                });
            }
        }
    }

    /// Whether the cache is being reused, or rewritten every turn.
    fn cache_findings(&self, calls: u64, findings: &mut Vec<ContextFinding>) {
        if let Some(cache) = self.cache_hit_pct {
            if calls >= CACHE_MIN_CALLS && cache < CACHE_COLD_PCT {
                findings.push(ContextFinding {
                    level: FindingLevel::Warn,
                    text: format!(
                        "Only {cache:.0}% of input is being reused from cache, so something above \
                         the conversation is changing between turns and the whole prefix is being \
                         re-billed. `LUXE_WIRE_LOG=<path>` then `luxe wire <path>` names the byte \
                         that moved."
                    ),
                });
            }
        }

        // Deliberately independent of the hit rate rather than an `else`: the expensive failure is
        // the one where both are high, because a prefix rewritten every turn is read back too.
        if let Some(written) = self.cache_write_pct {
            if calls >= CACHE_REWRITE_MIN_CALLS && written > CACHE_REWRITE_PCT {
                findings.push(ContextFinding {
                    level: FindingLevel::Warn,
                    text: format!(
                        "{written:.0}% of input is being written to cache rather than read from \
                         it, and a write costs around twelve times a read. Either the request \
                         changes between turns — `LUXE_WIRE_LOG=<path>` then `luxe wire <path>` \
                         names the byte that moved — or the gaps between turns are outliving the \
                         five-minute cache, which `supports_long_cache_retention` fixes."
                    ),
                });
            }
        }
    }

    /// One figure's share of the whole window, or of what is used when the window is unknown.
    fn window_share(&self, tokens: u64) -> f32 {
        let denominator =
            if self.context_window > 0 { u64::from(self.context_window) } else { self.used_tokens };
        if denominator == 0 {
            return 0.0;
        }
        (tokens as f64 / denominator as f64 * 100.0) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn builder() -> CompositionBuilder {
        let mut builder = CompositionBuilder::new();
        builder.add(ContextPart::Conversation, 40_000);
        builder.add(ContextPart::ToolSchemas, 24_000);
        builder.add(ContextPart::Harness, 6_000);
        builder
    }

    /// The rule the whole module exists to hold: the rows sum to the number the provider reported,
    /// so the breakdown can never disagree with the reading beside it.
    #[test]
    fn the_parts_sum_to_the_measured_total() {
        let (slices, used, calibration) = builder().resolve(Some(20_000));
        assert_eq!(used, 20_000);
        assert!(calibration.is_measured());
        let summed: u64 = slices.iter().map(|slice| slice.tokens).sum();
        // Rounding may move a token between rows; it may never move one out of the total.
        assert!(summed.abs_diff(20_000) <= slices.len() as u64, "rows summed to {summed}");
    }

    /// A part is added more than once (harness is three separate assembled sections), and the row
    /// is their sum rather than the last one.
    #[test]
    fn repeated_parts_accumulate() {
        let mut builder = CompositionBuilder::new();
        builder.add(ContextPart::Harness, 1_000);
        builder.add(ContextPart::Harness, 3_000);
        let (slices, _, _) = builder.resolve(None);
        assert_eq!(slices.len(), 1);
        assert_eq!(slices[0].tokens, 1_000, "4,000 prose chars at four per token");
    }

    /// JSON tokenizes shorter than prose, so equal character counts are not equal token counts.
    #[test]
    fn the_tool_block_is_weighted_as_json() {
        let mut builder = CompositionBuilder::new();
        builder.add(ContextPart::ToolSchemas, 12_000);
        builder.add(ContextPart::Instructions, 12_000);
        let (slices, _, _) = builder.resolve(None);
        let tool = slices.iter().find(|slice| slice.part == ContextPart::ToolSchemas).unwrap();
        let prose = slices.iter().find(|slice| slice.part == ContextPart::Instructions).unwrap();
        assert_eq!(tool.tokens, 4_000);
        assert_eq!(prose.tokens, 3_000);
        assert!(
            tool.tokens > prose.tokens,
            "same characters, more tokens: the bias being corrected"
        );
    }

    /// Rows are ranked, because the question is "what is biggest".
    #[test]
    fn slices_are_ranked_largest_first() {
        let (slices, _, _) = builder().resolve(Some(20_000));
        let tokens: Vec<u64> = slices.iter().map(|slice| slice.tokens).collect();
        let mut sorted = tokens.clone();
        sorted.sort_unstable_by(|left, right| right.cmp(left));
        assert_eq!(tokens, sorted);
        assert_eq!(slices[0].part, ContextPart::Conversation);
    }

    /// Nothing billed yet: the report says it is estimating rather than inventing a measurement.
    #[test]
    fn an_unbilled_session_reports_an_estimate() {
        let (slices, used, calibration) = builder().resolve(None);
        assert_eq!(calibration, Calibration::Estimated);
        assert_eq!(used, slices.iter().map(|slice| slice.tokens).sum::<u64>());
        assert!(used > 0);
    }

    /// A detail can never claim more than the row it sits under.
    #[test]
    fn details_scale_with_their_part() {
        let mut builder = CompositionBuilder::new();
        builder.add(ContextPart::ToolSchemas, 12_000);
        builder.detail(
            ContextPart::ToolSchemas,
            vec![ContextDetail { label: "bash".into(), tokens: 6_000 }],
        );
        let (slices, _, _) = builder.resolve(Some(2_000));
        let tool = &slices[0];
        assert_eq!(tool.tokens, 2_000);
        assert_eq!(tool.detail[0].tokens, 1_000, "half the characters, half the row");
        assert!(tool.detail[0].tokens <= tool.tokens);
    }

    /// An empty part is not a row. A window with no skills should not print `skills 0`.
    #[test]
    fn empty_parts_are_omitted() {
        let mut builder = CompositionBuilder::new();
        builder.add(ContextPart::Harness, 4_000);
        builder.add(ContextPart::Skills, 0);
        let (slices, _, _) = builder.resolve(None);
        assert_eq!(slices.len(), 1);
        assert_eq!(slices[0].part, ContextPart::Harness);
    }

    #[test]
    fn free_space_and_shares_come_off_the_window() {
        let composition = ContextComposition {
            model: "test".into(),
            context_window: 200_000,
            used_tokens: 50_000,
            calibration: Calibration::Measured { chars_per_token: 3.6 },
            slices: vec![ContextSlice {
                part: ContextPart::Conversation,
                tokens: 25_000,
                detail: Vec::new(),
            }],
            findings: Vec::new(),
            cache_hit_pct: None,
            cache_write_pct: None,
            mcp: None,
        };
        assert_eq!(composition.free_tokens(), Some(150_000));
        assert_eq!(composition.used_pct(), Some(25.0));
        assert_eq!(composition.share_pct(&composition.slices[0]), 50.0);
    }

    /// A local model with no published window still reports its parts; it just cannot report a
    /// percentage, and must not invent one.
    #[test]
    fn an_unknown_window_reports_no_percentage() {
        let composition = ContextComposition {
            model: "local".into(),
            context_window: 0,
            used_tokens: 12_000,
            calibration: Calibration::Estimated,
            slices: Vec::new(),
            findings: Vec::new(),
            cache_hit_pct: None,
            cache_write_pct: None,
            mcp: None,
        };
        assert_eq!(composition.used_pct(), None);
        assert_eq!(composition.free_tokens(), None);
    }
}
