//! TUI application state and message handling.
//!
//! Implements the Elm-style app loop: Msg → update → view.

use crate::dashboard::DashboardState;
use crate::editor::Editor;
use crate::guided_review::GuidedReviewWorkspace;
use crate::theme::styles::ThemeStyles;
use crate::workspace_dock::{DockList, DockListMetrics, DockState};
use crate::workspace_layout::WorkspaceLayout;
use luxe_core::config::Config;
use luxe_core::engine::Action;
use luxe_core::event::Event;
use luxe_core::guided_review::{GuidedReview, ReviewSession, ReviewSessionSaveRequestId};
use luxe_core::keymap::{KeyMap, UiAction};
use luxe_core::theme::ThemeRegistry;
use luxe_core::{ReviewLoadRequestId, ToolActivity, WorkspaceScopeId, WorkspaceSnapshot};
use std::collections::{HashMap, HashSet};
use tokio::sync::mpsc;

mod auth;
mod block_selection;
mod commands;
mod configuration;
mod construct;
mod conversation;
mod engine_send;
mod environment;
mod events;
mod images;
/// Key and mouse routing into [`Msg`].
///
/// Public because the review surface's router is worth testing directly and an integration test
/// cannot reach a `pub(crate)` one.
pub mod input;
pub(crate) mod messages;
#[cfg(test)]
pub(crate) use messages::github_post::item_count as github_post_item_count;
pub(crate) use messages::github_post::{
    is_pull_request as github_post_is_pull_request, plan as github_post_plan,
    refusal as github_post_refusal,
};
pub(crate) use messages::pull_request_opening::offered as pull_request_opening_offered;
pub(crate) use messages::review::reply_asks_for_fix;
pub(crate) use messages::review::window_sends;
pub(crate) use messages::review_draft::summary as review_draft_summary;
pub(crate) use messages::thread_answers::is_own_pull_request;
mod msg;
mod navigation;
mod opener;
mod outbox;
mod overlays;
mod pickers;
mod plan_handoff;
mod pointer;
mod queries;
mod review_edit;
mod review_mirror;
mod selection;
mod settings;
mod setup;
mod status;
mod submissions;
mod types;
mod workspace;

pub(crate) use construct::theme_notices;
pub(crate) use review_edit::ReviewEditPurpose;

pub(crate) use conversation::mcp_server;
pub use conversation::ToolRunFold;
use environment::AppEnvironment;
pub(crate) use input::chord_of;
pub use msg::*;
pub use opener::ExternalOpener;
pub use outbox::Outbox;
pub use overlays::Overlays;
pub use pointer::PointerState;
pub use review_mirror::ReviewMirror;
pub use status::{AppState, StatusModel};
pub use types::*;

/// Application state.
pub struct App {
    pub editor: Editor,
    pub convo: Vec<ConversationItem>,
    next_provisional_turn: u64,
    pub status: StatusModel,
    pub should_quit: bool,
    pub engine_tx: mpsc::Sender<Action>,
    pub current_view: ViewMode,
    pub active_guided_review_request: Option<ReviewLoadRequestId>,
    pub guided_review: Option<Box<GuidedReview>>,
    /// Route-local loading and focus state for the full-screen review workspace.
    pub(crate) guided_review_workspace: GuidedReviewWorkspace,
    /// The open review's session, what waits on the desk, and what was derived from it
    pub review: ReviewMirror,
    /// Whether the last conversation block is an assistant reply still being streamed
    streaming: bool,
    /// Whether the in-flight message streamed a reasoning block, which the commit refreshes in
    /// place rather than adding fresh.
    streamed_thinking: bool,
    /// Which of a message's several reasoning blocks the streaming item belongs to, so the commit
    /// can refresh it in place.
    streaming_thinking_block: Option<usize>,
    pub keymap: KeyMap,
    pub config: Config,
    /// Whether mouse capture is desired (`config.mouse`, flipped by `/mouse`); the event loop
    /// reconciles the terminal to match.
    pub mouse_enabled: bool,
    /// Whether permission `ask`s are auto-approved (walk-away mode); `/permissions` flips it.
    pub permission_auto_approve: bool,
    /// Per-frame mouse hit-test regions, published by the render pass and read by `route_mouse`.
    /// Behind a `RefCell` so the many `&App` render helpers can record regions without threading
    /// a `&mut` through the whole view layer.
    pub hit: std::cell::RefCell<crate::mouse::HitRegions>,
    pub dock: DockState,
    /// Largest useful dock scroll offset, written by the dock renderer each frame so wheel
    /// scrolling can clamp without duplicating the layout math (interior-mutable: render is `&App`).
    pub dock_scroll_max: std::cell::Cell<usize>,
    /// What the same renderer measured about PLAN, FILES and ELSEWHERE, in that order, so a
    /// wheel notch or a dragged thumb over one of them moves it without re-deriving the layout.
    dock_lists: std::cell::Cell<[DockListMetrics; 3]>,
    /// One file's diff, taking the conversation column. `None` when the transcript is showing.
    pub(crate) changes: Option<crate::changes::ChangesView>,
    pub files: crate::files::FilesTree,
    /// Files outside the project this session has already looked at, and the ± they carried then.
    ///
    /// Not [`ReviewMirror::seen`], which records what a review covered. Nothing can review a file
    /// outside the project, so a look is all that can happen to one — and a place whose files have
    /// all been looked at should stop taking rail. The counts are held with the path so a file
    /// touched again since comes back unread.
    pub(crate) external_seen: std::collections::HashMap<String, (u64, u64)>,
    /// Places under ELSEWHERE the reader has folded or unfolded by hand, overriding the default.
    pub(crate) external_folds: std::collections::HashMap<String, bool>,
    pub statusline_state: crate::statusline_state::StatuslineState,
    pub workspace_layout: WorkspaceLayout,
    pub theme_styles: ThemeStyles,
    /// Every theme a name can resolve to, read at start and again where one may have changed
    pub(crate) themes: environment::Themes,
    /// The commands the palette and `/` ran, most recent first, which lead the next menu
    pub recent_commands: crate::commands::RecentCommands,
    pub dashboard: Option<DashboardState>,
    /// The frame the logotype is lit at, shared by every surface that draws it
    pub banner: crate::widgets::BannerClock,
    /// The command whose argument forms the reader put away, so they stay away while it is typed
    pub slash_arguments_dismissed: Option<String>,
    /// The GitHub rows referenced this session, by token target, so a mention sends what the row
    /// is and not only its address
    pub github_rows: crate::mentions::GithubRows,
    /// Every row the last GitHub listing found, flattened and deduplicated, for the `@` picker;
    /// `None` until something asked
    pub github_listing: Option<Vec<luxe_core::github::ListItem>>,
    /// The searches `/github`'s tabs were changed to this session, by the configured search each
    /// replaces, so a tab reopens on what the reader last made it
    pub github_searches: std::collections::HashMap<String, String>,
    /// What each source row referenced this session stands for, by `kind:id`: the text the model
    /// is sent in its place
    pub mention_rows: crate::mentions::SourceRows,
    /// The MCP servers whose tool names this conversation has been sent, so a second reference is
    /// the pointer alone; cleared when compaction or a new session could have removed the first
    pub mcp_listed: std::collections::HashSet<String>,
    /// Each source's rows, by kind, once listed this session
    pub mention_listings:
        std::collections::HashMap<String, Vec<luxe_core::mention_source::MentionRow>>,
    /// The plan file plan mode wrote during the turn in flight, offered a next step when it ends
    pub plan_written: Option<String>,
    /// The id of the question modal when it is the plan's handoff, which the TUI answers itself
    pub plan_handoff: Option<luxe_core::question::QuestionRequestId>,
    /// What waits on the session this TUI last asked the engine for
    pub(crate) arrival: navigation::SessionArrival,
    pub file_provider: crate::ref_provider::FileRefProvider,
    /// The `@` picker's file list being read again, off the render thread
    pub file_listing: FileListing,
    /// The project a card's absolute path is shown relative to; `None` in a fixture
    pub project_root: Option<std::path::PathBuf>,
    /// Comments carried from reading a pull request, waiting for its checked-out review to load
    pub carried_findings: Option<crate::main_loop::FixLaunch>,
    /// The worktree `/worktree remove` asked about, waiting on the engine's fresh look at what is
    /// uncommitted in it
    pub worktree_removal_check: Option<std::path::PathBuf>,
    /// The login engine, injected by the CLI (which owns config and the network).
    pub auth_service: Option<crate::auth_wizard::AuthServiceHandle>,
    /// Channel a spawned login task reports back on. Held so the app can build an interaction for
    /// each flow it starts.
    pub auth_tx: Option<tokio::sync::mpsc::Sender<crate::auth_wizard::AuthUiMsg>>,
    /// Available models to browse in the picker (configured + models.dev), supplied by the CLI.
    pub model_catalog: Vec<crate::model_picker::ModelEntry>,
    /// Rebuild `model_catalog` when the picker opens, so config/catalog edits land mid-session,
    /// and again off the frame once a local server has said what it is serving
    pub model_sources: Option<crate::model_picker::ModelSources>,
    /// Where `/sessions` reads the project's sessions from
    pub(crate) session_catalog: navigation::SessionCatalog,
    /// Full info for the active model, used to annotate the thinking picker's supported levels.
    pub active_model_info: Option<luxe_core::provider::ModelInfo>,
    pub scroll_state: crate::scroll::ScrollState,
    /// First visible semantic conversation position from the latest render.
    pub conversation_anchor: Option<ConversationAnchor>,
    /// Asks parked for the event loop to run between frames
    pub outbox: Outbox,
    pub pointer: PointerState,
    /// What is drawn over the view, taking its input while open
    pub overlays: Overlays,
    /// The transcript search; active while its query is non-empty
    pub search_state: crate::search::SearchState,
    pub reveal: crate::reveal::Reveal,
    /// Notifier for turn-end feedback: a desktop notification through the terminal
    pub notifier: Box<dyn crate::notify::Notifier>,
    /// Whether the terminal has focus, once it has said; `None` until its first focus report, or
    /// for a terminal that never sends one
    pub terminal_focused: Option<bool>,
    /// Whether this is a headless run, which suppresses notifications
    pub headless: bool,
    /// Whether submit sends `Action::EditResend` instead of `SubmitUserMessage`
    edit_resend_mode: bool,
    /// Whether this front-end started a turn whose user message the engine hasn't committed yet.
    ///
    /// That message is already on screen, so its commit adopts the copy rather than adding one;
    /// a commit with this unset came from a client joined to the session.
    own_commit_pending: bool,
    /// When an interrupt was sent that the engine has not yet confirmed by ending the turn.
    pub(crate) interrupt_sent_at: Option<std::time::Instant>,
    /// Whether this session has already been told that nothing is selected.
    ///
    /// One line, however many times the reader presses Enter before choosing: the picker opening
    /// over the composer is the message after the first time.
    said_to_choose_a_model: bool,
    /// Transcript items recording a submission that was dropped before it ever ran.
    pub never_sent: HashSet<ConversationItemId>,
    /// Mid-turn submissions pinned above the composer, in submission order.
    pub pending_submissions: Vec<PendingSubmission>,
    /// The conversation block the copy actions act on; `None` is the last block
    pub selected_block: Option<usize>,
    pub panes: crate::workspace_layout::PaneLayout,
    /// The Context Dock's width as this frame drew it, which the rows inside it are measured to
    pub(crate) dock_rail: std::cell::Cell<u16>,
    /// Clipboard sink for writing copied text. Shared, not rebuilt per copy: on X11 the selection
    /// belongs to this instance and dies with it.
    pub clipboard_sink: std::sync::Arc<dyn crate::clipboard::ClipboardSink>,
    pub opener: ExternalOpener,
    pub workspace: Option<WorkspaceSnapshot>,
    /// Whether uncommitted work has left the working tree since this session started.
    ///
    /// Tells "nothing has happened yet" apart from "it was committed", which look identical in a
    /// snapshot and mean opposite things to a reader about to review.
    pub worktree_emptied: bool,
    /// The agent's tool calls in flight that push or open a pull request
    pub pull_request_pushes: HashSet<String>,
    /// Whether such a call finished and the session has not been linked to its pull request yet
    pub pull_request_link_pending: bool,
    /// Project-open generations a newer snapshot superseded; a late snapshot from one is dropped
    pub retired_workspace_scopes: HashSet<WorkspaceScopeId>,
    session_id: Option<String>,
    pub tool_activity: HashMap<(String, String), ToolActivity>,
    /// Structured tool presentation state keyed by the same semantic call identity.
    pub tool_items: HashMap<(String, String), ToolItem>,
    /// Full persisted output available only for explicit structured-tool copy actions.
    pub tool_copy_output: HashMap<(String, String), String>,
    /// Folded runs of tool calls the reader has opened, keyed by the call each run starts at.
    expanded_tool_groups: HashSet<ConversationItemId>,
    /// User messages the reader asked to see in full past the preview cap.
    expanded_messages: HashSet<ConversationItemId>,
    /// Whether every tool card is showing its full body and output.
    ///
    /// Transcript-wide rather than per-card: when you want to see what a tool actually returned you
    /// rarely want it for exactly one call. Per-card expansion still works and either one opens a
    /// card, so this never takes a choice away.
    pub tool_output_expanded: bool,
    pub images: crate::graphics::ImageStore,
    next_attachment_image: u64,
    /// The images each tool call produced, kept in the order its result carried them
    pub tool_images: HashMap<(String, String), Vec<crate::graphics::ImageKey>>,
    /// The transcript's laid-out rows, kept between frames.
    ///
    /// Not view state so much as the *result* of viewing: the render path owns it entirely and
    /// rebuilds only the items whose content or presentation moved. It lives here because a frame
    /// has to be able to find last frame's work. See [`crate::view::transcript`].
    pub(crate) transcript: crate::view::transcript::TranscriptLayout,
    /// Things to tell the reader once the first session has loaded — which replaces the
    /// conversation, so anything shown before it would be wiped unread.
    pub(crate) startup_notices: Vec<String>,
}

impl App {
    /// Sizes `pane` to `width`, or back to its own default with `None`.
    pub(crate) fn size_pane(&mut self, pane: crate::workspace_layout::Pane, width: Option<u16>) {
        self.panes.set(pane, width);
        if pane == crate::workspace_layout::Pane::ReviewGuide {
            self.guided_review_workspace.set_guide_width(width);
        }
    }

    /// Where the last message you sent sits in the transcript, if there is one.
    pub(crate) fn last_user_message_index(&self) -> Option<usize> {
        self.convo.iter().rposition(|item| matches!(item.block(), ConvoBlock::User { .. }))
    }

    /// Takes everything from item `index` on out of the transcript.
    ///
    /// For a re-send that replaces what it follows: the engine branches the session there, and a
    /// transcript that kept the old messages would show the reader a conversation the model no
    /// longer has.
    pub(crate) fn drop_transcript_from(&mut self, index: usize) {
        self.convo.truncate(index);
        if self.selected_block.is_some_and(|selected| selected >= index) {
            self.selected_block = None;
        }
        self.streaming = false;
        self.streaming_thinking_block = None;
    }

    /// Appends one live conversation block with a session-scoped provisional identity.
    pub fn push_convo(&mut self, block: ConvoBlock) {
        let id = ConversationItemId::Provisional {
            session_id: self.session_id.clone().unwrap_or_default(),
            turn_id: self.next_provisional_turn,
            block_index: 0,
        };
        self.next_provisional_turn = self.next_provisional_turn.saturating_add(1);
        self.convo.push(ConversationItem::new(id, block));
    }
    /// Starts a placeholder session for a newly opened review, until the desk's arrives.
    ///
    /// The engine follows every `GuidedReviewLoaded` with the session that goes with it, so this
    /// only covers the moment in between: a review on screen with no session at all would render
    /// its comments as absent. It is never written anywhere; the desk's copy replaces it.
    pub(crate) fn initialize_review_session(&mut self, review: &GuidedReview) {
        if self.review.session().is_some_and(|session| session.id == review.id) {
            return;
        }
        self.review.session = Some(ReviewSession::new(
            review.id.clone(),
            review.target.clone(),
            review.source.clone(),
            review.generated_at_ms,
        ));
        self.review.edits.clear();
        self.review.bound = false;
        self.review.provenance = None;
        self.review.commit_request = None;
        // `review.attention` and `review.flags` are deliberately not cleared here: they belong to
        // the review, not the feedback session, and the load handler assigns them just before.
    }

    /// Whether the dashboard itself owns input, rather than something drawn over it.
    ///
    /// Opening a picker or the context list does not change [`ViewMode`] — the dashboard stays
    /// behind them on purpose, so the reader can see what they are changing. That makes the view
    /// mode the wrong question to ask before handing the dashboard a key or a click: the model
    /// picker was excluded by name and the context list was not, so arrowing inside it walked the
    /// column underneath instead. One owner for the question, and every route asks it.
    pub fn dashboard_owns_input(&self) -> bool {
        self.current_view == ViewMode::Dashboard && !self.overlays.cover_the_dashboard()
    }

    /// Enter the startup dashboard with the given state.
    ///
    /// Which style it wears is a UI preference, and the live config lives here rather than with
    /// whoever assembled the dashboard's session data.
    pub fn enter_dashboard(&mut self, state: DashboardState) {
        self.current_view = ViewMode::Dashboard;
        self.dashboard = Some(state.with_style(self.config.dashboard_style()));
        self.editor.cancel();
    }

    /// Handles a message from input, the engine, or the terminal loop.
    pub fn update(&mut self, msg: Msg) {
        messages::reduce(self, msg);
    }

    /// What the workspace snapshot says changed in the project, as the FILES tree reads it.
    pub(crate) fn project_changes(&self) -> Vec<crate::workspace_dock::DockChangeItem> {
        messages::files::changed(self)
    }

    /// Route a key: resolve via KeyMap first, then fall through to submit/editor.
    ///
    /// If the chord resolves to a known UiAction, dispatch it. Otherwise, Enter (no Shift)
    /// submits the current buffer — slash commands are dispatched locally, everything else
    /// is echoed and sent to the engine as a user message. Shift+Enter inserts a newline.
    fn handle_key(&mut self, key: crossterm::event::KeyEvent) {
        input::handle_key(self, key);
    }

    fn submit_prompt(&mut self) {
        input::submit_prompt(self);
    }

    fn sync_completion_flyouts(&mut self) {
        input::sync_completion_flyouts(self);
    }

    /// Opens, narrows or closes the `/` and `@` flyouts for the start screen's prompt, after the
    /// dashboard has edited it.
    pub fn sync_dashboard_flyouts(&mut self) {
        if self.current_view == ViewMode::Dashboard {
            input::sync_completion_flyouts(self);
        }
    }

    /// Whether the composer's `/` or `@` flyout is open.
    pub fn flyout_open(&self) -> bool {
        self.overlays.slash_flyout.is_some() || self.overlays.mention.is_some()
    }

    /// Dispatch a resolved UiAction. Returns `true` if the action was consumed (handled),
    /// `false` if it was unresolved — callers should fall through to the editor/submit path.
    fn dispatch_action(&mut self, action: UiAction) -> bool {
        commands::dispatch_action(self, action)
    }

    /// Takes a workspace snapshot from the engine, unless a newer one has already been taken.
    fn apply_workspace_snapshot(&mut self, snapshot: WorkspaceSnapshot) {
        workspace::apply_snapshot(self, snapshot);
    }

    fn apply_tool_activity(&mut self, activity: ToolActivity) {
        workspace::apply_tool_activity(self, activity);
    }
}

/// Lines of a user message the transcript shows before collapsing the rest behind a summary.
///
/// A submitted message carries its pasted payload verbatim, so a stack trace or a whole file can
/// arrive as one transcript item. Previewing keeps the conversation readable; the full text is
/// always one expand away, and always what the clipboard and the model receive.
pub const USER_MESSAGE_PREVIEW_LINES: usize = 12;

/// Whether a user message is short enough to render without a preview cap.
pub(super) fn user_message_fits_preview(text: &str) -> bool {
    text.lines().count() <= USER_MESSAGE_PREVIEW_LINES + 1
}

#[cfg(test)]
mod tests;
