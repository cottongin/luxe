//! Compaction — the dock while a summary is written, and its reading once one stands.
//!
//! A summary is one provider call that can take a minute, on request or mid-turn. Without a state
//! of its own the dock showed nothing for `/compact` and the turn's label for an automatic one, and
//! the context reading kept the full figure until the next provider call reported a new one.

use super::super::{App, AppState, ConvoBlock};

/// The engine began writing a summary.
pub(super) fn started(app: &mut App) {
    app.status.compacting_mid_turn = matches!(app.status.state, AppState::Streaming);
    app.status.state = AppState::Compacting;
}

/// The summary stands: the dock reads what it left, and the transcript says what happened.
pub(super) fn done(app: &mut App, tokens_before: u64, tokens_after: u64) {
    use luxe_core::usage::format_tokens;
    ended(app);
    if app.status.context_window > 0 {
        let used = tokens_after as f32 / app.status.context_window as f32 * 100.0;
        app.status.context_pct = used.min(100.0);
    }
    // The summary may not have kept a server's tool names, so the next reference sends them
    app.mcp_listed.clear();
    app.push_convo(ConvoBlock::System(format!(
        "Compacted the conversation — {} → {} of context.",
        format_tokens(tokens_before),
        format_tokens(tokens_after)
    )));
}

/// The compaction under way is over, with or without a summary; the turn it interrupted, if any,
/// carries on.
pub(super) fn ended(app: &mut App) {
    if !matches!(app.status.state, AppState::Compacting) {
        return;
    }
    app.status.state =
        if app.status.compacting_mid_turn { AppState::Streaming } else { AppState::Idle };
}
