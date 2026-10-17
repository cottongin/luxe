//! Dashboard catalog — the sessions the start screen lists, their previews, titles and ages.

/// Who produced a [`PreviewTurn`], used to style the session-preview excerpt like the loaded
/// conversation (a user turn, plain assistant prose, or a tool call).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewRole {
    /// A user message.
    User,
    /// An assistant reply.
    Assistant,
    /// A tool call the assistant made.
    Tool,
}

/// One turn of a session's preview transcript: a role and its text.
///
/// User/assistant turns carry raw markdown (rendered when the preview is drawn); tool turns
/// carry a compact one-line call summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewTurn {
    /// Who produced this turn.
    pub role: PreviewRole,
    /// The turn's text — markdown for user/assistant, a `name(args)` summary for tools.
    pub text: String,
}

/// A recent session row prepared for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSummary {
    /// Durable session identifier
    pub id: String,
    /// Human title derived from the first user message
    pub name: String,
    /// Relative modification time
    pub age: String,
    /// Bounded transcript text used by session search
    pub search_text: String,
    /// Bounded, role-tagged transcript excerpt rendered in the picker preview pane. Empty for
    /// rows that are never previewed (e.g. the dashboard's recent list).
    pub preview: Vec<PreviewTurn>,
    /// The branch of the worktree the session works in, when it has one of its own
    pub worktree: Option<String>,
}

/// One session as the CLI read it from the store, before the catalog titles and dates it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRecord {
    /// Durable session identifier
    pub id: String,
    /// When its log was last written
    pub modified: std::time::SystemTime,
    /// Its first user message, which becomes its title
    pub title: String,
    /// Bounded transcript text used by session search
    pub search_text: String,
    /// Bounded, role-tagged transcript excerpt for the picker's preview
    pub preview: Vec<PreviewTurn>,
    /// The branch of the worktree it works in, when it has one of its own
    pub worktree: Option<String>,
}

/// Reads the project's session catalog, for `/sessions` in a running session.
///
/// The CLI owns the session store, so it supplies this rather than a snapshot taken at launch,
/// which would be missing every session started since. It reads every recent session's log, so the
/// TUI runs it off the frame and fills the open picker when it answers.
pub type SessionCatalogSource = std::sync::Arc<
    dyn Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<SessionSummary>> + Send>>
        + Send
        + Sync,
>;

pub(super) fn normalize_title(title: &str) -> Option<String> {
    let normalized = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

/// How long ago, in the coarse words a list wants (`12m ago`, `2d ago`).
///
/// Public because the CLI derives the same figure for the Resume row, and two spellings of "an hour
/// ago" on one screen is the kind of detail that reads as a different product.
pub fn relative_age(modified: std::time::SystemTime, now: std::time::SystemTime) -> String {
    compute_age(modified, now)
}

pub(super) fn compute_age(modified: std::time::SystemTime, now: std::time::SystemTime) -> String {
    let seconds = now.duration_since(modified).unwrap_or(std::time::Duration::ZERO).as_secs();
    if seconds < 60 {
        "just now".to_string()
    } else if seconds < 3_600 {
        format!("{}m ago", seconds / 60)
    } else if seconds < 86_400 {
        format!("{}h ago", seconds / 3_600)
    } else if seconds < 2_592_000 {
        format!("{}d ago", seconds / 86_400)
    } else {
        format!("{}mo ago", seconds / 2_592_000)
    }
}
