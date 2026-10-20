//! Compaction — the dock while a summary is written, and its reading once one stands.
//!
//! A summary is one provider call that can take a minute, on request or mid-turn. Without a state
//! of its own the dock showed nothing for `/compact` and the turn's label for an automatic one, and
//! the context reading kept the full figure until the next provider call reported a new one.
//!
//! The transcript says so as it starts, not only once it's done: the row above the composer shows
//! no label, and the statusline that does is hidden while the dock is open, so a minute of summary
//! otherwise looks like a turn that stalled. The line is settled in place when the summary ends.

use super::super::{App, AppState, ConvoBlock};
use luxe_core::usage::format_tokens;

/// The engine began writing a summary of `tokens_before` of context.
pub(super) fn started(app: &mut App, tokens_before: u64) {
    app.status.compacting_mid_turn = matches!(app.status.state, AppState::Streaming);
    app.status.state = AppState::Compacting;
    let note =
        format!("Compacting the conversation — {} of context…", format_tokens(tokens_before));
    app.push_convo(ConvoBlock::System(note));
    app.status.compaction_note = app.convo.last().map(|item| item.id.clone());
}

/// The summary stands: the dock reads what it left, and the transcript says what happened.
pub(super) fn done(app: &mut App, tokens_before: u64, tokens_after: u64) {
    ended(app);
    if app.status.context_window > 0 {
        let used = tokens_after as f32 / app.status.context_window as f32 * 100.0;
        app.status.context_pct = used.min(100.0);
    }
    // The summary may not have kept a server's tool names, so the next reference sends them
    app.mcp_listed.clear();
    let before = format_tokens(tokens_before);
    let after = format_tokens(tokens_after);
    settle_note(app, format!("Compacted the conversation — {before} → {after} of context."));
}

/// The summary was cancelled or failed; a failure's reason follows as a notice of its own.
pub(super) fn stopped(app: &mut App) {
    ended(app);
    settle_note(app, "Compaction stopped.".to_string());
}

/// The compaction under way is over, with or without a summary; the turn it interrupted, if any,
/// carries on.
fn ended(app: &mut App) {
    if !matches!(app.status.state, AppState::Compacting) {
        return;
    }
    app.status.state =
        if app.status.compacting_mid_turn { AppState::Streaming } else { AppState::Idle };
}

/// Rewrites the line `started` left with how it ended, or adds one when it's gone (a session
/// switched mid-summary, or a host that never said it started).
fn settle_note(app: &mut App, text: String) {
    let note = app.status.compaction_note.take();
    let item = note.and_then(|id| app.convo.iter_mut().rev().find(|item| item.id == id));
    match item {
        Some(item) => item.set_block(ConvoBlock::System(text)),
        None => app.push_convo(ConvoBlock::System(text)),
    }
}
