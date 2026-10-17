//! Opening and answering — a pull request opened, a stack of them, a fix on another's, and answers
//! to one's own threads.

use luxe_core::guided_review::FindingId;

/// A branch's review as a stack of pull requests, before it opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StackOpening {
    /// The attempt in flight
    pub(crate) request_id: luxe_core::guided_review::PullRequestOpenId,
    /// The pull requests, bottom first, once drafted
    pub(crate) layers: Option<Vec<luxe_core::github::NewPullRequest>>,
    /// Whether they open as drafts
    pub(crate) draft: bool,
    /// Whether it is opening
    pub(crate) opening: bool,
    /// Why it cannot, or what stopped it
    pub(crate) error: Option<String>,
}

/// Fixing someone else's pull request in a worktree, before the session is left to do it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PullRequestFix {
    /// Number within its repository
    pub(crate) number: u64,
    /// The local branch it would be checked out on, once GitHub said it can be
    pub(crate) branch: Option<String>,
    /// Why it cannot be, once GitHub said so
    pub(crate) refusal: Option<String>,
    /// Unsent comments that go with it
    pub(crate) comments: usize,
}

/// A branch's pull request, drafted from its review, before it opens.
///
/// Modal from open to settled, like the post preview: closing it while it opens would lose the
/// answer that says where the pull request is.
#[derive(Debug, Clone)]
pub(crate) struct PullRequestOpening {
    /// The attempt in flight
    pub(crate) request_id: luxe_core::guided_review::PullRequestOpenId,
    /// The draft and what opening it involves, once the engine has read them
    pub(crate) prepared: Option<PreparedOpening>,
    /// The field being edited, when one is
    pub(crate) editing: Option<(OpeningField, crate::editor::Editor)>,
    /// Rows the description is scrolled down, clamped where it is drawn
    pub(crate) body_scroll: u16,
    /// Rows the description can scroll, as last drawn
    pub(crate) body_room: std::cell::Cell<u16>,
    /// Whether it has been sent and not yet answered
    pub(crate) opening: bool,
    /// The last failure, as a sentence
    pub(crate) error: Option<String>,
}

/// What opening a pull request involves, as the preview shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PreparedOpening {
    /// The pull request, with the reader's edits
    pub(crate) pull_request: luxe_core::github::NewPullRequest,
    /// Whether the branch is on GitHub yet
    pub(crate) publication: luxe_core::github::Publication,
    /// Paths with changes not committed
    pub(crate) uncommitted: usize,
    /// Subjects of the commits written first
    pub(crate) commits: Vec<String>,
}

/// The part of a pull request the reader is editing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpeningField {
    /// The one-line title
    Title,
    /// The description
    Body,
}

/// Answers on their way to your own pull request's review threads.
///
/// Modal from open to settled, like the post preview, and for the same reason: closing it while
/// answers are in flight would drop the results that mark them answered.
#[derive(Debug, Clone)]
pub(crate) struct ThreadAnswers {
    /// The attempt in flight
    pub(crate) request_id: luxe_core::guided_review::ThreadAnswersId,
    /// One per unanswered thread, in review order
    pub(crate) rows: Vec<AnswerRow>,
    /// Row under the cursor
    pub(crate) selected: usize,
    /// The reply being written for the selected row, when one is
    pub(crate) replying: Option<crate::editor::Editor>,
    /// Whether the fix is on GitHub, once asked: `(pushed, uncommitted)`
    pub(crate) prepared: Option<(bool, bool)>,
    /// Whether answers have been sent and not yet reported on
    pub(crate) sending: bool,
    /// A failure before any thread was touched
    pub(crate) error: Option<String>,
}

/// One thread in the answers preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AnswerRow {
    /// The finding the thread became
    pub(crate) finding_id: FindingId,
    /// Whether to resolve it
    pub(crate) resolve: bool,
    /// The reader's reply, empty for none
    pub(crate) reply: String,
    /// How the last attempt at this thread went, when it failed
    pub(crate) error: Option<String>,
}

impl ThreadAnswers {
    /// Opens on every thread not yet resolved on GitHub, resolving by default the ones the reader
    /// verified and may resolve.
    pub(crate) fn new(
        request_id: luxe_core::guided_review::ThreadAnswersId,
        findings: &[luxe_core::guided_review::ReviewFinding],
    ) -> Self {
        let rows = findings
            .iter()
            .filter_map(|finding| {
                let thread = finding.github_thread.as_ref().filter(|thread| !thread.resolved)?;
                Some(AnswerRow {
                    finding_id: finding.id.clone(),
                    resolve: thread.can_resolve
                        && finding.state == luxe_core::guided_review::FindingState::Resolved,
                    reply: String::new(),
                    error: None,
                })
            })
            .collect();
        Self {
            request_id,
            rows,
            selected: 0,
            replying: None,
            prepared: None,
            sending: false,
            error: None,
        }
    }
}
