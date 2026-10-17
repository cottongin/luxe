//! Named UI actions and their stable namespaced ids (`tui.app.open_palette`, ...).
//!
//! Each action is declared once, in the table below, and the enum, its ids, labels, lookup and
//! [`UiAction::ALL`] are generated from it. An id is a key in someone's `keybindings.json`, so it
//! never changes; a retired one goes in [`ALIASES`] instead. The table's order matters too: a chord
//! bound to two actions resolves to the one declared first.

/// Declares every action as `Variant => ("id", "Label")`.
macro_rules! ui_actions {
    ($($variant:ident => ($id:literal, $label:literal),)*) => {
        /// Named UI actions. One variant per user-visible command.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum UiAction {
            $($variant,)*
        }

        impl UiAction {
            /// Every action, in declaration order.
            pub const ALL: &'static [Self] = &[$(Self::$variant,)*];

            /// Human-readable label for palette/help display.
            pub fn label(&self) -> &'static str {
                match self {
                    $(Self::$variant => $label,)*
                }
            }

            /// The action id string (for config serialization).
            pub fn id(&self) -> &'static str {
                match self {
                    $(Self::$variant => $id,)*
                }
            }

            /// Resolve an id string to a UiAction (for config loading), retired ids included.
            pub fn from_id(id: &str) -> Option<Self> {
                match id {
                    $($id => Some(Self::$variant),)*
                    _ => ALIASES.iter().find(|(alias, _)| *alias == id).map(|(_, action)| *action),
                }
            }
        }
    };
}

ui_actions! {
    // Editor
    Submit => ("tui.editor.submit", "Submit"),
    Cancel => ("tui.editor.cancel", "Cancel"),
    NewLine => ("tui.editor.new_line", "New line"),
    PasteClipboard => ("tui.editor.paste", "Paste from clipboard"),
    QueueMessage => ("tui.editor.queue_message", "Queue message for the next turn"),
    DequeueMessages => ("tui.editor.dequeue_message", "Restore waiting messages"),
    SelectAll => ("tui.editor.select_all", "Select the whole draft"),

    // App
    Quit => ("tui.app.quit", "Quit"),
    OpenPalette => ("tui.app.open_palette", "Open command palette"),
    OpenSlashMenu => ("tui.app.open_slash_menu", "Open slash menu"),
    OpenAtMenu => ("tui.app.open_at_menu", "Open @-mention picker"),
    ToggleHelp => ("tui.app.toggle_help", "Help"),
    Reload => ("tui.app.reload", "Reload config"),
    CycleMode => ("tui.app.cycle_mode", "Switch mode"),

    // Conversation
    ScrollUp => ("tui.convo.scroll_up", "Scroll up"),
    ScrollDown => ("tui.convo.scroll_down", "Scroll down"),
    PageUp => ("tui.convo.page_up", "Page up"),
    PageDown => ("tui.convo.page_down", "Page down"),
    SearchConvo => ("tui.convo.search", "Search conversation"),
    CopyMessage => ("tui.convo.copy_message", "Copy message"),
    CopyCode => ("tui.convo.copy_code", "Copy code block"),
    CopySelection => ("tui.convo.copy_selection", "Copy selection"),
    SelectBlockUp => ("tui.convo.select_block_up", "Select block up"),
    SelectBlockDown => ("tui.convo.select_block_down", "Select block down"),
    ToggleToolOutput => ("tui.convo.toggle_tool_output", "Full tool output"),
    OpenImage => ("tui.convo.open_image", "View image"),
    EditLastMessage => ("tui.convo.edit_last", "Edit last message"),
    Retry => ("tui.convo.retry", "Ask again"),
    Regenerate => ("tui.convo.regenerate", "Replace the last answer"),
    EditResend => ("tui.convo.edit_resend", "Edit and resend"),

    // Workspace
    ToggleDock => ("tui.workspace.toggle_dock", "Toggle Context Dock"),
    ShowDock => ("tui.workspace.show_dock", "Show Context Dock"),
    HideDock => ("tui.workspace.hide_dock", "Hide Context Dock"),
    ResetDockAuto => ("tui.workspace.reset_dock_auto", "Reset dock to automatic behavior"),
    ToggleDockPosition => ("tui.workspace.toggle_dock_position", "Swap the sidebar's side"),
    FocusFiles => ("tui.workspace.focus_files", "Browse files by keyboard"),
    ToggleStatusline => ("tui.statusline.toggle", "Toggle statusline"),
    ShowStatusline => ("tui.statusline.show", "Show statusline"),
    HideStatusline => ("tui.statusline.hide", "Hide statusline"),
    ResetStatuslineAuto => ("tui.statusline.reset_auto", "Reset statusline to automatic behavior"),
    ToggleMouse => ("tui.app.toggle_mouse", "Toggle mouse support"),

    // Changed files
    ToggleChanges => ("tui.preview.toggle", "Preview"),
    ChangesFileNext => ("tui.preview.file_next", "Next changed file"),
    ChangesFilePrev => ("tui.preview.file_prev", "Previous changed file"),
    ChangesToggleWordWrap => ("tui.preview.toggle_word_wrap", "Wrap long lines"),
    ChangesOpenInEditor => ("tui.preview.open_in_editor", "Open file in editor"),
    ChangesToggleView => ("tui.preview.toggle_view", "File or diff"),
    AskAboutSelection => ("tui.preview.ask_about_selection", "Ask about picked lines"),

    // Guided Review workspace
    ReviewOpen => ("tui.review.open", "Review changes"),
    ReviewOpenSelection => ("tui.review.open_selection", "Open what the cursor is on"),
    ReviewExit => ("tui.review.exit", "Exit review"),
    ReviewReturnChat => ("tui.review.return_chat", "Leave review"),
    ReviewAddFinding => ("tui.review.add_finding", "Comment on this line"),
    ReviewAskQuestion => ("tui.review.ask", "Ask the agent"),
    ReviewOpenInBrowser => ("tui.review.open_in_browser", "Open on GitHub"),
    ReviewOpenInEditor => ("tui.review.open_in_editor", "Open file in editor"),
    ReviewSend => ("tui.review.send", "Send comments"),
    ReviewEditFinding => ("tui.review.edit_finding", "Edit the selected comment"),
    ReviewDeleteFinding => ("tui.review.delete_finding", "Delete the selected unsent comment"),
    ReviewReanchorFinding => ("tui.review.reanchor_finding", "Reanchor the selected stale comment"),
    ReviewFindings => ("tui.review.findings", "All comments"),
    ReviewResolveFinding => ("tui.review.resolve_finding", "Resolve or reopen the selected comment"),
    ReviewFinish => ("tui.review.finish", "Finish review"),
    ReviewRefresh => ("tui.review.refresh", "Refresh review"),
    ReviewChapterNext => ("tui.review.chapter_next", "Next Guide chapter"),
    ReviewChapterPrev => ("tui.review.chapter_prev", "Previous Guide chapter"),
    ReviewFileNext => ("tui.review.file_next", "Next review file"),
    ReviewFilePrev => ("tui.review.file_prev", "Previous review file"),
    ReviewHunkNext => ("tui.review.hunk_next", "Next review hunk"),
    ReviewHunkPrev => ("tui.review.hunk_prev", "Previous review hunk"),
    ReviewLineNext => ("tui.review.line_next", "Next review line"),
    ReviewLinePrev => ("tui.review.line_prev", "Previous review line"),
    ReviewPageNext => ("tui.review.page_next", "Next review page"),
    ReviewPagePrev => ("tui.review.page_prev", "Previous review page"),
    ReviewScrollLeft => ("tui.review.scroll_left", "Scroll review diff left"),
    ReviewScrollRight => ("tui.review.scroll_right", "Scroll review diff right"),
    ReviewLineStart => ("tui.review.line_start", "Return review diff to column zero"),
    ReviewToggleNoise => ("tui.review.toggle_noise", "Collapsed churn"),
    // Both diff surfaces answer to it, so the label names the layout rather than the surface. The
    // id stays `tui.review.toggle_split`, because renaming one breaks the keybindings someone has
    // already written.
    ReviewToggleSplit => ("tui.review.toggle_split", "Side by side"),
    ReviewToggleWordWrap => ("tui.review.toggle_word_wrap", "Wrap long lines"),
    ReviewToggleGuide => ("tui.review.toggle_guide", "Review guide"),
    ReviewSource => ("tui.review.source", "Change review source"),
    ReviewHelp => ("tui.review.help", "Help"),

    // Dashboard
    DashboardNewSession => ("tui.dashboard.new_session", "New session"),
    DashboardResumeLatest => ("tui.dashboard.resume_latest", "Resume latest"),
    DashboardFindSession => ("tui.dashboard.find_session", "Find session"),
    DashboardRecentSessions => ("tui.dashboard.recent_sessions", "Recent sessions"),
    DashboardGrepSessions => ("tui.dashboard.grep_sessions", "Grep sessions"),
    DashboardReviewChanges => ("tui.dashboard.review_changes", "Review changes"),
    DashboardOpenConfig => ("tui.dashboard.open_config", "Configuration"),
    DashboardQuit => ("tui.dashboard.quit", "Quit"),
}

/// Ids an action had in older builds, still read from `keybindings.json` files that carry them.
const ALIASES: &[(&str, UiAction)] = &[
    ("tui.editor.paste_image", UiAction::PasteClipboard),
    // The preview was the changes pane
    ("tui.changes.toggle", UiAction::ToggleChanges),
    ("tui.changes.file_next", UiAction::ChangesFileNext),
    ("tui.changes.file_prev", UiAction::ChangesFilePrev),
    ("tui.changes.toggle_word_wrap", UiAction::ChangesToggleWordWrap),
    ("tui.changes.open_in_editor", UiAction::ChangesOpenInEditor),
    ("tui.changes.toggle_view", UiAction::ChangesToggleView),
    ("tui.changes.ask_about_selection", UiAction::AskAboutSelection),
];

impl UiAction {
    /// Whether this is one of Guided Review's own actions, the ones its workspace routes.
    ///
    /// By namespace, since that is how the actions are grouped and the ids never change.
    pub fn is_review(&self) -> bool {
        self.id().starts_with("tui.review.")
    }
}
