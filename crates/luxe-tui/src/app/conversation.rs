//! Conversation selection — selected blocks and clipboard-ready text.
//!
//! Selection remains state-only here; terminal clipboard I/O stays in the event loop so the
//! conversation reducer is deterministic.

use super::{App, ConversationItem, ConversationItemId, ConvoBlock, ToolItem};
use luxe_core::ToolActivityState;
use std::ops::Range;

pub(super) fn select_up(app: &mut App) {
    let len = app.convo.len();
    if len == 0 {
        app.selected_block = None;
        return;
    }
    app.selected_block = Some(match app.selected_block {
        Some(index) if index > 0 => index - 1,
        Some(_) | None => len - 1,
    });
}

pub(super) fn select_down(app: &mut App) {
    let len = app.convo.len();
    if len == 0 {
        app.selected_block = None;
        return;
    }
    app.selected_block = Some(match app.selected_block {
        Some(index) => (index + 1) % len,
        None => 0,
    });
}

/// Expands or collapses whatever the selected transcript item has to hide.
pub(super) fn toggle_selected_details(app: &mut App) -> bool {
    match app.selected_block {
        Some(index) => toggle_details_at(app, index),
        None => false,
    }
}

/// Expands or collapses whatever the item at `index` has to hide: a collapsed run of successful
/// tool calls, a reasoning block, a truncated user message, or a tool card's details.
///
/// Returns whether anything was actually opened or closed, so a mouse click on a block with nothing
/// behind it can stay a no-op rather than flashing state the reader did not ask for.
pub(super) fn toggle_details_at(app: &mut App, index: usize) -> bool {
    if let Some(fold) = tool_run_folds(app).into_iter().find(|fold| fold.range.contains(&index)) {
        if let Some(group) = tool_group_key(app, &fold.range).cloned() {
            if !app.expanded_tool_groups.insert(group.clone()) {
                app.expanded_tool_groups.remove(&group);
            }
            return true;
        }
    }
    let Some(item) = app.convo.get_mut(index) else {
        return false;
    };
    // Decided before anything is written, because asking for the block mutably is what marks it
    // changed — and a block that turns out to have nothing to open must not be marked at all.
    let opens = match item.block() {
        // Reasoning blocks expand/collapse with the same selection + Enter/Space as tool cards.
        ConvoBlock::Thinking { .. } => Opens::Reasoning,
        // So does a user message long enough to have been previewed.
        ConvoBlock::User { text, .. } if !super::user_message_fits_preview(text) => Opens::Message,
        ConvoBlock::Tool(_) => Opens::Card,
        _ => return false,
    };
    let id = item.id.clone();
    match opens {
        Opens::Reasoning => {
            if let ConvoBlock::Thinking { expanded, .. } = item.block_mut() {
                *expanded = !*expanded;
            }
            return true;
        }
        Opens::Message => {
            if !app.expanded_messages.insert(id.clone()) {
                app.expanded_messages.remove(&id);
            }
            return true;
        }
        Opens::Card => {}
    }
    let ConvoBlock::Tool(tool_item) = item.block_mut() else {
        return false;
    };
    tool_item.expanded = !tool_item.expanded;
    tool_item.details_toggled = true;
    let expanded = tool_item.expanded;
    // Mirror it onto the live activity, so the next update for this call doesn't close what the
    // reader just opened. A card whose identity isn't a call — a replayed transcript, a fixture —
    // has no live activity to mirror onto and simply keeps the state on its own block.
    if let ConversationItemId::Tool { session_id, call_id } = id {
        if let Some(tool_item) = app.tool_items.get_mut(&(session_id, call_id)) {
            tool_item.expanded = expanded;
            tool_item.details_toggled = true;
        }
    }
    true
}

/// What the item under the cursor has to open, once its block has been looked at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Opens {
    Reasoning,
    Message,
    Card,
}

/// A run of lookups the transcript draws as one line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolRunFold {
    /// Transcript items the one line stands for
    pub range: Range<usize>,
    /// Whether calls the run was promised are still to come
    pub arriving: bool,
}

/// Runs of lookups drawn as one line instead of a card each.
///
/// A model decides for itself how many calls go in one message, and a run of file lookups arrives
/// faster than anyone reads: seven `read` cards land in half a second and the conversation they
/// were supposed to be part of is off the top of the screen. Folding them costs the reader nothing
/// — the run is one line saying what it stands for, and a click opens it.
///
/// What makes a run is the *message*, not the screen: every member was promised by
/// [`luxe_core::lookup_runs`] before any of it was dispatched, so the first card to land is already
/// the whole run's line. Counting cards instead would let the third one fold the two above it, and
/// drop four rows out from under whatever the reader was in the middle of.
///
/// A run still short of its promised length keeps its fold, which is what lets it start with one
/// member. A run that will never reach it — a failure in the middle broke it — gives the fold back
/// unless enough members survived to be worth one.
pub(super) fn tool_run_folds(app: &App) -> Vec<ToolRunFold> {
    let mut folds: Vec<ToolRunFold> = Vec::new();
    if app.config.conversation.tool_runs() == luxe_core::config::ToolRuns::Cards {
        return folds;
    }
    let mut stretch: Option<Range<usize>> = None;

    for (index, item) in app.convo.iter().enumerate() {
        let place = folds_into_a_run(item);
        // A run's calls arrive in order, so a place that does not follow the one before it is the
        // next message's run rather than more of this one.
        let continues = stretch.as_ref().is_some_and(|open| {
            matches!((place, run_of(app, open.end - 1)), (Some(next), Some(last))
                if next.total == last.total && next.index == last.index + 1)
        });
        match (&mut stretch, continues) {
            (Some(open), true) => open.end = index + 1,
            (open, _) => {
                folds.extend(open.take().and_then(|range| keep_fold(app, range)));
                *open = place.map(|_| index..index + 1);
            }
        }
    }
    folds.extend(stretch.and_then(|range| keep_fold(app, range)));
    folds.extend(mcp_runs(app));
    folds.sort_by_key(|fold| fold.range.start);
    folds
}

/// Calls in a row to one MCP server before they fold into one line.
///
/// The same wall a message's lookups have to make: two cards are not one.
const FOLDABLE_MCP_RUN: usize = 3;

/// Runs of `mcp` calls to one server, each card next to the last, drawn as one line.
///
/// A model calls an MCP server one call per message more often than not — each call needs the
/// last one's answer — so these runs cannot be promised up front the way a message's lookups are
/// ([`luxe_core::lookup_runs`]); they are seen forming instead, and the third call folds the two
/// before it. What breaks one is anything the reader would see between two calls: prose, a
/// reasoning block, another tool, a call that failed, a call to another server. A message that
/// said nothing but whitespace draws nothing, so it does not.
///
/// A `search` or a bare overview names no server, and joins the run on either side of it: it is
/// how the agent found the tool it is about to call.
fn mcp_runs(app: &App) -> Vec<ToolRunFold> {
    struct Open<'a> {
        range: Range<usize>,
        server: Option<&'a str>,
        calls: usize,
    }
    let close = |open: Option<Open<'_>>| {
        open.filter(|open| open.calls >= FOLDABLE_MCP_RUN)
            .map(|open| ToolRunFold { range: open.range, arriving: false })
    };
    let mut folds = Vec::new();
    let mut open: Option<Open<'_>> = None;
    for (index, item) in app.convo.iter().enumerate() {
        match foldable_mcp_call(item) {
            Some(server) => {
                let joins = open.as_ref().is_some_and(|run| match (run.server, server) {
                    (Some(left), Some(right)) => left == right,
                    _ => true,
                });
                if joins {
                    let run = open.as_mut().expect("joins only an open run");
                    run.range.end = index + 1;
                    run.calls += 1;
                    run.server = run.server.or(server);
                } else {
                    folds.extend(close(open.take()));
                    open = Some(Open { range: index..index + 1, server, calls: 1 });
                }
            }
            None if draws_nothing(item) => {}
            None => folds.extend(close(open.take())),
        }
    }
    folds.extend(close(open));
    folds
}

/// The server an `mcp` card called, for a card that may disappear into a run: running or cleanly
/// done, closed, with nothing under its header. `Some(None)` is a search, which names no server.
fn foldable_mcp_call(item: &ConversationItem) -> Option<Option<&str>> {
    let ConvoBlock::Tool(tool) = item.block() else {
        return None;
    };
    let ToolItem { activity, expanded, .. } = tool.as_ref();
    let quiet_state =
        matches!(activity.state, ToolActivityState::Running | ToolActivityState::Succeeded);
    let quiet_summary =
        activity.summary.as_deref().is_none_or(|summary| summary.eq_ignore_ascii_case("completed"));
    let foldable = activity.name == "mcp"
        && quiet_state
        && quiet_summary
        && !expanded
        && activity.card_body.is_none();
    foldable.then(|| mcp_server(&activity.label))
}

/// The server in an `mcp` call's `server/tool` label, or `None` for a search or an overview.
pub(crate) fn mcp_server(label: &str) -> Option<&str> {
    let (server, tool) = label.split_once('/')?;
    let plain = |part: &str| !part.is_empty() && !part.contains(char::is_whitespace);
    (plain(server) && plain(tool)).then_some(server)
}

/// Whether an item draws nothing at all, so a run of cards on either side of it is still adjacent.
fn draws_nothing(item: &ConversationItem) -> bool {
    matches!(item.block(), ConvoBlock::Assistant(text) if text.trim().is_empty())
}

/// Whether a stretch of members earned the one line it is drawn as, and if so how it should read.
///
/// Three cards is the wall worth folding; below that the fold has to be earning its place some
/// other way, which it does only while the rest of the run is still on its way.
fn keep_fold(app: &App, range: Range<usize>) -> Option<ToolRunFold> {
    // Nothing has been appended after it, so the calls it is still owed are what comes next.
    let arriving = range.end == app.convo.len()
        && run_of(app, range.end - 1).is_some_and(|last| !last.is_last());
    (arriving || range.len() >= 3).then_some(ToolRunFold { range, arriving })
}

/// The place the item at `index` was promised in its run, for an item that has one.
fn run_of(app: &App, index: usize) -> Option<luxe_core::ToolRun> {
    match app.convo.get(index)?.block() {
        ConvoBlock::Tool(tool) => tool.activity.run,
        _ => None,
    }
}

pub(super) fn tool_group_is_expanded(app: &App, range: &Range<usize>) -> bool {
    tool_group_key(app, range).is_some_and(|key| app.expanded_tool_groups.contains(key))
}

/// What an opened run is remembered by: the call it starts at.
///
/// Not the *whole* run: a run forms while it is still working, so it grows, and a key made of
/// everything in it would change with every call that joined, snapping shut the run the reader had
/// just opened.
fn tool_group_key<'a>(app: &'a App, range: &Range<usize>) -> Option<&'a ConversationItemId> {
    app.convo.get(range.start).map(|item| &item.id)
}

/// Where a card sits in its run, for a card that can disappear into one: a lookup, still running or
/// cleanly finished, that draws nothing under its header anyway.
///
/// Which tools those are is [`luxe_core::lookup_runs`]' call, made when the message was committed —
/// a card carries a place in a run only if it was promised one. What is judged here is everything
/// that call could not know yet: how this one turned out, and whether the reader has opened it.
///
/// Judged on what the card *renders*, not on what data reached it: the live projector fills in the
/// arguments on every call and the resume path fills in the output, so requiring empty previews
/// would never fold a run. Those previews are what a card shows when the reader *opens* it — a
/// collapsed lookup is its header and nothing else, however much is behind it.
///
/// A failure never folds. It is the one card in a run that has something to say.
fn folds_into_a_run(item: &ConversationItem) -> Option<luxe_core::ToolRun> {
    let ConvoBlock::Tool(tool) = item.block() else {
        return None;
    };
    let ToolItem { activity, expanded, .. } = tool.as_ref();
    // Running counts, or the fold could only ever form after the fact — which is exactly when the
    // reader no longer needs it.
    let quiet_state =
        matches!(activity.state, ToolActivityState::Running | ToolActivityState::Succeeded);
    let quiet_summary =
        activity.summary.as_deref().is_none_or(|summary| summary.eq_ignore_ascii_case("completed"));
    activity
        .run
        .filter(|_| quiet_state && quiet_summary && !expanded && activity.card_body.is_none())
}

pub(super) fn selected_text(app: &App) -> Option<String> {
    let index = app.selected_block.unwrap_or_else(|| app.convo.len().saturating_sub(1));
    app.convo.get(index).map(|item| match (&item.id, item.block()) {
        (
            super::ConversationItemId::Tool { session_id, call_id },
            super::ConvoBlock::Tool(tool_item),
        ) => tool_item_to_plain_text(
            tool_item,
            app.tool_copy_output.get(&(session_id.clone(), call_id.clone())).map(String::as_str),
        ),
        _ => convo_block_to_plain_text(item.block()),
    })
}

pub(super) fn copy_selected(app: &mut App) -> bool {
    match selected_text(app) {
        Some(text) => {
            app.record_copy(text);
            true
        }
        None => false,
    }
}

/// The body of the first fenced code block in `text`, without its fence lines or info string.
///
/// An unclosed fence still yields what follows it, since that is code the reader can see; a fence
/// with nothing in it yields nothing.
pub(crate) fn first_code_block(text: &str) -> Option<String> {
    let (_, after) = text.split_once("```")?;
    // the rest of the opening line is the info string (`rust`, `toml title=…`), never code
    let body = after.split_once('\n').map_or("", |(_, body)| body);
    let body = body.split_once("```").map_or(body, |(code, _)| code);
    let code = body.trim_end_matches(['\n', '\r']).trim_start_matches(['\n', '\r']);
    (!code.trim().is_empty()).then(|| code.to_string())
}

/// Convert a ConvoBlock to plain text for copying.
fn convo_block_to_plain_text(block: &ConvoBlock) -> String {
    match block {
        ConvoBlock::User { text, attached, .. } => {
            // Plain text: this feeds the system clipboard, so no glyphs to paste as tofu.
            let mut out = format!("> {}\n", text);
            for item in attached {
                out.push_str(&format!("   - {}\n", item));
            }
            out
        }
        ConvoBlock::Image { width, height, .. } => format!("[image {}x{}]\n", width, height),
        ConvoBlock::Assistant(text) => {
            format!("{}\n", luxe_core::briefs::for_the_reader(text))
        }
        ConvoBlock::Thinking { text, .. } => format!("{}\n", text),
        ConvoBlock::Tool(item) => tool_item_to_plain_text(item, None),
        ConvoBlock::System(text) => format!("[system] {}\n", text),
        // Copied as plain rows: this feeds the system clipboard, where the column alignment the
        // report is set in survives nothing and the figures are the whole point.
        ConvoBlock::Context(report) => context_report_to_plain_text(report),
    }
}

/// A `/context` report as pasteable text — the figures, without the column layout.
fn context_report_to_plain_text(report: &luxe_core::context_report::ContextComposition) -> String {
    use luxe_core::context_report::format_tokens;
    let mut out = format!("[context] {}  {}", report.model, format_tokens(report.used_tokens));
    match (report.used_pct(), report.context_window) {
        (Some(pct), window) => {
            out.push_str(&format!(" of {} ({pct:.0}%)\n", format_tokens(u64::from(window))));
        }
        _ => out.push('\n'),
    }
    for slice in &report.slices {
        out.push_str(&format!("  {:<14} {}\n", slice.part.label(), format_tokens(slice.tokens)));
    }
    for finding in &report.findings {
        out.push_str(&format!("  - {}\n", finding.text));
    }
    out
}

fn tool_item_to_plain_text(item: &ToolItem, full_output: Option<&str>) -> String {
    let activity = &item.activity;
    let mut lines = vec![format!("{} {}", activity.name, activity.label)];
    if let Some(input) = &activity.input_preview {
        lines.push(format!("input: {input}"));
    }
    if let Some(output) = full_output.or(activity.output_preview.as_deref()) {
        lines.push(format!("output:\n{output}"));
    }
    if let Some(summary) = &activity.summary {
        lines.push(summary.clone());
    }
    format!("{}\n", lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::{seeded_app, FixtureScenario};
    use luxe_core::keymap::UiAction;

    /// The break this catches: the block was `split("```")[1]`, so a ```` ```rust ```` fence put
    /// `rust` on the clipboard's first line.
    #[test]
    fn the_first_code_block_is_its_body_alone() {
        let code = |text: &str| first_code_block(text);
        assert_eq!(
            code("Here:\n```rust\nfn main() {}\n```\nmore").as_deref(),
            Some("fn main() {}")
        );
        assert_eq!(code("```\na\nb\n```").as_deref(), Some("a\nb"));
        assert_eq!(code("no fence at all"), None);
        assert_eq!(code("cut off:\n```py\nprint(1)\n").as_deref(), Some("print(1)"));
        assert_eq!(code("```rust\n```"), None);
    }

    /// Copying a code block goes through the copy every other path takes, toast and all.
    #[test]
    fn copy_code_copies_the_body_and_says_so() {
        let mut app = seeded_app("dark", FixtureScenario::ChatIdle).expect("fixture");
        app.convo.clear();
        app.push_convo(ConvoBlock::Assistant("Try:\n\n```rust\nfn main() {}\n```".into()));
        app.selected_block = Some(0);
        assert!(super::super::commands::dispatch_action(&mut app, UiAction::CopyCode));
        assert_eq!(app.outbox.copy.as_deref(), Some("fn main() {}"));
        assert!(app.status.toast().is_some(), "the copy is confirmed on screen");
    }
}
