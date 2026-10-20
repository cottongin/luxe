//! The transcript — committed messages, a loaded session replayed, and the tool output and images
//! they carry.

use crate::app::{App, AppState, ConversationItem, ConversationItemId, ConvoBlock};
use luxe_core::config::ThinkingDisplay;
use luxe_core::event::SessionMessage;
use luxe_core::message::{ContentBlock, Message, ToolOutput};
use luxe_core::redact::redact_secrets;
use luxe_core::{describe_call, tool_card_body, ToolActivity, ToolActivityState};

const HISTORY_PREVIEW_LIMIT: usize = 320;

pub(super) fn message_committed(app: &mut App, session_id: String, entry_id: String, msg: Message) {
    if app.session_id.as_deref() != Some(session_id.as_str()) {
        tracing::debug!(%session_id, "ignoring committed message for inactive session");
        return;
    }
    super::github::note_pull_request_pushes(app, &msg.content);
    let text_blocks = text_blocks(&msg.content);
    if msg.role == luxe_core::message::Role::Tool {
        cache_tool_copy_output(app, &session_id, &msg.content);
        intern_tool_images(app, &session_id, &msg.content);
    }

    match msg.role {
        luxe_core::message::Role::User => {
            // The user's own words are never held back — a steer that lands mid-turn has to be
            // visible the moment it lands, not when the prose above it finishes arriving.
            app.reveal_catch_up();
            let words = luxe_core::briefs::readers_words(&msg.content);
            if std::mem::take(&mut app.own_commit_pending) {
                let block_index = words.first().map_or(0, |(index, _)| *index);
                if adopt_own_message(app, &session_id, &entry_id, block_index) {
                    return;
                }
            }
            for (block_index, text) in words {
                app.convo.push(ConversationItem::new(
                    persisted_id(&session_id, &entry_id, block_index),
                    user_block(text),
                ));
            }
        }
        luxe_core::message::Role::Assistant => {
            commit_assistant(app, &session_id, &entry_id, &msg.content);
        }
        luxe_core::message::Role::System => {
            for (block_index, text) in text_blocks {
                app.convo.push(ConversationItem::new(
                    persisted_id(&session_id, &entry_id, block_index),
                    ConvoBlock::System(text),
                ));
            }
        }
        _ => {}
    }
}

/// This front-end's own message, on screen since it was sent, takes the id the engine gave it.
///
/// The copy on screen keeps its chips and images, which the committed text doesn't carry.
fn adopt_own_message(app: &mut App, session_id: &str, entry_id: &str, block_index: usize) -> bool {
    let Some(index) = app.last_user_message_index() else {
        return false;
    };
    app.convo[index].id = persisted_id(session_id, entry_id, block_index);
    true
}

pub(super) fn session_loaded(app: &mut App, session_id: String, messages: Vec<SessionMessage>) {
    app.own_commit_pending = false;
    app.review.last_turn = None;
    app.mcp_listed.clear();
    app.worktree_emptied = false;
    load_session(app, session_id, messages);
    flush_startup_notices(app);
    crate::app::plan_handoff::session_loaded(app);
}

/// Put everything held for the reader into the conversation, once there is one to put it in.
///
/// Notices raised at startup — a theme that did not load, a `--no-tool` that disabled nothing, a
/// project root that is not the checkout — arrive before the reader has left the dashboard, and
/// every row on it loads a session, which replaces the transcript. Shown at the wrong moment they
/// are wiped unread, so they wait for the first moment a transcript is on screen.
pub(in crate::app) fn flush_startup_notices(app: &mut App) {
    if app.dashboard.is_some() {
        return;
    }
    for notice in std::mem::take(&mut app.startup_notices) {
        app.push_convo(ConvoBlock::System(notice));
    }
}

fn load_session(app: &mut App, session_id: String, messages: Vec<SessionMessage>) {
    let switching = app.session_id.as_deref() != Some(session_id.as_str());
    // A replayed transcript is history: whatever the reveal was pacing is gone with the old items.
    app.reveal_catch_up();
    app.session_id = Some(session_id.clone());
    app.tool_activity.clear();
    app.tool_items.clear();
    app.tool_copy_output.clear();
    app.tool_images.clear();
    app.overlays.image_viewer = None;
    // The replaced transcript's images go back to the terminal too, or a session-hopping run
    // leaves every one of them in its memory.
    app.images.clear();
    app.convo.clear();
    // The running cost is a property of the session, so a different one starts its own tally.
    if switching {
        app.status.reset_session_cost();
    }

    for entry in messages {
        replay_entry(app, &session_id, entry);
    }

    let interrupted = ToolActivityState::Interrupted;
    super::turn::mark_unfinished_tools(app, interrupted, "Interrupted after session ended");

    app.streaming = false;
    app.streamed_thinking = false;
    app.streaming_thinking_block = None;
    app.status.state = AppState::Idle;
    app.status.active_tool = None;
    app.scroll_state.to_bottom();
}

fn replay_entry(app: &mut App, session_id: &str, entry: SessionMessage) {
    let entry_id = entry.entry_id.0;
    let message = entry.message;
    let text_blocks = text_blocks(&message.content);

    match message.role {
        luxe_core::message::Role::User => {
            for (block_index, text) in luxe_core::briefs::readers_words(&message.content) {
                app.convo.push(ConversationItem::new(
                    persisted_id(session_id, &entry_id, block_index),
                    user_block(text),
                ));
            }
        }
        luxe_core::message::Role::Assistant => {
            let display = app.config.conversation.thinking();
            for (block_index, render) in assistant_render_blocks(&message.content, display) {
                app.convo.push(ConversationItem::new(
                    persisted_id(session_id, &entry_id, block_index),
                    render.into_block(display),
                ));
            }
            // Which calls fold together is a fact about the message, so history knows it as well
            // as the live path does and a resumed session folds the same runs. Progress is the
            // part replay has nothing to say about, which is why `batch` stays empty.
            let runs = luxe_core::lookup_runs(&message.content);
            let root = luxe_core::ToolContext::working_dir();
            for block in message.content {
                if let ContentBlock::ToolCall { id, name, arguments } = block {
                    // Same human label and body the live path uses, so a replayed card reads
                    // identically — a resumed session's `write` still shows its content.
                    let label = history_preview(&describe_call(&name, &arguments, &root));
                    let run = runs.get(&id).copied();
                    let mut activity = ToolActivity::running(session_id, id, name, label, 0);
                    activity.card_body = tool_card_body(&activity.name, &arguments, &root);
                    activity.run = run;
                    app.apply_tool_activity(activity);
                }
            }
        }
        luxe_core::message::Role::System => {
            for (block_index, text) in text_blocks {
                app.convo.push(ConversationItem::new(
                    persisted_id(session_id, &entry_id, block_index),
                    ConvoBlock::System(text),
                ));
            }
        }
        luxe_core::message::Role::Tool => {
            cache_tool_copy_output(app, session_id, &message.content);
            intern_tool_images(app, session_id, &message.content);
            for block in message.content {
                let ContentBlock::ToolResult { call_id, content, is_error } = block else {
                    continue;
                };
                let key = (session_id.to_string(), call_id);
                let Some(mut activity) = app.tool_activity.get(&key).cloned() else {
                    tracing::debug!(call_id = %key.1, "ignoring historical tool result without a call");
                    continue;
                };
                activity.revision += 1;
                activity.state =
                    if is_error { ToolActivityState::Failed } else { ToolActivityState::Succeeded };
                let output = tool_output_text(&content);
                // The tool's own words, exactly as the live path renders them. A replayed card that
                // said only "Failed" made the reader open the details to learn what one line would
                // have told them — and it is the interrupted calls, whose result explains what may or
                // may not have happened, where that matters most.
                activity.summary = Some(match is_error {
                    true => luxe_core::activity::failure_reason(&output),
                    false => "Completed".to_string(),
                });
                activity.output_preview = Some(history_preview(&output));
                app.apply_tool_activity(activity);
            }
        }
    }
}

fn persisted_id(session_id: &str, entry_id: &str, block_index: usize) -> ConversationItemId {
    ConversationItemId::Persisted {
        session_id: session_id.into(),
        entry_id: entry_id.into(),
        block_index,
    }
}

/// A content block from an assistant message that renders as its own conversation item, in the
/// original block order. Tool calls surface via separate tool-activity events, so they're absent.
enum AssistantRender {
    Thinking(String),
    Text(String),
}

impl AssistantRender {
    /// The concrete conversation block, seeding a reasoning block's expansion from `display`
    /// (`Collapsed` starts as a peek; anything else shows in full).
    fn into_block(self, display: ThinkingDisplay) -> ConvoBlock {
        match self {
            AssistantRender::Thinking(text) => ConvoBlock::Thinking {
                text,
                expanded: !matches!(display, ThinkingDisplay::Collapsed),
            },
            AssistantRender::Text(text) => ConvoBlock::Assistant(text),
        }
    }

    /// Whether this render item and an existing convo block are the same kind (used to line up
    /// streamed provisional blocks with their committed counterparts).
    fn same_kind(&self, block: &ConvoBlock) -> bool {
        matches!(
            (self, block),
            (AssistantRender::Thinking(_), ConvoBlock::Thinking { .. })
                | (AssistantRender::Text(_), ConvoBlock::Assistant(_))
        )
    }
}

/// The renderable items of an assistant message in block order: reasoning (unless hidden) then
/// non-empty text. Each keeps its source block index so persisted ids stay stable.
fn assistant_render_blocks(
    content: &[ContentBlock],
    display: ThinkingDisplay,
) -> Vec<(usize, AssistantRender)> {
    content
        .iter()
        .enumerate()
        .filter_map(|(block_index, block)| match block {
            // Empty reasoning is a replay token rather than content: OpenAI returns an item
            // carrying only `id`/`encrypted_content` whenever summaries are off, and it must be
            // round-tripped without ever being drawn. Rendering it would also invent a block the
            // stream never produced, which is what forces the commit to rebuild the tail.
            ContentBlock::Thinking { text, .. }
                if !text.is_empty() && !matches!(display, ThinkingDisplay::Hidden) =>
            {
                Some((block_index, AssistantRender::Thinking(text.clone())))
            }
            // Redacted reasoning has no readable text; show a placeholder so the transcript still
            // signals that the model reasoned here (and it round-trips identically on reload).
            ContentBlock::RedactedThinking { .. }
                if !matches!(display, ThinkingDisplay::Hidden) =>
            {
                Some((block_index, AssistantRender::Thinking("[redacted reasoning]".to_string())))
            }
            ContentBlock::Text(text) if !text.is_empty() => {
                Some((block_index, AssistantRender::Text(text.clone())))
            }
            _ => None,
        })
        .collect()
}

/// Reconcile a freshly committed assistant message into the transcript.
///
/// During a live turn the reasoning/answer blocks already stream in as provisional items at the
/// tail, so we reconcile those in place (promoting their ids, refreshing their text) rather than
/// re-pushing — otherwise the answer would appear twice. A message that never streamed (e.g. a
/// direct commit) is simply appended in order.
fn commit_assistant(app: &mut App, session_id: &str, entry_id: &str, content: &[ContentBlock]) {
    let display = app.config.conversation.thinking();
    let items = assistant_render_blocks(content, display);
    let streamed = app.streaming || app.streamed_thinking;
    app.streaming = false;
    app.streamed_thinking = false;
    app.streaming_thinking_block = None;

    if !streamed {
        // Nothing streamed, so there is nothing to pace: a direct commit (a compaction summary, a
        // replayed turn) is a whole message arriving at once.
        app.reveal_catch_up();
        for (block_index, render) in items {
            app.convo.push(ConversationItem::new(
                persisted_id(session_id, entry_id, block_index),
                render.into_block(display),
            ));
        }
        return;
    }

    if items.is_empty() {
        return;
    }
    let streamed_at = streamed_tail(app);
    let lines_up = streamed_at.len() == items.len()
        && items
            .iter()
            .zip(streamed_at.iter())
            .all(|((_, render), at)| render.same_kind(app.convo[*at].block()));

    if lines_up {
        for ((block_index, render), at) in items.into_iter().zip(streamed_at) {
            let item = &mut app.convo[at];
            item.id = persisted_id(session_id, entry_id, block_index);
            // Refresh content in place, preserving a manual expand/collapse a user set mid-stream.
            match (render, item.block_mut()) {
                (AssistantRender::Thinking(text), ConvoBlock::Thinking { text: existing, .. }) => {
                    *existing = text;
                }
                (AssistantRender::Text(text), ConvoBlock::Assistant(existing)) => {
                    *existing = text;
                }
                (render, block) => *block = render.into_block(display),
            }
        }
        return;
    }

    // The streamed tail didn't line up (unexpected ordering). Drop the provisional
    // reasoning/answer blocks and put the canonical items back where they were, so nothing
    // duplicates — and with the items the reveal was pacing about to be replaced wholesale, stop
    // pacing them.
    app.reveal_catch_up();
    let anchor = streamed_at.first().copied().unwrap_or(app.convo.len());
    for at in streamed_at.into_iter().rev() {
        app.convo.remove(at);
    }
    for (offset, (block_index, render)) in items.into_iter().enumerate() {
        app.convo.insert(
            anchor + offset,
            ConversationItem::new(
                persisted_id(session_id, entry_id, block_index),
                render.into_block(display),
            ),
        );
    }
}

/// Where the in-flight message's own reasoning and answer blocks sit, in transcript order.
///
/// Scanned back from the end and **stepping over tool cards**, because a call the model is still
/// writing gets its card the moment the call opens — well before the message that asked for it
/// commits. So the tail of the transcript at commit time is not `[… reasoning, answer]` but
/// `[… reasoning, answer, card]`, or with an interleaving model `[answer, card, answer, card]`.
/// Lining the committed blocks up against the last *n* items would match nothing, and a reply that
/// spoke and then called a tool would say everything twice.
///
/// Only provisional blocks count. A committed one has a persisted id and belongs to a message that
/// is already settled, which is what stops the scan from reaching into the turn before this one.
fn streamed_tail(app: &App) -> Vec<usize> {
    let mut found: Vec<usize> = Vec::new();
    for (index, item) in app.convo.iter().enumerate().rev() {
        match item.block() {
            ConvoBlock::Assistant(_) | ConvoBlock::Thinking { .. }
                if matches!(item.id, ConversationItemId::Provisional { .. }) =>
            {
                found.push(index);
            }
            // A card belongs to this same message and simply is not one of its rendered blocks.
            ConvoBlock::Tool(_) => {}
            _ => break,
        }
    }
    found.reverse();
    found
}

fn text_blocks(content: &[ContentBlock]) -> Vec<(usize, String)> {
    content
        .iter()
        .enumerate()
        .filter_map(|(block_index, block)| match block {
            ContentBlock::Text(text) => Some((block_index, text.clone())),
            _ => None,
        })
        .collect()
}

fn cache_tool_copy_output(app: &mut App, session_id: &str, content: &[ContentBlock]) {
    for block in content {
        let ContentBlock::ToolResult { call_id, content, .. } = block else {
            continue;
        };
        app.tool_copy_output
            .insert((session_id.to_string(), call_id.clone()), tool_output_text(content));
    }
}

/// Takes the images a tool result carried into the store, keyed to the call that produced them.
///
/// This is why a screenshot could be sent to the model and not shown to the reader: the pixels have
/// always arrived here, and the activity projection that feeds tool cards carries only text, so
/// they were dropped on the floor. Called from the live commit and from replay, so a resumed
/// session shows its images too.
fn intern_tool_images(app: &mut App, session_id: &str, content: &[ContentBlock]) {
    for block in content {
        let ContentBlock::ToolResult { call_id, content, .. } = block else {
            continue;
        };
        // The tool's own name is what a caption and a text-tier placeholder should say; the call is
        // already on record by the time its result commits.
        let label = app
            .tool_activity
            .get(&(session_id.to_string(), call_id.clone()))
            .map_or_else(|| "image".to_string(), |activity| activity.name.clone());
        let images: Vec<_> = content
            .iter()
            .enumerate()
            .filter_map(|(index, output)| match output {
                ToolOutput::Image { data, name, .. } => Some((index, data.clone(), name.clone())),
                ToolOutput::Text(_) => None,
            })
            .collect();
        let keys: Vec<_> = images
            .into_iter()
            .filter_map(|(index, data, name)| {
                let key = crate::graphics::ImageKey::tool_result(session_id, call_id, index);
                let origin = app.image_origin(label.clone(), name.as_deref());
                app.intern_image(&key, &data, origin).then_some(key)
            })
            .collect();
        if !keys.is_empty() {
            app.tool_images.insert((session_id.to_string(), call_id.clone()), keys);
        }
    }
}

fn tool_output_text(content: &[ToolOutput]) -> String {
    content
        .iter()
        .map(|output| match output {
            ToolOutput::Text(text) => text.as_str(),
            ToolOutput::Image { .. } => "[image output]",
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn history_preview(value: &str) -> String {
    let redacted = redact_secrets(value, &[]);
    let mut preview = redacted.chars().take(HISTORY_PREVIEW_LIMIT).collect::<String>();
    if redacted.chars().count() > HISTORY_PREVIEW_LIMIT {
        preview.push('…');
    }
    preview
}

/// A user message as the transcript shows it: the reader's words as they wrote them, and a brief
/// luxe wrote to the agent as one line saying what happened.
///
/// A question from a review is the exception, since its words *are* the reader's: it shows what
/// they asked, under where they asked it, so the answer below it is not an answer to nothing.
fn user_block(text: String) -> ConvoBlock {
    let asked = luxe_core::briefs::review_questions(&text);
    if !asked.is_empty() {
        return review_questions_block(&asked);
    }
    match luxe_core::briefs::label(&text) {
        Some(label) => ConvoBlock::System(label.to_string()),
        None => ConvoBlock::User { text, attached: Vec::new(), chips: Vec::new() },
    }
}

/// Questions from a review as the reader's own message, each headed by where it was asked.
///
/// The heading is a chip, the way a reference reads in a message the reader typed: something
/// pointed at rather than more of the sentence.
fn review_questions_block(asked: &[luxe_core::briefs::ReviewQuestion]) -> ConvoBlock {
    let mut text = String::new();
    let mut chips = Vec::new();
    for question in asked {
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        let start = text.len();
        text.push_str(&format!("Asked in review \u{b7} {}", question.location));
        chips.push(start..text.len());
        text.push('\n');
        text.push_str(&question.question);
    }
    ConvoBlock::User { text, attached: Vec::new(), chips }
}
