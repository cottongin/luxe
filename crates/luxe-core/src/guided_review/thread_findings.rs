//! Thread findings — the review threads on your own pull request, as findings to work through.
//!
//! A reviewer's thread and a comment the reader left themselves ask for the same thing, so they
//! take the same road: `RequestFixes` sends them to the agent, the remediation report comes back
//! as evidence, and the reader verifies. What is different is who wrote them. The words are a
//! stranger's — already filtered and bounded when the conversation was read — and the finding
//! remembers the thread it came from, so answering it later reaches the right place.
//!
//! # Placement
//!
//! A thread's line numbers count on the pull request's head, and this review runs to the working
//! tree, which may have moved since. So a number is never trusted on its own: a thread lands on a
//! line only when that line's text is the one the thread was written on, first at the number it
//! gives and then anywhere in its file where that text appears exactly once. Failing both, it sits
//! on the review as a whole and says where it was written, rather than on a nearby line that
//! happens to be free.

use super::{
    ConversationEntry, DiffSide, EarlierThread, FindingAnchor, FindingId, FindingSeverity,
    FindingState, GitHubThread, GuidedReview, ReviewFile, ReviewFinding, ReviewHunk, ReviewLine,
    ReviewLineKind,
};

/// What the reader sends back to one thread, named by the comment it became: their own reply, if
/// they wrote one, and whether to resolve it.
///
/// What a front end sends. The engine reads which GitHub thread that is, and whether it was said in
/// the conversation, from the session, and answers with a [`ThreadAnswer`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ThreadReply {
    /// The comment the thread became
    pub finding_id: FindingId,
    /// The reader's reply, in their words; nothing is added to it
    #[serde(default)]
    pub reply: Option<String>,
    /// Whether to resolve it
    #[serde(default)]
    pub resolve: bool,
}

/// What the reader sends back to one thread: their own reply, if they wrote one, and whether to
/// resolve it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ThreadAnswer {
    /// The finding the thread became
    pub finding_id: FindingId,
    /// GitHub's id for the thread
    pub thread_id: String,
    /// The reader's reply, in their words; nothing is added to it
    pub reply: Option<String>,
    /// Whether to resolve it
    pub resolve: bool,
    /// Whether it was said in the conversation, so a reply is a new comment there
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub conversation: bool,
}

/// What answering one thread did.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ThreadAnswerResult {
    /// The finding the thread became
    pub finding_id: FindingId,
    /// Where the reply can be read, when one was sent and landed
    pub reply_url: Option<String>,
    /// Whether the thread is now resolved
    pub resolved: bool,
    /// What went wrong, as a sentence, when something did
    pub error: Option<String>,
}

/// Longest a thread's title runs, in characters, before it is cut.
const MAX_TITLE_CHARS: usize = 80;

/// A finding for each unresolved thread on the review's pull request, and for each thing said in
/// its conversation.
///
/// Resolved threads stay where they were, in the earlier conversation: there is nothing left to do
/// about them. The conversation has no such mark, so everything said there comes, and the reader
/// decides what is still asked.
pub fn thread_findings(review: &GuidedReview) -> Vec<ReviewFinding> {
    let Some(conversation) = &review.conversation else {
        return Vec::new();
    };
    let threads = conversation
        .threads()
        .iter()
        .filter(|thread| !thread.resolved && !thread.id.is_empty() && !thread.comments.is_empty())
        .map(|thread| finding(&review.files, thread));
    let said = conversation.said().iter().filter(|entry| !entry.id.is_empty()).map(said_finding);
    threads.chain(said).collect()
}

/// Something said about the pull request as a whole, as a finding on the whole review.
fn said_finding(entry: &ConversationEntry) -> ReviewFinding {
    ReviewFinding {
        id: FindingId::new(),
        label: String::new(),
        anchor: FindingAnchor::Review,
        severity: FindingSeverity::Issue,
        title: title(&entry.author, &entry.body),
        body: format!("{}: {}", author(&entry.author), entry.body),
        proposed_fix: None,
        state: FindingState::Draft,
        thread: Vec::new(),
        posted: None,
        github_thread: Some(GitHubThread {
            id: entry.id.clone(),
            url: entry.url.clone(),
            author: entry.author.clone(),
            can_reply: true,
            can_resolve: false,
            reply_url: None,
            resolved: false,
            conversation: true,
        }),
    }
}

fn finding(files: &[ReviewFile], thread: &EarlierThread) -> ReviewFinding {
    let placed = place(files, thread);
    let opening = &thread.comments[0];
    let mut body = String::new();
    if placed.is_none() {
        body.push_str(&written_on(thread));
        body.push_str("\n\n");
    }
    let said: Vec<String> = thread
        .comments
        .iter()
        // Plain, not markdown: a comment is drawn as text under its line
        .map(|comment| format!("{}: {}", author(&comment.author), comment.body))
        .collect();
    body.push_str(&said.join("\n\n"));
    let unread = thread.total_comments.saturating_sub(thread.comments.len());
    if unread > 0 {
        body.push_str(&format!("\n\n… {unread} more on GitHub"));
    }
    ReviewFinding {
        id: FindingId::new(),
        label: String::new(),
        anchor: placed.unwrap_or(FindingAnchor::Review),
        severity: FindingSeverity::Issue,
        title: title(&opening.author, &opening.body),
        body,
        proposed_fix: None,
        state: FindingState::Draft,
        thread: Vec::new(),
        posted: None,
        github_thread: Some(GitHubThread {
            id: thread.id.clone(),
            url: opening.url.clone(),
            author: opening.author.clone(),
            can_reply: thread.can_reply,
            can_resolve: thread.can_resolve,
            reply_url: None,
            resolved: false,
            conversation: false,
        }),
    }
}

/// The line the thread was written on, when the working tree still has it provably.
fn place(files: &[ReviewFile], thread: &EarlierThread) -> Option<FindingAnchor> {
    // About the file, not a line in it, whatever text came along
    if thread.file_level {
        return None;
    }
    // A blank line proves nothing: most files have several
    let text =
        thread.anchor_text.as_deref().map(str::trim_end).filter(|text| !text.trim().is_empty())?;
    let file = files.iter().find(|file| path_of(file).as_deref() == Some(thread.path.as_str()))?;
    let lines: Vec<(&ReviewHunk, &ReviewLine, u32)> = file
        .hunks
        .iter()
        .flat_map(|hunk| hunk.lines.iter().map(move |line| (hunk, line)))
        .filter_map(|(hunk, line)| Some((hunk, line, number_on(line, thread.side)?)))
        .collect();
    let at_its_number = match (thread.outdated, thread.line) {
        (false, Some(number)) => {
            lines.iter().find(|(_, line, at)| *at == number && line.text.trim_end() == text)
        }
        _ => None,
    };
    let only_match = || {
        let mut matching = lines.iter().filter(|(_, line, _)| line.text.trim_end() == text);
        match (matching.next(), matching.next()) {
            (Some(only), None) => Some(only),
            _ => None,
        }
    };
    let (hunk, line, number) = at_its_number.or_else(only_match)?;
    Some(FindingAnchor::Line {
        file_id: file.id.clone(),
        hunk_id: hunk.id.clone(),
        side: thread.side,
        line: *number,
        context_hash: line.context_hash(),
    })
}

/// The number a line has on `side`, when it exists there at all.
fn number_on(line: &ReviewLine, side: DiffSide) -> Option<u32> {
    match (side, line.kind) {
        (DiffSide::New, ReviewLineKind::Deletion) | (DiffSide::Old, ReviewLineKind::Addition) => {
            None
        }
        (DiffSide::New, _) => line.new_line,
        (DiffSide::Old, _) => line.old_line,
    }
}

fn path_of(file: &ReviewFile) -> Option<String> {
    file.new_path.as_deref().or(file.old_path.as_deref()).map(crate::paths::display)
}

/// Where a thread was written, for one that could not be placed on a line.
fn written_on(thread: &EarlierThread) -> String {
    let path = &thread.path;
    if thread.file_level {
        return format!("Written on `{path}` as a whole.");
    }
    let place = match thread.line {
        Some(line) => format!("`{path}` line {line}"),
        None => format!("`{path}`"),
    };
    match thread.anchor_text.as_deref().map(str::trim).filter(|text| !text.is_empty()) {
        Some(text) => format!(
            "Written on {place}, at `{text}`, which this review does not show as it was, so it \
             sits on the review as a whole."
        ),
        None => format!(
            "Written on {place}, which this review cannot match to a line, so it sits \
                         on the review as a whole."
        ),
    }
}

fn author(login: &str) -> &str {
    if login.is_empty() {
        "a deleted account"
    } else {
        login
    }
}

/// `mara: This starts at 1…`, from the opening comment's first line.
fn title(login: &str, body: &str) -> String {
    let first = body.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or("A comment");
    let title = format!("{}: {first}", author(login));
    if title.chars().count() <= MAX_TITLE_CHARS {
        return title;
    }
    let kept: String = title.chars().take(MAX_TITLE_CHARS - 1).collect();
    format!("{}…", kept.trim_end())
}

#[cfg(test)]
mod tests;
