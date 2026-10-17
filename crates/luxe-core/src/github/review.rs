//! Review — threads, patches and reviews: the vocabulary of reviewing a pull request.

use super::*;

/// The review threads on a pull request, and what was said in its conversation, as far as one
/// query reads them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReviewThreads {
    /// The threads read, oldest first
    pub threads: Vec<ReviewThread>,
    /// How many the pull request has in all, which is more than `threads` past the query's limit
    pub total: usize,
    /// The latest conversation comments and review summaries, oldest first: not minimized, not
    /// empty
    pub conversation: Vec<ConversationComment>,
}

/// Something said about a pull request as a whole: a comment in its conversation, or the summary
/// a review was submitted with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationComment {
    /// GitHub's id for it
    pub id: String,
    /// Author login; empty for a deleted account
    pub author: String,
    /// Whether the author is a bot account
    pub bot: bool,
    /// Whether the account `gh` is signed in as wrote it
    pub mine: bool,
    /// What they wrote, raw
    pub body: String,
    /// When it was written, as GitHub reports it (RFC 3339)
    pub created_at: String,
    /// Web address
    pub url: String,
}

/// A review thread on a pull request: comments anchored to a file, and usually to a line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewThread {
    /// GitHub's id for the thread, which a reply or a resolve names
    pub id: String,
    /// File the thread is on, relative to the repository
    pub path: String,
    /// Last line of the anchor at the current head, absent when the thread is outdated or on the
    /// whole file
    pub line: Option<u32>,
    /// First line of a multi-line anchor
    pub start_line: Option<u32>,
    /// Which side of the diff the line numbers count on
    pub side: ThreadSide,
    /// Whether the code it was written against has since changed
    pub outdated: bool,
    /// Whether someone marked it resolved
    pub resolved: bool,
    /// Whether it is about the whole file rather than a line
    pub file_level: bool,
    /// The comments, oldest first; at most 20
    pub comments: Vec<ThreadComment>,
    /// How many comments the thread has in all
    pub total_comments: usize,
    /// The line the thread was written on, as its first comment's patch shows it, without the
    /// patch's `+`, `-` or space: what places it on a working tree that has moved since
    pub anchor_text: Option<String>,
    /// Whether this account may reply to it
    pub can_reply: bool,
    /// Whether this account may resolve it
    pub can_resolve: bool,
}

/// Which side of a pull request's diff a thread's line numbers count on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadSide {
    /// The base, before the change
    Left,
    /// The head, after the change
    Right,
}

/// One comment in a review thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadComment {
    /// Author login; empty for a deleted account
    pub author: String,
    /// Comment text, raw
    pub body: String,
    /// When it was written, as GitHub reports it (RFC 3339)
    pub created_at: String,
    /// Web address of the comment
    pub url: String,
}

/// One file a pull request changes, as GitHub's own diff shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilePatch {
    /// Path at the pull request's head
    pub path: String,
    /// Path before a rename
    pub previous_path: Option<String>,
    /// The lines a comment can sit on
    pub commentable: CommentableLines,
}

/// The lines of one file GitHub accepts a line comment on, by side.
///
/// GitHub refuses a comment on any other line (`Line could not be resolved`), and a file too large
/// or too binary for a patch takes none at all.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommentableLines {
    /// Old-side line numbers inside a hunk
    pub left: std::collections::BTreeSet<u32>,
    /// New-side line numbers inside a hunk
    pub right: std::collections::BTreeSet<u32>,
}

impl CommentableLines {
    /// Whether a comment can sit on `line` of `side`.
    pub fn accepts(&self, side: ThreadSide, line: u32) -> bool {
        match side {
            ThreadSide::Left => self.left.contains(&line),
            ThreadSide::Right => self.right.contains(&line),
        }
    }
}

/// A verdict to post with a review.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewEvent {
    Comment,
    Approve,
    RequestChanges,
}

/// One review, ready to post.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmitReview {
    /// Commit the comments are anchored to: the one the reviewer read
    pub commit_id: String,
    /// The verdict
    pub event: ReviewEvent,
    /// The review's own text
    pub body: String,
    /// Comments on lines
    pub comments: Vec<DraftComment>,
}

/// A comment on one line, or on a run of lines ending at `line`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftComment {
    /// File, relative to the repository
    pub path: String,
    /// Last line of the anchor
    pub line: u32,
    /// Side both lines count on
    pub side: ThreadSide,
    /// First line of a multi-line anchor
    pub start_line: Option<u32>,
    /// What the reviewer wrote
    pub body: String,
}

/// A review that landed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostedReview {
    /// GitHub's id for it
    pub id: u64,
    /// Where it can be read
    pub url: String,
}

/// A review already on a pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExistingReview {
    /// GitHub's id for it
    pub id: u64,
    /// Login of whoever posted it
    pub author: String,
    /// Commit it was posted against
    pub commit_id: String,
    /// Its text
    pub body: String,
    /// When it was submitted (RFC 3339); empty for one still pending
    pub submitted_at: String,
    /// Where it can be read
    pub url: String,
}
