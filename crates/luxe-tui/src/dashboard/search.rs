//! Dashboard search — the rows a draft matches, and whether a draft reads as a prompt.

use super::{SEARCH_CHARS, SEARCH_WORDS};
use crate::commands::Command;

/// One line of the list under the prompt.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Entry {
    /// No provider yet: what everything else needs
    SetUp,
    Resume,
    NewSession,
    /// The finder over every session
    Sessions,
    /// The working tree's changes, into Guided Review
    Review,
    Configuration,
    Quit,
    /// A section title the cursor steps over. Args: `(label, note)`.
    Heading(&'static str, Option<String>),
    /// Air between sections
    Blank,
    /// A recent session, by its index into [`DashboardState::recent`]
    Recent(usize),
    /// Send the draft to a new session
    Ask,
    /// Run the slash command the draft spells. Args: `(command)`.
    Slash(Command),
    /// A session whose title matches, by index into [`DashboardState::sessions`], with the
    /// characters that matched
    Session(usize, Vec<usize>),
    /// Search every transcript for the draft
    Grep,
    /// A command whose name matches, by index into the searchable commands, with what matched
    Command(usize, Vec<usize>),
    /// The instruction files, skills and MCP servers a turn reads
    ProjectContext,
}

impl Entry {
    pub(super) fn selectable(&self) -> bool {
        !matches!(self, Self::Heading(..) | Self::Blank)
    }
}

/// Whether a draft is asking about what a turn reads: its instruction files, skills, MCP servers.
///
/// The list that answers it is not a command, so it has no entry in the registry to be ranked by;
/// these are the words a person would reach for.
pub(super) fn mentions_project_context(query: &str) -> bool {
    let query = query.trim().to_lowercase();
    query.len() >= 3
        && ["project context", "agents.md", "skills", "mcp servers", "instructions"].iter().any(
            |words| {
                words.starts_with(&query) || words.split(' ').any(|word| word.starts_with(&query))
            },
        )
}

/// Whether `draft` has stopped being a search: more than a line, more than a few words, long, or
/// carrying a mention.
///
/// A command is never a prompt, however many arguments it carries. A mention always makes one: it
/// is context for something to ask, and nobody searches session titles for `@[file:src/app.rs]`.
pub(super) fn reads_as_prompt(draft: &str) -> bool {
    let draft = draft.trim();
    !draft.starts_with('/')
        && (draft.contains('\n')
            || draft.contains("@[")
            || draft.split_whitespace().count() > SEARCH_WORDS
            || draft.chars().count() > SEARCH_CHARS)
}

/// Indexes into `entries` the cursor may rest on.
pub(super) fn selectable_in(entries: &[Entry]) -> Vec<usize> {
    (0..entries.len()).filter(|index| entries[*index].selectable()).collect()
}
