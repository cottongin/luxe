//! Sending and posting — the send preview, and a review's post to GitHub with its verdict.

use luxe_core::guided_review::FindingId;

/// Which comments a pending send will carry, and what it records when it lands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SendPreview {
    /// Comments excluded by the reviewer; everything else in the session ships.
    pub(crate) excluded: std::collections::HashSet<FindingId>,
    /// Row under the cursor.
    pub(crate) selected: usize,
    /// Outcome to record on send — `Some` only when the Finish dialog opened this.
    pub(crate) outcome: Option<luxe_core::guided_review::ReviewOutcome>,
    /// Mode pinned by the Finish dialog, else derived from what is included.
    pub(crate) mode: Option<luxe_core::guided_review::ReviewFeedbackMode>,
}

/// A pull request's review on its way to GitHub.
///
/// Opens by asking GitHub what the preview needs, and stays modal until the post settles: closing
/// it mid-post would drop the answer that marks the comments posted, and the next post would send
/// them again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GithubPost {
    /// The attempt in flight, so a late answer to an earlier one is ignored
    pub(crate) request_id: luxe_core::guided_review::PullRequestPostId,
    /// Verdict chosen
    pub(crate) verdict: luxe_core::github::ReviewEvent,
    /// What GitHub said about the pull request, once it has
    pub(crate) prepared: Option<PreparedPost>,
    /// Whether the post has been sent and not yet answered
    pub(crate) posting: bool,
    /// The last failure, as a sentence
    pub(crate) error: Option<String>,
    /// The item under the cursor: the review itself, then each line comment in order
    pub(crate) selected: usize,
    /// Rows the selected item's text is scrolled by
    pub(crate) scroll: u16,
    /// The summary the agent is drafting, by its request id
    pub(crate) drafting: Option<String>,
    /// When that draft was asked for, so the wait can count
    pub(crate) drafting_since: Option<std::time::Instant>,
    /// What the agent said beside the summary it drafted, read with it
    pub(crate) draft_note: Option<String>,
}

/// What a post's preview needs from GitHub.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PreparedPost {
    /// GitHub's patches, which decide where each comment lands
    pub(crate) patches: Vec<luxe_core::github::FilePatch>,
    /// Whether the author pushed after the review was pinned
    pub(crate) head_moved: bool,
    /// Whether the reader opened the pull request, so only a comment is possible
    pub(crate) own_pull_request: bool,
}

impl GithubPost {
    /// The verdicts GitHub will take from this reader, in the order the dialog lists them.
    pub(crate) fn verdicts(&self) -> &'static [luxe_core::github::ReviewEvent] {
        use luxe_core::github::ReviewEvent;
        if self.prepared.as_ref().is_some_and(|prepared| prepared.own_pull_request) {
            &[ReviewEvent::Comment]
        } else {
            &[ReviewEvent::Comment, ReviewEvent::Approve, ReviewEvent::RequestChanges]
        }
    }

    /// Moves the cursor through `count` items, wrapping, and puts the new item's text at its top.
    pub(crate) fn move_selection(&mut self, forward: bool, count: usize) {
        if count == 0 {
            return;
        }
        let at = self.selected.min(count - 1);
        self.selected = if forward { (at + 1) % count } else { (at + count - 1) % count };
        self.scroll = 0;
    }

    /// Scrolls the selected item's text; the render clamps it to what there is.
    pub(crate) fn scroll_by(&mut self, rows: i32) {
        self.scroll = u16::try_from((i32::from(self.scroll) + rows).max(0)).unwrap_or(u16::MAX);
    }

    /// Steps the verdict through the ones on offer, wrapping.
    pub(crate) fn move_verdict(&mut self, forward: bool) {
        let verdicts = self.verdicts();
        let at = verdicts.iter().position(|verdict| *verdict == self.verdict).unwrap_or(0);
        let step = if forward { 1 } else { verdicts.len() - 1 };
        self.verdict = verdicts[(at + step) % verdicts.len()];
    }
}
