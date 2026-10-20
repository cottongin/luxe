//! Navigation reduction — dashboard outcomes into TUI surface transitions.
//!
//! Keeping these transitions together prevents overlays from inventing independent state
//! lifecycles.

use super::{App, AppState, NextSession, ViewMode};
use luxe_core::dashboard::PickerKind;
use luxe_core::engine::Action;
use luxe_core::guided_review::ReviewTarget;

/// Reduces dashboard actions into the owning TUI surface state.
pub(super) fn apply_dashboard_outcome(app: &mut App, outcome: crate::dashboard::DashOutcome) {
    use crate::dashboard::DashOutcome;

    match outcome {
        DashOutcome::Quit => {
            app.should_quit = true;
            app.editor.cancel();
        }
        DashOutcome::NewSession => start_new_session(app),
        // The row named a session, so open the one it named. Re-resolving "latest" here would open
        // whatever a loop run or a second terminal happened to write in the meantime.
        DashOutcome::Resume(session_id) => open_from_start_screen(app, session_id),
        // The session the Resume row names when there is one, for the same reason
        DashOutcome::ResumeLatest => {
            match app.dashboard.as_ref().and_then(|dashboard| dashboard.resume.clone()) {
                Some(resume) => open_from_start_screen(app, resume.id),
                None => {
                    app.leave_for_session(NextSession::ResumeLatest);
                }
            }
        }
        // Straight into Guided Review on the working tree — the same surface `/diff` opens, and the
        // answer to the changed-file count the dashboard just showed. The dashboard is dropped
        // first so leaving the review returns to the conversation, not back here.
        DashOutcome::ReviewChanges => {
            app.dashboard = None;
            app.status.state = AppState::Idle;
            super::commands::start_guided_review(app, ReviewTarget::WorkingTree);
        }
        DashOutcome::OpenPicker(picker_kind) => {
            let sessions = session_entries(app);
            app.overlays.session_picker =
                Some(crate::session_picker::SessionPickerState::for_kind(sessions, picker_kind));
            app.current_view = ViewMode::SessionPicker;
        }
        // The setup surface takes the screen rather than opening over the briefing: everything on
        // it is about getting to a model, and half a dashboard behind it says nothing useful.
        DashOutcome::OpenConfig => app.open_configuration(),
        DashOutcome::OpenContextFiles => app.open_context_files(),
        // The engine holds the mode across sessions, so the one chosen here is the one the next
        // session starts in, whichever way the reader leaves
        DashOutcome::CycleMode => super::commands::cycle_mode(app),
        // Opens *over* the dashboard and hands it back on Esc, the draft still in the prompt
        DashOutcome::OpenModelPicker => app.open_model_picker(),
        DashOutcome::Ask => ask_in_a_new_session(app),
        DashOutcome::GrepSessions(query) => {
            let sessions = session_entries(app);
            let mut picker = crate::session_picker::SessionPickerState::for_kind(
                sessions,
                PickerKind::GrepSessions,
            );
            picker.set_query(&query);
            app.editor.cancel();
            app.overlays.session_picker = Some(picker);
            app.current_view = ViewMode::SessionPicker;
        }
        DashOutcome::RunCommand(command) => run_command(app, command),
        DashOutcome::None => {}
    }
}

/// Opens `session` from the start screen: in place when it works where this run does, otherwise
/// through the CLI, which first moves the process into its worktree, or out of this one. Loaded in
/// place, a session from another worktree would run in a checkout its work is not in.
fn open_from_start_screen(app: &mut App, session: String) {
    let in_place =
        app.dashboard.as_ref().is_none_or(|dashboard| dashboard.opens_in_place(&session));
    match in_place {
        true => {
            app.leave_for_session(NextSession::Load(session));
        }
        false => app.resume_through_cli(session),
    }
}

/// Sends the draft on the start screen as the first message of a new session.
///
/// The draft stays in the composer until the session has loaded and is sent from there, so it
/// shows and steers exactly as anything typed in a session does — and with no model chosen yet,
/// the picker opens over the start screen with the draft still in hand, which is the same courtesy
/// the composer extends.
fn ask_in_a_new_session(app: &mut App) {
    if app.editor.current_input().trim().is_empty() && !app.editor.has_images() {
        return;
    }
    if !app.model_selected() {
        app.open_model_picker();
        return;
    }
    start_new_session(app);
    app.arrival.send_draft = true;
}

/// Runs a command typed or found on the start screen.
///
/// The ones that open a picker or a surface the start screen can host do it here; anything else
/// is a command about a session, so it gets one first — a new session, as `/new` would make.
fn run_command(app: &mut App, command: crate::commands::Command) {
    use crate::commands::id;
    app.editor.cancel();
    if let Some(dashboard) = app.dashboard.as_mut() {
        dashboard.sync(&app.editor);
    }
    match command.id.as_ref() {
        id::MODEL => app.open_model_picker(),
        id::THINKING => app.open_thinking_picker(),
        id::THEME => app.open_theme_picker(),
        id::CONFIG | id::LOGIN => app.open_configuration(),
        id::SETTINGS => app.open_settings(),
        id::NEW => start_new_session(app),
        // The mode outlives the session, so it is chosen here rather than in a session made for it
        id::MODE => super::commands::dispatch(app, command),
        id::SESSIONS => app.open_sessions(),
        // Leaves luxe, so a session started for it would only be left empty
        id::UPDATE => app.outbox.update = true,
        _ if command.slash_name() == Some("/quit") => {
            app.should_quit = true;
        }
        _ => {
            start_new_session(app);
            super::commands::dispatch(app, command);
        }
    }
}

/// Leave the start screen for a fresh session, keeping whatever is in the composer.
fn start_new_session(app: &mut App) {
    app.leave_for_session(NextSession::New);
}

/// The dashboard's sessions, as a picker lists them.
fn session_entries(app: &App) -> Vec<crate::session_picker::SessionEntry> {
    let sessions = app.dashboard.as_ref().map(|dashboard| dashboard.sessions.iter());
    sessions.into_iter().flatten().map(crate::session_picker::SessionEntry::from).collect()
}

impl App {
    /// Leaves whatever is on screen for another session: the conversation, emptied and at rest,
    /// with the engine asked for `next`. Its `SessionLoaded` fills the transcript back in. Answers
    /// whether the engine took the ask.
    pub(crate) fn leave_for_session(&mut self, next: NextSession) -> bool {
        self.current_view = ViewMode::Chat;
        self.dashboard = None;
        self.overlays.session_picker = None;
        self.convo.clear();
        self.streaming = false;
        self.status.state = AppState::Idle;
        let (action, doing) = match next {
            NextSession::New => {
                // The engine reuses an empty session and re-emits `SessionLoaded`, which resets
                // `session_id` and the transcript; without forgetting it here, the sidebar keeps
                // naming the session that was left until then.
                self.session_id = None;
                (Action::NewSession, "start a new session")
            }
            NextSession::Load(session_id) => (Action::LoadSession(session_id), "load a session"),
            NextSession::ResumeLatest => (Action::ResumeLatest, "resume the latest session"),
        };
        let sent = self.send_action(action, doing);
        self.arrival.owes_snapshot = sent;
        sent
    }
}

/// What waits on a session the TUI asked the engine for, settled as it arrives.
#[derive(Debug, Default)]
pub(crate) struct SessionArrival {
    /// The plan's build prompt, built from scratch in the new session
    pub(crate) build: Option<String>,
    /// Whether the composer's draft goes out once it loads: a prompt typed on the start screen
    pub(crate) send_draft: bool,
    /// Whether its closing state snapshot is still to come. It trails `SessionLoaded`, which may
    /// already have sent a turn, so its "idle" is stale
    pub(crate) owes_snapshot: bool,
}

/// Where `/sessions` reads the project's sessions from, and how many reads it has asked for.
#[derive(Default)]
pub(crate) struct SessionCatalog {
    /// Reads the catalog, supplied by the CLI; `None` where there is none to read
    pub(crate) source: Option<crate::dashboard::SessionCatalogSource>,
    /// Reads asked for so far, which numbers the next one
    pub(crate) reads: u64,
}

impl App {
    /// Opens the session picker: over the start screen, on the catalog read for it at launch, or
    /// over the conversation, reading the catalog off the frame, since one read at launch would be
    /// missing every session started since.
    pub(crate) fn open_sessions(&mut self) {
        if self.dashboard.is_some() {
            let find = crate::dashboard::DashOutcome::OpenPicker(PickerKind::FindSession);
            apply_dashboard_outcome(self, find);
            return;
        }
        if self.session_catalog.source.is_none() {
            self.status.flash_toast("There is no session list here; `luxe sessions list` has it");
            return;
        }
        if self.refuse_to_switch_mid_turn() {
            return;
        }
        self.session_catalog.reads += 1;
        let request = self.session_catalog.reads;
        let picker = crate::session_picker::SessionPickerState::reading(request);
        self.overlays.session_picker = Some(picker);
        self.current_view = ViewMode::SessionPicker;
        self.outbox.session_catalog = Some(request);
    }

    /// Lists the catalog read `request` brought back, less the session on screen, which there is
    /// no switching to. Only the picker that asked for that read takes it: one closed in the
    /// meantime wants nothing, and one opened since waits for its own.
    pub(super) fn fill_session_picker(
        &mut self,
        request: u64,
        catalog: Vec<crate::dashboard::SessionSummary>,
    ) {
        let current = self.session_id.clone();
        let waiting = self.overlays.session_picker.as_mut().filter(|p| p.waits_for(request));
        let Some(picker) = waiting else {
            return;
        };
        let others = catalog.iter().filter(|session| Some(&session.id) != current.as_ref());
        picker.fill(others.map(crate::session_picker::SessionEntry::from).collect());
    }

    /// Opens the session picked in the session picker.
    ///
    /// The start screen opens it as its rows do. Mid-session it goes through the CLI, as
    /// `/github`'s resume does: the engine and the workspace services belong to this session's
    /// checkout, and the one picked may live in a worktree of its own.
    pub(crate) fn open_picked_session(&mut self, session_id: String) {
        if self.dashboard.is_some() {
            open_from_start_screen(self, session_id);
            return;
        }
        if self.refuse_to_switch_mid_turn() {
            return;
        }
        self.overlays.session_picker = None;
        self.current_view = ViewMode::Chat;
        self.resume_through_cli(session_id);
    }

    /// Leaves for `session` through the CLI, which moves the process into its worktree when it has
    /// one. The draft goes with it, and [`Self::carry_in`] takes it up on the far side; the engine
    /// carries the mode, model and thinking level over itself.
    pub(crate) fn resume_through_cli(&mut self, session: String) {
        let (draft, images) = self.editor.draft();
        self.outbox.resume = Some(crate::main_loop::ResumeLaunch { session, draft, images });
    }

    /// Takes up the draft the reader carried out of the surface they left for this session.
    pub(crate) fn carry_in(&mut self, launch: crate::main_loop::ResumeLaunch) {
        if !launch.draft.is_empty() || !launch.images.is_empty() {
            self.editor.prepend_messages([(launch.draft, launch.images)]);
        }
    }

    /// Says so, and answers true, when a turn is running: leaving for another session stops it, and
    /// stopping a turn is the reader's call to make, with `esc`, not a side effect of a picker.
    fn refuse_to_switch_mid_turn(&mut self) -> bool {
        let running = self.turn_in_flight();
        if running {
            self.status
                .flash_toast("A turn is running; switch sessions once it ends or is stopped");
        }
        running
    }
}

/// Opens the full-screen Guided Review workspace for a source capture.
pub(super) fn enter_guided_review(app: &mut App, target: ReviewTarget) {
    app.current_view = ViewMode::GuidedReview;
    app.dock.scroll = 0;
    app.guided_review_workspace.begin_loading(target);
}

/// Closes the full-screen Guided Review workspace and returns to conversation.
pub(super) fn exit_guided_review(app: &mut App) {
    record_what_this_review_covered(app);
    // The feedback window is part of the visit, not of the review: coming back opens on the diff
    app.guided_review_workspace.close_feedback_window();
    app.current_view = ViewMode::Chat;
    // The one route from the dashboard to a conversation that loads no session: `Review changes`
    // opens the review straight from the menu, so anything held for the reader lands here.
    super::events::flush_startup_notices(app);
}

/// Records the sections the reviewer actually walked, so the next review of this target leads with
/// what is new instead of re-presenting work they already read.
///
/// The desk works out what they covered from the review it holds. The walk on screen goes first,
/// in the same queue, so a chapter finished just before leaving counts.
pub(super) fn record_what_this_review_covered(app: &mut App) {
    let Some(review_id) = app.guided_review.as_ref().map(|review| review.id.clone()) else {
        return;
    };
    super::messages::persist_review_progress(app);
    app.send_quietly(
        luxe_core::engine::Action::RecordReviewSeen { review_id },
        "record the review",
    );
}

#[cfg(test)]
mod tests;
