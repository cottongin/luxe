//! Overlays — the surfaces drawn over a view, which take its input while they are open.
//!
//! Which one answers a key, a paste or a click is decided by ladders that walk these in different
//! orders (`input::modal`, `input::picker`, `input::flyout` for keys, `modal_took_paste` for a
//! paste, `main_loop::mouse` for the pointer), and they do not agree in every corner;
//! `main_loop::overlay_matrix` pins what each does for every pair. The lists below are the ones
//! asked from more than one place, named for what they answer rather than unified, because each
//! answers a different question.

/// Every overlay that can be open, each `None` (or `false`) while it is not.
#[derive(Default)]
pub struct Overlays {
    /// A right-click's menu
    pub context_menu: Option<crate::context_menu::ContextMenu>,
    /// The command palette
    pub palette: Option<crate::palette::PaletteState>,
    /// The `/` flyout
    pub slash_flyout: Option<crate::slash_flyout::SlashFlyoutState>,
    /// The `@` picker
    pub mention: Option<crate::mention_flyout::MentionFlyoutState>,
    /// A tool call waiting for the reader's approval
    pub permission: Option<crate::permission_prompt::PermissionPromptState>,
    /// The agent's question, from the `question` tool
    pub question: Option<crate::question_prompt::QuestionPromptState>,
    /// What `/stack` would do to the stack's branches, open until pushed or closed
    pub restack: Option<crate::restack_panel::RestackPanel>,
    /// What `/github` lists, open until closed or a row is acted on
    pub github: Option<crate::github_panel::GithubPanel>,
    /// `/pr`: the branch's pull request in detail, open until closed
    pub pull_request_card: Option<crate::github_detail::PullRequestCard>,
    /// The session picker, in its own view
    pub session_picker: Option<crate::session_picker::SessionPickerState>,
    /// The model picker
    pub model_picker: Option<crate::model_picker::ModelPickerState>,
    /// The thinking-level picker
    pub thinking_picker: Option<crate::thinking_picker::ThinkingPickerState>,
    /// The theme picker
    pub theme_picker: Option<crate::theme_picker::ThemePickerState>,
    /// The login wizard
    pub auth_wizard: Option<crate::auth_wizard::AuthWizardState>,
    /// The Configuration surface, in its own view
    pub configuration: Option<crate::configuration::ConfigurationState>,
    /// The context-file list, open over the dashboard
    pub context_files: Option<crate::context_files::ContextFilesState>,
    /// The image filling the viewport, when the reader opened one
    pub image_viewer: Option<crate::graphics::ImageKey>,
    /// The chat's key card, open over the conversation
    pub keys_card: bool,
}

impl Overlays {
    /// Whether something is drawn over the conversation, so its own keys and mouse wait.
    pub fn covers_chat(&self) -> bool {
        self.image_viewer.is_some()
            || self.keys_card
            || self.palette.is_some()
            || self.model_picker.is_some()
            || self.thinking_picker.is_some()
            || self.theme_picker.is_some()
            || self.auth_wizard.is_some()
            || self.permission.is_some()
            || self.question.is_some()
            || self.slash_flyout.is_some()
            || self.mention.is_some()
            || self.github.is_some()
            || self.pull_request_card.is_some()
    }

    /// Whether something the start screen opens is drawn over it. Narrower than
    /// [`Self::covers_chat`]: these are the only overlays the dashboard opens.
    pub fn cover_the_dashboard(&self) -> bool {
        self.model_picker.is_some()
            || self.thinking_picker.is_some()
            || self.context_files.is_some()
            || self.theme_picker.is_some()
    }

    /// Whether anything is open over the session picker, which then gets the keys first. Only a
    /// picker with nothing over it owns every key, so none reaches the composer behind it.
    pub fn cover_the_session_picker(&self) -> bool {
        self.covers_chat()
            || self.cover_the_dashboard()
            || self.context_menu.is_some()
            || self.restack.is_some()
            || self.configuration.is_some()
    }

    /// Whether an overlay takes Chat's keys before the conversation's own search, Esc and page
    /// keys do. Narrower than [`Self::covers_chat`]: the modal surfaces outside this list get the
    /// key later, from the app's own ladder, after the conversation has had first refusal.
    pub fn take_chat_keys(&self) -> bool {
        self.palette.is_some()
            || self.slash_flyout.is_some()
            || self.mention.is_some()
            || self.model_picker.is_some()
            || self.thinking_picker.is_some()
            || self.theme_picker.is_some()
    }
}
