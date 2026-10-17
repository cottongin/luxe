//! App queries — what the app can say about itself, without changing anything.

use super::*;

impl App {
    /// Whether the logotype is on screen, and so whether its clock is worth a frame.
    ///
    /// Two surfaces draw it: the dashboard (with the session picker over it) and setup. Neither
    /// is a place a turn runs, so this is the only animation the app owes frames to while nothing
    /// else is happening.
    pub(super) fn showing_wordmark(&self) -> bool {
        match self.current_view {
            ViewMode::Dashboard | ViewMode::SessionPicker => self.dashboard.is_some(),
            ViewMode::Configuration => self.overlays.configuration.is_some(),
            ViewMode::Chat | ViewMode::GuidedReview => false,
        }
    }

    /// Whether the conversation holds a message of yours: where a fresh session stops being one.
    pub fn has_user_message(&self) -> bool {
        self.convo.iter().any(|item| matches!(item.block(), ConvoBlock::User { .. }))
    }

    /// Whether a turn is currently running (so a submission steers or queues rather than starting
    /// a new one).
    pub fn turn_in_flight(&self) -> bool {
        matches!(self.status.state, AppState::Streaming | AppState::Compacting)
    }

    /// What the agent is doing, in the terms herdr's sidebar uses; see [`crate::herdr`].
    ///
    /// A prompt waiting on the reader outranks the turn it interrupts: the turn has stopped until
    /// someone answers. A review whose guide is being written counts as work, so herdr's "done"
    /// marks the moment it can be read.
    pub fn agent_state(&self) -> crate::herdr::AgentState {
        use crate::herdr::AgentState;
        if let Some(prompt) = &self.overlays.permission {
            return AgentState::Blocked(format!("Approve {}", prompt.tool));
        }
        if let Some(question) = &self.overlays.question {
            return AgentState::Blocked(format!("Question: {}", question.prompt));
        }
        let reviewing = self.review.queued.as_ref().is_some_and(|queued| {
            matches!(queued.state, QueuedReviewState::Capturing | QueuedReviewState::Writing { .. })
        });
        match self.turn_in_flight() || reviewing {
            true => AgentState::Working,
            false => AgentState::Idle,
        }
    }

    /// The key that switches the mode and the mode it switches to, for a hint to name, or `None`
    /// when nothing is bound to it.
    ///
    /// Args: → `(chord, next mode)`.
    pub(crate) fn mode_switch_key(&self) -> Option<(String, String)> {
        let chord = self.keymap.chords_for(UiAction::CycleMode)?.first()?.to_string();
        let next = luxe_core::mode::Modes::default().after(&self.status.mode).name.clone();
        Some((chord, next))
    }

    /// The session on screen, once one is loaded or started.
    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref().filter(|id| !id.is_empty())
    }

    /// Whether anything on screen moves without an event arriving — the render loop's cue that a
    /// bare animation tick still earns a frame.
    ///
    /// Every animation in the TUI reads `status.spinner_frame`, which only advances while a turn is
    /// streaming or compacting, and the only other time-varying readout is that turn's elapsed
    /// timer. With neither live, two consecutive frames are identical by construction — so drawing
    /// again would re-lay out the whole transcript to paint the cells that are already there.
    ///
    /// The wordmark is the exception that moves with nothing else happening, and it only claims
    /// frames while a shine is actually crossing the letters.
    ///
    /// `status.animating` is the other: a review capture runs *after* the turn that prompted it, so
    /// nothing above is true while it works, and without it the tray's spinner would freeze into a
    /// still image of motion, which reads as a hang.
    pub fn animating(&self) -> bool {
        self.turn_in_flight()
            || self.interrupt_sent_at.is_some()
            || self.status.turn_started.is_some()
            || self.status.animating
            || self.reveal_pending()
            || (self.showing_wordmark() && self.banner.animating())
            || (self.current_view == ViewMode::GuidedReview
                && self.guided_review_workspace.waiting())
            || self.panel_waiting()
    }

    /// Whether a panel over the composer is waiting on GitHub or git, showing a spinner.
    pub fn panel_waiting(&self) -> bool {
        use crate::restack_panel::RestackPanel;
        self.overlays
            .github
            .as_ref()
            .is_some_and(|panel| panel.sections.is_none() || panel.reading() || panel.searching())
            || self.overlays.mention.as_ref().is_some_and(|mention| mention.fetching())
            || matches!(self.overlays.restack, Some(RestackPanel::Planning | RestackPanel::Pushing))
    }

    /// Whether the transcript's tail item can still grow, because a live stream is appending to it.
    ///
    /// The reveal's completion signal: an item that can no longer grow drains to a deadline instead
    /// of being paced, which is what makes every finished message a resync point.
    pub fn tail_still_streaming(&self) -> bool {
        self.streaming || self.streamed_thinking
    }

    /// Whether streamed text is paced onto the screen rather than painted as it lands.
    pub fn reveal_enabled(&self) -> bool {
        matches!(self.config.conversation.stream_reveal(), luxe_core::config::StreamReveal::Smooth)
    }

    /// Whether the reveal still owes the screen some text — the loop's cue to run its faster clock.
    pub fn reveal_pending(&self) -> bool {
        self.reveal_enabled() && self.reveal.pending()
    }

    /// The instant this frame is drawn at — the wall clock, unless a test or fixture pinned it.
    pub fn frame_now(&self) -> std::time::Instant {
        self.status.now()
    }

    /// Shows everything that has arrived at once, because something wants the user now.
    ///
    /// A prompt, a question, an error, their own keystroke, an interrupt, or a scroll away from the
    /// bottom where the animation isn't even on screen. Pacing resumes at the next message.
    pub fn reveal_catch_up(&mut self) {
        self.reveal.catch_up();
    }

    /// Whether the next submission re-sends an edited message instead of starting a fresh turn.
    pub fn edit_resend_mode(&self) -> bool {
        self.edit_resend_mode
    }

    /// What the last layout measured about one of the dock's scrolling lists.
    pub fn dock_list_metrics(&self, list: DockList) -> DockListMetrics {
        self.dock_lists.get()[list.slot()]
    }

    /// Records what this frame's layout measured about one of them.
    pub fn set_dock_list_metrics(&self, list: DockList, metrics: DockListMetrics) {
        let mut lists = self.dock_lists.get();
        lists[list.slot()] = metrics;
        self.dock_lists.set(lists);
    }

    /// Whether either text surface holds a selection that covers cells — i.e. a drag happened
    /// rather than a bare click.
    pub fn text_selection_is_active(&self) -> bool {
        selection::is_active(self)
    }

    /// Whether an overlay is drawn over the chat surface.
    ///
    /// The transcript keeps its wheel region underneath one — scrolling the thing you are reading is
    /// harmless — but it stops offering clicks and selectable text, because a press on an overlay's
    /// own padding must not grab the message hidden behind it.
    pub fn chat_overlay_open(&self) -> bool {
        self.overlays.covers_chat()
    }

    /// Whether a transcript item records a submission that was dropped before it ran.
    pub fn was_never_sent(&self, id: &ConversationItemId) -> bool {
        self.never_sent.contains(id)
    }

    /// Whether a long user message is showing its full body rather than the preview.
    pub fn message_is_expanded(&self, id: &ConversationItemId) -> bool {
        self.expanded_messages.contains(id)
    }

    /// Returns the runs of lookups the transcript draws as one line each.
    pub fn tool_run_folds(&self) -> Vec<ToolRunFold> {
        conversation::tool_run_folds(self)
    }

    /// Returns whether a folded run of lookups is open.
    pub fn tool_group_is_expanded(&self, range: &std::ops::Range<usize>) -> bool {
        conversation::tool_group_is_expanded(self, range)
    }
}
