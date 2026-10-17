//! Earlier conversation — what reviewers already said on a pull request, placed on the code.
//!
//! A pull request arrives with review threads attached, and with what was said about it as a whole
//! in its conversation. They are other people's words about this change, so a reader should see
//! them beside the lines they are about, and should see every one of them: a thread that no longer
//! maps — outdated, on a whole file, or on a line this capture does not show — is kept and listed,
//! never dropped.
//!
//! On someone else's pull request they are context, and stay out of the guide's prompt, which is
//! why they live on the review rather than in its context: the guide should not argue with a
//! reviewer on the reader's behalf. On your own they are the work, and
//! [`super::thread_findings`] makes them feedback.

use super::{DiffSide, FileId, HunkId, ReviewFile};
use crate::github::{self, ReviewThreads, ThreadSide};
use serde::{Deserialize, Serialize};

/// Comments kept per thread: the one that opened it and the latest replies.
pub const MAX_KEPT_COMMENTS: usize = 4;

/// Longest comment kept, in characters.
const MAX_COMMENT_CHARS: usize = 800;

/// What a review knows about the conversation already on its pull request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum EarlierConversation {
    /// The threads, as far as they were read.
    Read {
        /// Threads in the order GitHub lists them
        threads: Vec<EarlierThread>,
        /// Threads the pull request has past the ones read
        #[serde(default)]
        more: usize,
        /// What people said about the pull request as a whole, oldest first
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        said: Vec<ConversationEntry>,
    },
    /// GitHub could not be asked, so the review cannot say whether anyone commented.
    Unreadable,
}

impl EarlierConversation {
    /// The threads read, empty when none could be.
    pub fn threads(&self) -> &[EarlierThread] {
        match self {
            Self::Read { threads, .. } => threads,
            Self::Unreadable => &[],
        }
    }
    /// What people said about the pull request as a whole, empty when it could not be read.
    pub fn said(&self) -> &[ConversationEntry] {
        match self {
            Self::Read { said, .. } => said,
            Self::Unreadable => &[],
        }
    }
}

/// One review thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EarlierThread {
    /// GitHub's id for it
    #[serde(default)]
    pub id: String,
    /// File it is on, relative to the repository
    pub path: String,
    /// Which side of the diff its line counts on
    pub side: DiffSide,
    /// Last line of its anchor at the pinned head, absent when outdated or on the whole file
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    /// First line of a multi-line anchor
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_line: Option<u32>,
    /// Whether the code it was written against has changed since
    pub outdated: bool,
    /// Whether someone marked it resolved
    pub resolved: bool,
    /// Whether it is about the whole file
    pub file_level: bool,
    /// Where it sits in this review, when its line is one the review shows
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<ThreadAnchor>,
    /// The opening comment and the latest replies, scrubbed and bounded
    pub comments: Vec<EarlierComment>,
    /// How many comments the thread has in all
    pub total_comments: usize,
    /// The line it was written on, as its patch shows it
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor_text: Option<String>,
    /// Whether the reader may reply to it
    #[serde(default)]
    pub can_reply: bool,
    /// Whether the reader may resolve it
    #[serde(default)]
    pub can_resolve: bool,
}

/// The line of a review a thread is drawn under.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadAnchor {
    /// File lineage
    pub file_id: FileId,
    /// Hunk holding the line
    pub hunk_id: HunkId,
}

/// One comment in a thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EarlierComment {
    /// Login of whoever wrote it; empty for a deleted account
    pub author: String,
    /// What they wrote, scrubbed and bounded
    pub body: String,
    /// Web address of the comment
    pub url: String,
}

/// A comment in a pull request's conversation, or the summary a review was submitted with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationEntry {
    /// GitHub's id for it
    pub id: String,
    /// Login of whoever wrote it; empty for a deleted account
    pub author: String,
    /// What they wrote, scrubbed and bounded
    pub body: String,
    /// Web address
    pub url: String,
}

impl EarlierThread {
    /// Whether this thread is drawn under `line` of `file`, counted on `side`.
    pub fn sits_at(&self, file_id: &FileId, side: DiffSide, line: u32) -> bool {
        self.anchor.as_ref().is_some_and(|anchor| &anchor.file_id == file_id)
            && self.side == side
            && self.line == Some(line)
    }
}

/// Places a pull request's threads on the review built from it.
///
/// A thread maps when its file is in the review and its line, on its side, is one a hunk shows.
/// Everything else keeps no anchor and is listed instead: an outdated thread has no line at the
/// current head, a file-level thread has none at all, and a line outside every hunk is not on
/// screen. Text is scrubbed here, once, because it is a stranger's.
pub fn earlier_conversation(files: &[ReviewFile], read: ReviewThreads) -> EarlierConversation {
    let more = read.total.saturating_sub(read.threads.len());
    let said = still_asked(read.conversation);
    let threads = read
        .threads
        .into_iter()
        .map(|thread| {
            let side = match thread.side {
                ThreadSide::Left => DiffSide::Old,
                ThreadSide::Right => DiffSide::New,
            };
            let anchor = match (thread.outdated || thread.file_level, thread.line) {
                (false, Some(line)) => anchor_for(files, &thread.path, side, line),
                _ => None,
            };
            EarlierThread {
                id: thread.id,
                anchor_text: thread.anchor_text,
                can_reply: thread.can_reply,
                can_resolve: thread.can_resolve,
                path: thread.path,
                side,
                line: thread.line,
                start_line: thread.start_line,
                outdated: thread.outdated,
                resolved: thread.resolved,
                file_level: thread.file_level,
                anchor,
                total_comments: thread.total_comments,
                comments: kept(thread.comments),
            }
        })
        .collect();
    EarlierConversation::Read { threads, more, said }
}

/// What was said in the conversation that is still asking something, scrubbed.
///
/// The conversation has no resolve, so answered has to be read off it. An answer from here is a
/// comment of the reader's that quotes the first line of what it answers, the way GitHub's own
/// quote reply does: that comment is an answer rather than an ask, and what it quotes has been
/// answered. Reading it off GitHub rather than remembering it is what keeps a pull request read
/// again tomorrow from asking everything again. A bot's comment is a report — coverage, a deploy
/// preview — rather than someone asking for something.
fn still_asked(conversation: Vec<github::ConversationComment>) -> Vec<ConversationEntry> {
    let entries: Vec<(bool, ConversationEntry)> = conversation
        .into_iter()
        .filter(|comment| !comment.bot)
        .map(|comment| {
            let entry = ConversationEntry {
                id: comment.id,
                author: comment.author,
                body: github::untrusted(comment.body.trim(), MAX_COMMENT_CHARS),
                url: comment.url,
            };
            (comment.mine, entry)
        })
        .collect();
    let answers: Vec<&str> = entries
        .iter()
        .filter(|(mine, _)| *mine)
        .filter_map(|(_, entry)| quoted(&entry.body))
        .collect();
    let answered = |entry: &ConversationEntry| {
        let Some(first) = first_line(&entry.body) else { return false };
        answers.iter().any(|quote| match quote.strip_suffix('…') {
            Some(cut) => first.starts_with(cut),
            None => first == *quote,
        })
    };
    entries
        .iter()
        .filter(|(mine, entry)| !(*mine && quoted(&entry.body).is_some()) && !answered(entry))
        .map(|(_, entry)| entry.clone())
        .collect()
}

/// The first line of `body` with anything on it, trimmed.
pub(crate) fn first_line(body: &str) -> Option<&str> {
    body.lines().map(str::trim).find(|line| !line.is_empty())
}

/// What `body` quotes, when it opens with a quote.
fn quoted(body: &str) -> Option<&str> {
    first_line(body)?.strip_prefix('>').map(str::trim).filter(|quote| !quote.is_empty())
}

/// The hunk of the file at `path` that shows `line` on `side`, if any does.
fn anchor_for(files: &[ReviewFile], path: &str, side: DiffSide, line: u32) -> Option<ThreadAnchor> {
    let named = |candidate: &Option<std::path::PathBuf>| {
        candidate.as_deref().is_some_and(|candidate| crate::paths::display(candidate) == path)
    };
    let file = files.iter().find(|file| named(&file.new_path) || named(&file.old_path))?;
    let hunk = file.hunks.iter().find(|hunk| {
        hunk.lines.iter().any(|candidate| match side {
            DiffSide::Old => candidate.old_line == Some(line),
            DiffSide::New => candidate.new_line == Some(line),
        })
    })?;
    Some(ThreadAnchor { file_id: file.id.clone(), hunk_id: hunk.id.clone() })
}

/// The opening comment and the latest replies, which is where a thread's point and its outcome are.
fn kept(comments: Vec<github::ThreadComment>) -> Vec<EarlierComment> {
    let count = comments.len();
    comments
        .into_iter()
        .enumerate()
        .filter(|(index, _)| *index == 0 || *index + MAX_KEPT_COMMENTS > count)
        .map(|(_, comment)| EarlierComment {
            author: comment.author,
            body: github::untrusted(comment.body.trim(), MAX_COMMENT_CHARS),
            url: comment.url,
        })
        .collect()
}

#[cfg(test)]
mod tests;
