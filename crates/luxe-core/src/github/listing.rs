//! Listing — what a search finds, and the sections a list of it is drawn in.

use super::*;

/// A pull request or an issue as a list shows it.
///
/// The title is a stranger's words; it has been through [`untrusted`], is shown, and reaches the
/// model only quoted as the title, when the reader references the row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListItem {
    /// Whether it is a pull request, rather than an issue
    pub pull_request: bool,
    /// Number within its repository
    pub number: u64,
    /// Title, filtered and bounded
    pub title: String,
    /// Web address
    pub url: String,
    /// `owner/name`
    pub repository: String,
    /// Who opened it; empty for a deleted account
    pub author: String,
    /// Whether it is a draft pull request
    pub draft: bool,
    /// When it last changed, RFC 3339
    pub updated_at: String,
    /// Open, closed or merged; an issue is never merged. Open when a frame from before this
    /// field was sent says nothing
    #[serde(default = "PullRequestState::open")]
    pub state: PullRequestState,
    /// What its second line says in `/github`: size, checks and verdict, or labels and what closes
    /// it. Absent from a frame sent before rows carried it
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facts: Option<overview::RowFacts>,
}

/// One page of a GitHub search.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchPage {
    /// Its rows, in the search's order
    pub items: Vec<ListItem>,
    /// How many the search matches in all, as GitHub counts them
    pub total: u64,
    /// The cursor the next page starts after, when there is one
    pub next: Option<String>,
}

/// One section of `/github`: its rows, or why it has none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListedSection {
    /// What its tab says
    pub name: String,
    /// What its search found
    pub items: Vec<ListItem>,
    /// Why the search failed, as a sentence
    pub error: Option<String>,
    /// How many the search matches in all, which can be more than it brought. Absent from a
    /// frame sent before sections carried it
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
    /// The cursor the next page starts after, when the search has more than it brought
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
}
