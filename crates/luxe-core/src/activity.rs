//! Tool activity — bounded, revisioned display state for front-ends.
//!
//! The engine projects canonical tool lifecycle events into these records. Tool output stays in
//! the session transcript; this module carries only redacted, bounded display summaries.
//!
//! [`describe_call`] turns a call's tool and arguments into the one-line label a card shows, and
//! [`tool_card_body`] into the multi-line payload the card renders under it. Both live here so the
//! live projection and session replay describe a call identically. So does [`lookup_runs`], which
//! says which calls a front-end may draw as one folded run. The directory a `bash` call starts in
//! comes from the caller, never the process, so a label depends on nothing but its inputs.

use crate::tool::{denial, guards};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

mod bash_display;
mod card_body;
mod describe;
mod partial_json;

pub use card_body::{partial_card_body, tool_card_body};
pub use describe::{describe_call, describe_partial_call, web_search_queries};

/// Lines of a card body carried to the front-end before it is cut.
///
/// Generous, because this never reaches the model — it is display state, and the cost of holding a
/// written file per call is a few kilobytes. Deep enough that expanding a card shows the whole thing
/// for anything a human would sit and read.
const CARD_BODY_MAX_LINES: usize = 600;

/// The multi-line payload a tool card renders under its header.
///
/// `text` is what the front-end has to work with and may be shorter than the call's; `total_lines`
/// counts the *whole* payload, so a truncation footer can say how much is missing rather than
/// quietly reporting what survived.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCardBody {
    /// Payload text, capped at `CARD_BODY_MAX_LINES`.
    pub text: String,
    /// Lines the untruncated payload had.
    pub total_lines: usize,
    /// Lines a collapsed card shows of it, where the payload is detail rather than the point: a
    /// described `bash` command, whose description is already on the header. `None` leaves it to
    /// the front-end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collapsed_lines: Option<usize>,
}

impl ToolCardBody {
    /// Caps `text` for display while remembering how long it really was.
    fn capped(text: &str) -> Self {
        let total_lines = text.lines().count();
        if total_lines <= CARD_BODY_MAX_LINES {
            return Self { text: text.to_string(), total_lines, collapsed_lines: None };
        }
        let kept = text.lines().take(CARD_BODY_MAX_LINES).collect::<Vec<_>>().join("\n");
        Self { text: kept, total_lines, collapsed_lines: None }
    }

    /// Whether the carried text stops short of the full payload.
    pub fn is_truncated(&self) -> bool {
        self.total_lines > self.text.lines().count()
    }
}

/// Where a call sits in the run of calls its assistant message asked for.
///
/// A model decides for itself how many calls to put in one message; the engine runs them strictly
/// in order, one at a time. What the reader is missing is the *denominator* — with only the card in
/// front of them, seven lookups arriving in half a second read as a wall that came out of nowhere.
/// Knowing a call is the third of seven turns that into a queue with an end in sight.
///
/// Live information only. Once the run is done there is no progress left to report, and a front-end
/// that keeps drawing one is describing history as though it were still happening.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolBatch {
    /// 1-based position of this call in its message.
    pub index: u32,
    /// Calls the message asked for.
    pub total: u32,
}

/// Where a call sits in a run of adjacent lookups its message asked for.
///
/// Structure rather than progress, which is what separates it from [`ToolBatch`]: it says the run
/// exists and how long it will be, so a front-end can draw the run as one line from its *first*
/// card instead of letting three land and then folding them. A fold that arrives late has to take
/// back the rows it already spent, and the transcript jumps under whatever the reader was reading.
///
/// Present only on calls that belong to a run worth folding — see [`lookup_runs`] — so a front-end
/// needs no threshold of its own. It survives replay, because which calls one message asked for is
/// a fact about history and not a claim that anything is still happening.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolRun {
    /// 1-based position of this call in its run.
    pub index: u32,
    /// Calls the run holds.
    pub total: u32,
}

impl ToolRun {
    /// Whether this call is the last one its run is waiting for.
    pub fn is_last(&self) -> bool {
        self.index >= self.total
    }
}

/// A revisioned display-safe projection of one tool call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolActivity {
    /// Session that owns this tool call.
    pub session_id: String,
    /// Turn that started the call, when available.
    pub turn_id: Option<String>,
    /// Tool-call identifier, unique only within its session.
    pub call_id: String,
    /// Monotonic revision for this `(session_id, call_id)` pair.
    pub revision: u64,
    /// Stable registered tool name.
    pub name: String,
    /// Bounded, redacted display label.
    pub label: String,
    /// Current lifecycle state.
    pub state: ToolActivityState,
    /// Start timestamp in milliseconds since Unix epoch.
    pub started_at_ms: u64,
    /// Elapsed duration when available.
    pub elapsed_ms: Option<u64>,
    /// Bounded, redacted outcome summary.
    pub summary: Option<String>,
    /// Bounded, redacted input preview.
    pub input_preview: Option<String>,
    /// Bounded, redacted output preview.
    pub output_preview: Option<String>,
    /// Multi-line payload the card renders under its header, when the tool has one.
    #[serde(default)]
    pub card_body: Option<ToolCardBody>,
    /// Where this call sits in the run its message asked for, when that run is longer than one.
    #[serde(default)]
    pub batch: Option<ToolBatch>,
    /// Where this call sits in a run of adjacent lookups a front-end may fold, when it is in one.
    #[serde(default)]
    pub run: Option<ToolRun>,
}

impl ToolActivity {
    /// Creates a running activity record for a newly started tool call.
    pub fn running(
        session_id: impl Into<String>,
        call_id: impl Into<String>,
        name: impl Into<String>,
        label: impl Into<String>,
        started_at_ms: u64,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            turn_id: None,
            call_id: call_id.into(),
            revision: 0,
            name: name.into(),
            label: label.into(),
            state: ToolActivityState::Running,
            started_at_ms,
            elapsed_ms: None,
            summary: None,
            input_preview: None,
            output_preview: None,
            card_body: None,
            batch: None,
            run: None,
        }
    }

    /// The same record for a call the model is still writing.
    ///
    /// `started_at_ms` is when the *card* opened, not when the tool did; dispatch restarts the
    /// clock, so what a finished card reports is the tool's own time rather than the minute the
    /// model spent writing the call.
    pub fn composing(
        session_id: impl Into<String>,
        call_id: impl Into<String>,
        name: impl Into<String>,
        label: impl Into<String>,
        started_at_ms: u64,
    ) -> Self {
        Self {
            state: ToolActivityState::Composing,
            ..Self::running(session_id, call_id, name, label, started_at_ms)
        }
    }

    /// Returns whether this state cannot accept further lifecycle updates.
    pub fn is_terminal(&self) -> bool {
        self.state.is_terminal()
    }

    /// Whether this call belongs in a transcript a person reads.
    ///
    /// Bookkeeping that went fine does not. Everything else does, including bookkeeping that did
    /// not — a call the reader cannot see is a call that cannot explain a stall, and an invisible
    /// retry loop is worse than a visible one.
    pub fn shows_in_transcript(&self) -> bool {
        !is_bookkeeping(&self.name)
            || !matches!(
                self.state,
                ToolActivityState::Composing
                    | ToolActivityState::Queued
                    | ToolActivityState::Running
                    | ToolActivityState::Succeeded
            )
    }
}

/// Whether a tool is luxe's own housekeeping rather than work someone asked for.
///
/// Only [`crate::tool::ACTIVATE_TOOLS_NAME`], and only because *we* tier the registry: the model
/// asks for a deferred tool's schema before it can call it, which is a fact about our request
/// budget and not a step in anyone's task. Ship every schema and the call disappears. So the card
/// for it is process narration — the thing the communication contract exists to keep out of the
/// conversation — and it lands directly in front of the tool that actually did something.
///
/// A display rule only. The durable transcript and the wire log keep every call, because what
/// reached the model is a different question from what is worth a reader's attention.
pub fn is_bookkeeping(tool: &str) -> bool {
    tool == crate::tool::ACTIVATE_TOOLS_NAME
}

/// Whether a tool's arguments are worth streaming as the model writes them.
///
/// Only `write`, whose argument *is* the file, so a card can fill in as it arrives instead of
/// sitting blank for the twenty seconds a long file takes. Streaming gives up the provider's check
/// that the arguments are valid JSON, and Claude models garble `edit`'s nested arguments
/// (`[{"oldText">…`) whichever way they travel: replaying eleven such requests, 18 of 23 retries
/// came back garbled with streaming on and 17 of 23 with it off. The difference is the shape.
/// Streamed, the garble is broken JSON nothing can read; validated, the provider stringifies the
/// array and hoists the replacement beside it, both halves whole, which `edit` reads back as the
/// edit it was. `write` is one long string, and 12 of 12 came back whole streamed.
pub fn streams_arguments(tool: &str) -> bool {
    tool == "write"
}

/// Tools whose calls may disappear into a folded run.
///
/// Lookups, and only lookups. Everything else on a card is something a reader is *supervising* —
/// which command ran, what an edit changed, which todo moved — and hiding that behind a count
/// trades the oversight the transcript exists to provide for a few rows of screen. A `read` header
/// is reference material by comparison: worth having, never worth interrupting a conversation for.
///
/// Nearly [`crate::phase_review::REVIEWER_TOOLS`] and deliberately not the same list: that one is a
/// permission policy, and folding is a display rule. They move for unrelated reasons.
const LOOKUP_TOOLS: &[&str] = &[
    "read",
    "grep",
    "glob",
    "ast_grep",
    "symbol_search",
    "module_report",
    "read_symbol",
    "lsp_diagnostics",
    "lsp_navigation",
    "impact",
];

/// Adjacent lookups one message has to ask for before the run is worth folding.
///
/// Two cards are not a wall, and a fold that replaces two rows with one has spent a reader's
/// recognition of the shape to buy nothing.
const FOLDABLE_RUN: usize = 3;

/// Whether a tool only retrieves, so a run of its calls may be drawn as one line.
pub fn is_lookup(tool: &str) -> bool {
    LOOKUP_TOOLS.contains(&tool)
}

/// Projects the runs of adjacent lookups an assistant message asked for, keyed by call id.
///
/// The whole message is known here, before any of it has been dispatched, which is what lets the
/// *first* card of a run already know it is one of seven. Calls outside a run long enough to fold
/// are absent, so a front-end folds what this returns and needs no threshold of its own.
///
/// Only another tool call breaks a run: an assistant message's prose and reasoning render ahead of
/// all of its cards, so text between two calls never separates them on screen. Housekeeping is
/// transparent for the same reason — a successful [`is_bookkeeping`] call draws nothing to break
/// the run with.
pub fn lookup_runs(content: &[crate::message::ContentBlock]) -> HashMap<String, ToolRun> {
    let mut runs = HashMap::new();
    let mut pending: Vec<&str> = Vec::new();
    for block in content {
        let crate::message::ContentBlock::ToolCall { id, name, .. } = block else {
            continue;
        };
        match name {
            name if is_lookup(name) => pending.push(id),
            name if is_bookkeeping(name) => {}
            _ => record_run(&mut pending, &mut runs),
        }
    }
    record_run(&mut pending, &mut runs);
    runs
}

/// Files a finished run under each of its calls, or drops it for being too short to fold.
fn record_run(pending: &mut Vec<&str>, runs: &mut HashMap<String, ToolRun>) {
    let ids = std::mem::take(pending);
    if ids.len() < FOLDABLE_RUN {
        return;
    }
    let total = u32::try_from(ids.len()).unwrap_or(u32::MAX);
    for (index, id) in ids.into_iter().enumerate() {
        let index = u32::try_from(index + 1).unwrap_or(u32::MAX);
        runs.insert(id.to_string(), ToolRun { index, total });
    }
}

/// Lifecycle state for a projected tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolActivityState {
    /// The model is writing this call right now — it is not dispatched, and its arguments are
    /// still arriving.
    ///
    /// **Work is happening**, so a front-end should draw this the way it draws `Running`. The
    /// distinction from `Running` is whose work it is: the model's rather than the tool's, which
    /// is why the card's clock does not start until dispatch.
    Composing,
    /// The call hasn't started because something else comes first: it's written in full and
    /// waiting its turn, or still being written while an earlier call runs.
    ///
    /// Tools are dispatched strictly one at a time, so **at most one call is ever `Running`** —
    /// and a model writes the calls of one message in order, so at most one is ever `Composing`,
    /// and never beside a `Running` one, since a call written while another runs waits behind it.
    /// Between them that is the invariant a transcript has to show: exactly one card is working.
    /// Drawing every un-finished call as running broke it in the obvious direction (two `bash`
    /// cards spinning side by side, which reads as two shells at once); drawing them all as
    /// waiting broke it in the other, and put a paused mark on a card that was visibly filling in.
    Queued,
    /// The call is still running.
    Running,
    /// The call completed successfully.
    Succeeded,
    /// The call completed with an error.
    Failed,
    /// The call was cancelled before it completed.
    Cancelled,
    /// The process or session ended before a final result arrived.
    Interrupted,
}

impl ToolActivityState {
    /// Returns whether this state is final.
    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Composing | Self::Queued | Self::Running)
    }

    /// Whether something is happening for this call right now, whoever is doing it.
    ///
    /// The model writing the call and the tool running it are both work in progress, and a card
    /// says so the same way for each. Only a call that has been written and is waiting its turn is
    /// genuinely idle.
    pub fn is_working(self) -> bool {
        matches!(self, Self::Composing | Self::Running)
    }
}

/// Cells a failure summary may occupy on a tool card's one line before it is cut.
const FAILURE_SUMMARY_LIMIT: usize = 96;

/// The one-line reason a tool call failed, taken from its own error output.
///
/// Tools lead with `Error: <what went wrong>`, so the first non-empty line minus that prefix is
/// already the sentence a reader wants; the guard failures get a shorter, plainer phrasing because
/// they are recoverable instructions rather than faults.
///
/// Lives here rather than in a front-end because a card has to read the same live and replayed. The
/// live path used this while the resume path wrote a bare "Failed", so reopening a session quietly
/// downgraded every failed card to the one word that tells a reader nothing.
pub fn failure_reason(output: &str) -> String {
    // A test run's reply opens with its command, and "how many failed" is the reason worth a slot
    if let Some(summary) = crate::compression::TestSummary::parse_rendered(output) {
        if summary.failed > 0 {
            return format!("{} failed", summary.failed);
        }
    }
    let first = output.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or_default();
    let message = first.strip_prefix("Error:").unwrap_or(first).trim();
    let plain = match message {
        // The read-before-edit and staleness guards are the two failures a reader meets most, and
        // both are about *sequence*, not breakage. Name the sequence.
        _ if message.contains(guards::READ_BEFORE_EDIT) => "edited before reading the file",
        _ if message.contains(guards::READ_BEFORE_WRITE) => "overwrote a file it hadn't read",
        _ if message.contains(guards::CHANGED_ON_DISK) => "file changed since it was read",
        // A call that never ran is not a fault in the tool. Say who stopped it
        _ if message.starts_with(denial::BY_USER) => "Rejected",
        _ if message.starts_with(denial::BY_POLICY) => "Denied by the permission rules",
        "" => "Failed",
        other => other,
    };
    let mut summary: String = plain.chars().take(FAILURE_SUMMARY_LIMIT).collect();
    if plain.chars().count() > FAILURE_SUMMARY_LIMIT {
        summary.push('\u{2026}');
    }
    summary
}

#[cfg(test)]
mod tests;
