//! App construction — building an app from its config and the machine it runs on.

use super::*;
use crate::statusline_state::StatuslineState;

impl App {
    /// Create a new app with the given engine channel, keymap, and UI config.
    pub fn new(engine_tx: mpsc::Sender<Action>, keymap: KeyMap, config: Config) -> Self {
        let repo_root = std::env::current_dir().unwrap_or_default();
        let mut app = Self::with_file_provider(
            engine_tx,
            keymap,
            config,
            crate::ref_provider::FileRefProvider::new(&repo_root),
            AppEnvironment::detect,
        );
        app.project_root = Some(repo_root);
        app
    }

    /// Creates an app without reading project state, for deterministic fixture rendering.
    #[cfg(any(test, feature = "fixture"))]
    pub fn new_fixture(engine_tx: mpsc::Sender<Action>, keymap: KeyMap, config: Config) -> Self {
        Self::with_file_provider(
            engine_tx,
            keymap,
            config,
            crate::ref_provider::FileRefProvider::empty(),
            AppEnvironment::fixed,
        )
    }

    pub(super) fn with_file_provider(
        engine_tx: mpsc::Sender<Action>,
        keymap: KeyMap,
        config: Config,
        file_provider: crate::ref_provider::FileRefProvider,
        environment: fn(&Config) -> AppEnvironment,
    ) -> Self {
        let AppEnvironment { themes, image_tier, cell_size, notifier } = environment(&config);
        let mut startup_notices = theme_notices(&config, &themes);
        startup_notices.extend(mention_notices(&config));
        let theme_name = config.resolved_theme_name(&themes);
        let resolved = config.resolved_theme(&themes);
        let theme_styles = ThemeStyles::from_resolved(&resolved, &theme_name);
        let banner = crate::widgets::BannerClock::new(config.banner_animation());
        // Mouse on by default (parity with the confirmed UX default); config opts out.
        let mouse_enabled = config.mouse.unwrap_or(true);
        let mut dock = DockState::new(config.dock.mode());
        dock.position = config.dock.position();
        // Seed the review's diff presentation from config; `w` and `Ctrl+B` move it at runtime.
        let mut guided_review_workspace = GuidedReviewWorkspace::default();
        guided_review_workspace.word_wrap = config.diff.word_wrap();
        guided_review_workspace.layout = config.diff.layout();

        Self {
            editor: Editor::new(),
            convo: Vec::new(),
            next_provisional_turn: 0,
            streamed_thinking: false,
            streaming_thinking_block: None,
            status: StatusModel::default(),
            should_quit: false,
            engine_tx,
            current_view: ViewMode::Chat,
            active_guided_review_request: None,
            guided_review: None,
            guided_review_workspace,
            review: ReviewMirror::default(),
            streaming: false,
            keymap,
            statusline_state: StatuslineState::new(config.statusline.visibility()),
            config,
            mouse_enabled,
            hit: std::cell::RefCell::default(),
            panes: crate::workspace_layout::PaneLayout::default(),
            dock_rail: std::cell::Cell::new(crate::workspace_layout::CONTEXT_DOCK_WIDTH),
            dock_scroll_max: std::cell::Cell::new(0),
            dock_lists: std::cell::Cell::new([DockListMetrics::default(); 3]),
            dock,
            changes: None,
            files: crate::files::FilesTree::default(),
            external_seen: std::collections::HashMap::new(),
            external_folds: std::collections::HashMap::new(),
            workspace_layout: WorkspaceLayout::Standard,
            theme_styles,
            themes,
            recent_commands: crate::commands::RecentCommands::default(),
            dashboard: None,
            banner,
            slash_arguments_dismissed: None,
            github_rows: Default::default(),
            mcp_listed: Default::default(),
            github_listing: None,
            github_searches: Default::default(),
            mention_rows: Default::default(),
            mention_listings: Default::default(),
            plan_written: None,
            plan_handoff: None,
            arrival: Default::default(),
            permission_auto_approve: false,
            file_provider,
            file_listing: FileListing::default(),
            project_root: None,
            carried_findings: None,
            worktree_removal_check: None,
            auth_service: None,
            auth_tx: None,
            model_catalog: Vec::new(),
            model_sources: None,
            session_catalog: Default::default(),
            active_model_info: None,
            scroll_state: crate::scroll::ScrollState::new(0, 0),
            conversation_anchor: None,
            outbox: Outbox::default(),
            pointer: PointerState::default(),
            overlays: Overlays::default(),
            search_state: crate::search::SearchState::new(),
            reveal: crate::reveal::Reveal::default(),
            selected_block: None,
            clipboard_sink: crate::clipboard::initial_sink(),
            opener: ExternalOpener::default(),
            notifier,
            terminal_focused: None,
            headless: false, // Will be set by main_loop based on CLI args.
            edit_resend_mode: false,
            own_commit_pending: false,
            interrupt_sent_at: None,
            said_to_choose_a_model: false,
            never_sent: HashSet::new(),
            pending_submissions: Vec::new(),
            workspace: None,
            worktree_emptied: false,
            pull_request_pushes: HashSet::new(),
            pull_request_link_pending: false,
            retired_workspace_scopes: HashSet::new(),
            session_id: None,
            tool_activity: HashMap::new(),
            tool_items: HashMap::new(),
            tool_copy_output: HashMap::new(),
            expanded_tool_groups: HashSet::new(),
            expanded_messages: HashSet::new(),
            tool_output_expanded: false,
            images: crate::graphics::ImageStore::new(image_tier, cell_size),
            next_attachment_image: 0,
            tool_images: HashMap::new(),
            transcript: crate::view::transcript::TranscriptLayout::default(),
            startup_notices,
        }
    }
}

/// What is wrong with the reader's theme setup, in sentences: custom files that did not load, and
/// a configured theme that names nothing.
///
/// A theme that falls back looks like a theme that is not very different, so a warning in the log
/// alone left someone tuning a file Luxe had never read.
pub(crate) fn theme_notices(config: &Config, registry: &ThemeRegistry) -> Vec<String> {
    let mut notices: Vec<String> =
        registry.problems().iter().map(|problem| format!("theme not loaded — {problem}")).collect();
    let unknown = config.theme.as_deref().filter(|name| registry.get(name).is_none());
    if unknown == Some(luxe_core::omarchy::DESKTOP_THEME) {
        notices.push(format!(
            "theme `{}` follows Omarchy's desktop theme, and none is applied here — using `{}`",
            luxe_core::omarchy::DESKTOP_THEME,
            luxe_core::config::DEFAULT_THEME
        ));
    } else if let Some(name) = unknown {
        notices.push(format!(
            "theme `{name}` is not built in and there is no {} — using `{}`",
            luxe_core::user_dirs::config_path_display(&format!("themes/{name}.json")),
            luxe_core::config::DEFAULT_THEME
        ));
    }
    notices
}

/// A notice for each mention source whose kind one of Luxe's own tabs uses, so the tab it no longer
/// gets is explained rather than missing.
pub(crate) fn mention_notices(config: &Config) -> Vec<String> {
    config
        .mentions
        .iter()
        .filter(|source| source.reserved())
        .map(|source| {
            format!(
                "mention source `{}` is not listed — `@[{}:…]` belongs to Luxe's own tab. Give it a \
                 `kind` of its own in {}",
                source.name,
                source.kind(),
                luxe_core::user_dirs::config_path_display("config.json")
            )
        })
        .collect()
}
