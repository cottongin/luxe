//! Render test: verify the view renders correctly without a real terminal.
//!
//! Uses ratatui's TestBackend to render into an in-memory buffer.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use luxe_core::config::{Config, ConversationAlignment};
use luxe_core::guided_review::{ReviewLine, ReviewLineKind};
use luxe_core::keymap::KeyMap;
use luxe_core::{
    DebugSessionStatus, DiagnosticsSummary, FileChangeStatus, FileChangeSummary, FileDiagnostics,
    ProjectIdentity, ReviewSummary, SessionSummary as WorkspaceSessionSummary, TodoSummary,
    TodoSummaryItem, TodoSummaryStatus, ToolActivity, ToolActivityState, WorkspaceChangesSummary,
    WorkspaceScopeId, WorkspaceSnapshot,
};
use luxe_tui::app::input::review_keys::claim;
use luxe_tui::app::{App, ConvoBlock, Msg, ToolItem, ViewMode};
use luxe_tui::clipboard::ClipboardSink;
use luxe_tui::dashboard::{DashboardState, SessionSummary};
use luxe_tui::fixture::{seeded_app, FixtureScenario};
use luxe_tui::main_loop::{route_key, route_mouse, route_mouse_at, RouteAction};
use luxe_tui::session_picker::{SessionEntry, SessionPickerState};
use luxe_tui::view::render;
use luxe_tui::workspace_dock::DockList;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::Terminal;

mod activity;
mod canvas;
mod cards;
mod changes;
mod changes_pane;
mod chat_keys;
mod clicks;
mod composer;
mod dock;
mod elsewhere;
mod files;
mod hints;
mod images;
mod layout;
mod peek_selection;
mod pickers;
mod reasoning;
mod reveal;
mod review_context;
mod review_guide;
mod routing;
mod selection;
mod steering;
mod support;
mod transcript;

use activity::*;
use canvas::*;
use cards::*;
use changes::*;
use composer::*;
use dock::*;
use elsewhere::*;
use images::*;
use routing::*;
use support::*;
