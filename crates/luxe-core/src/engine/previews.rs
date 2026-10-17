//! Previews — what a confirm confirms, kept by the engine between the two steps.
//!
//! Restacking, opening a pull request or a stack, and posting a review are each prepared, shown,
//! then confirmed. The engine keeps what it prepared under the request's id, so the confirm names
//! it instead of handing it back: a front end cannot send a plan, a draft or a set of patches it
//! was never shown, and an RPC client never has to build one.

use crate::github::{FilePatch, NewPullRequest};
use crate::guided_review::{PullRequestOpenId, PullRequestPostId, ReviewId, ReviewRevisionId};
use crate::restack::{Planned, RestackId};

/// The newest request of one kind, and what it prepared once that arrives.
#[derive(Debug)]
pub(crate) struct Preview<Id, T> {
    current: Option<Id>,
    value: Option<T>,
}

impl<Id, T> Default for Preview<Id, T> {
    fn default() -> Self {
        Self { current: None, value: None }
    }
}

impl<Id: PartialEq, T> Preview<Id, T> {
    /// Makes `id` the request a confirm may name, dropping whatever an older one left.
    pub(crate) fn begin(&mut self, id: Id) {
        self.current = Some(id);
        self.value = None;
    }

    /// Keeps `value` if `id` is still the newest request; a superseded preview is never confirmed.
    pub(crate) fn keep(&mut self, id: &Id, value: T) {
        if self.current.as_ref() == Some(id) {
            self.value = Some(value);
        }
    }

    /// What `id` prepared, for a confirm that may fail and be tried again.
    pub(crate) fn get(&self, id: &Id) -> Option<&T> {
        self.value.as_ref().filter(|_| self.current.as_ref() == Some(id))
    }

    /// What `id` prepared, once: the confirm that used it is the last.
    ///
    /// A confirm that beats its own preview finds nothing, but leaves the request current, so the
    /// preview still lands and the next confirm gets it.
    pub(crate) fn take(&mut self, id: &Id) -> Option<T> {
        if self.current.as_ref() != Some(id) {
            return None;
        }
        let value = self.value.take();
        if value.is_some() {
            self.current = None;
        }
        value
    }
}

/// The pull request a branch's review would open, as drafted.
#[derive(Debug, Clone)]
pub(crate) struct OpeningPreview {
    /// The review it was drafted from
    pub(crate) review_id: ReviewId,
    /// The revision that review stood at, which a confirm must still find
    pub(crate) revision_id: ReviewRevisionId,
    /// Head, base, title, description
    pub(crate) pull_request: NewPullRequest,
    /// Whether the reviewed work is committed first, as the preview said
    pub(crate) commits: bool,
}

/// A branch's review drafted as a stack.
#[derive(Debug, Clone)]
pub(crate) struct StackPreview {
    pub(crate) review_id: ReviewId,
    pub(crate) revision_id: ReviewRevisionId,
    /// The pull requests, bottom first
    pub(crate) layers: Vec<NewPullRequest>,
}

/// What a pull request's review needs to be posted, as read when it was prepared.
#[derive(Debug, Clone)]
pub(crate) struct PostPreview {
    pub(crate) review_id: ReviewId,
    pub(crate) revision_id: ReviewRevisionId,
    /// GitHub's patches, which decide where each comment can land
    pub(crate) patches: Vec<FilePatch>,
    /// Whether the reader opened the pull request, which rules out approving it
    pub(crate) own_pull_request: bool,
}

/// Every preview the engine is holding for a confirm.
#[derive(Debug, Default)]
pub(crate) struct Previews {
    pub(crate) restack: Preview<RestackId, Planned>,
    pub(crate) opening: Preview<PullRequestOpenId, OpeningPreview>,
    pub(crate) stack: Preview<PullRequestOpenId, StackPreview>,
    pub(crate) post: Preview<PullRequestPostId, PostPreview>,
}

/// The previews, even if a thread panicked holding them: the worst a poisoned preview can be is
/// stale, and every confirm already refuses anything but the newest.
pub(crate) fn lock(previews: &std::sync::Mutex<Previews>) -> std::sync::MutexGuard<'_, Previews> {
    previews.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A confirm takes exactly the newest preview, once.
    ///
    /// The breaks this catches: confirming a preview a newer request superseded (the reader was
    /// shown the newer one), keeping a late answer from the older request, and confirming twice.
    #[test]
    fn only_the_newest_preview_is_taken_and_only_once() {
        let mut preview = Preview::<u8, &str>::default();
        preview.begin(1);
        preview.begin(2);
        preview.keep(&1, "late");
        preview.keep(&2, "shown");

        assert_eq!(preview.take(&1), None, "a superseded preview is never confirmed");
        assert_eq!(preview.take(&2), Some("shown"));
        assert_eq!(preview.take(&2), None, "confirmed once");
    }

    /// A confirm that arrives before its preview finds nothing, never an older preview, and does
    /// not stop the preview from landing.
    #[test]
    fn a_confirm_before_the_preview_arrives_finds_nothing() {
        let mut preview = Preview::<u8, &str>::default();
        preview.begin(1);
        preview.keep(&1, "old");
        preview.begin(2);
        assert_eq!(preview.take(&2), None, "the older preview does not stand in");
        assert_eq!(preview.take(&1), None, "beginning a new preview dropped the old one");
        preview.keep(&2, "shown");
        assert_eq!(preview.take(&2), Some("shown"));
    }

    /// A confirm that may be tried again reads the preview without using it up, and only for its
    /// own request.
    #[test]
    fn a_retryable_confirm_reads_without_taking() {
        let mut preview = Preview::<u8, &str>::default();
        preview.begin(1);
        preview.keep(&1, "shown");
        assert_eq!(preview.get(&1), Some(&"shown"));
        assert_eq!(preview.get(&1), Some(&"shown"), "still there for a second try");
        assert_eq!(preview.get(&2), None);
        preview.begin(2);
        assert_eq!(preview.get(&1), None, "superseded");
    }
}
