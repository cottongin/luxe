//! Dashboard — the one type core and the TUI share about the start screen.

/// Which session picker the dashboard opens.
///
/// The three differ in the entry point that opened the picker and its title; each searches the
/// same session names and text once it is up.
#[derive(Debug, Clone, PartialEq)]
pub enum PickerKind {
    /// Opened by "find a session"
    FindSession,
    /// Opened by "recent sessions"
    RecentSessions,
    /// Opened by "grep sessions"
    GrepSessions,
}
