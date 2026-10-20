//! GitHub detail — one pull request or issue: what stands in its way, and what was said.
//!
//! Shared by `/github`, beside its list, and `/pr`, on its own. It draws overviews the engine
//! fetched and never asks GitHub anything itself. The merge checks come from
//! [`luxe_core::github::overview::merge_checks`], so every front-end agrees on what blocks a merge;
//! this decides only how each one reads and which key it answers to.
//!
//! Every fact is a word beside its mark — `✗ 1 of 12 failing`, never a red cross alone — so a
//! terminal without the marks loses polish and nothing else.

mod card;
mod conversation;
mod draft;
mod header;
mod html;
mod keys;
mod merge;
mod overview;
mod parts;
mod replies;
#[cfg(test)]
mod test_support;

use std::collections::BTreeSet;

use luxe_core::github::overview::{merge_checks, CheckAction, ConversationEntry};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::theme::styles::ThemeStyles;

pub use card::{render_card, CardState, PullRequestCard};
pub(crate) use draft::note_lines as draft_note_lines;
pub use draft::{DraftCard, DraftKey};
pub use header::{ago, author_spans, diff_counts, state_word};
pub use html::readable;
pub use merge::MergeDialog;
pub use parts::action_hint;
pub(crate) use parts::lift_dim;

/// What the detail is about: the overview the engine read, one type from GitHub to the screen.
pub use luxe_core::github::overview::Overview as Subject;

/// The open tab.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DetailTab {
    #[default]
    Overview,
    Conversation,
}

/// A reply, comment or review summary waiting to be sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    /// Where it goes, as a person says it: `Reply to @jon on src/api/webhook.rs:42`
    pub target: String,
    /// The thread it answers, when it answers one
    pub thread: Option<String>,
    /// The text, markdown
    pub body: String,
    /// Whether the agent wrote it, rather than you
    pub by_agent: bool,
    /// What the agent drafted it from, as a person says it: `this session`, `the review comment,
    /// verified`, `4c1e2a0`; empty for your own words
    pub sources: Vec<String>,
    /// What the agent said beside the draft, read with it before sending
    pub note: Option<String>,
}

/// What a key asks of whoever holds the detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetailKey {
    /// The detail handled it
    Held,
    /// Give the keys back: to the list, or close
    Leave,
    /// Open this address
    Open(String),
    /// Do what a status row offers
    Act(CheckAction),
    /// Resume this session of yours, by id
    Resume(String),
    /// Read the conversation, which the Conversation tab has just been opened without
    ReadConversation,
    /// Post this reply or comment
    Post(PostRequest),
    /// Hand this text to the reader's editor, for the draft being read
    EditExternally(String),
    /// Merge it, or turn on auto-merge, as the confirmation was left
    Merge(luxe_core::engine::PullRequestWrite),
    /// Have the agent draft a reply
    Draft(DraftAsk),
}

/// A draft to ask the agent for, as the detail knows it; whoever holds the detail knows where it
/// is and what the review made of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftAsk {
    /// Correlates the answer with the card waiting on it
    pub request_id: String,
    /// What it answers
    pub target: luxe_core::engine::DraftTarget,
    /// Everything said, for the digest
    pub conversation: Vec<ConversationEntry>,
    /// What should change, for a redraft
    pub note: Option<String>,
    /// The draft as the reader left it, for a redraft
    pub current: Option<String>,
}

/// A reply or comment to post, as the detail knows it; whoever holds the detail knows where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostRequest {
    /// The review thread it replies in, when it answers one
    pub thread: Option<String>,
    /// What it answers, outside a thread, to quote above it
    pub answering: Option<String>,
    /// The words
    pub body: String,
}

/// A cursor stop in the Conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stop {
    /// An entry, by its index in the conversation
    Entry(usize),
    /// A draft, by its index among the drafts
    Draft(usize),
}

/// One pull request or issue, and where the reader is in it.
#[derive(Debug, Clone)]
pub struct GithubDetail {
    /// What it is about
    pub subject: Subject,
    /// Its conversation, once read
    pub conversation: Option<Vec<ConversationEntry>>,
    /// Your session on it, when there is one
    pub local: Option<luxe_core::github::overview::LocalSession>,
    /// Drafts kept for it, each shown under what it answers
    pub drafts: Vec<Draft>,
    /// The open tab
    pub tab: DetailTab,
    /// The stop under the cursor, among the open tab's rows that act
    pub cursor: usize,
    /// Conversation entries opened past their fold
    pub open: BTreeSet<usize>,
    /// Whether bots' entries are hidden
    pub hide_bots: bool,
    /// The clock ages are measured against on its own, in seconds since the Unix epoch; the wall
    /// clock when unset, which is every time but a fixture's
    pub clock: Option<u64>,
    /// The merge confirmation, while it is open
    pub merging: Option<MergeDialog>,
    /// Whether it is the pull request of the branch checked out here, the only one whose threads
    /// can be worked through against this working tree
    pub on_branch: bool,
    /// A reply or comment being written or read before it is sent
    pub composing: Option<DraftCard>,
    /// Whether the conversation has been asked for
    pub conversation_asked: bool,
    /// Why the conversation could not be read, as a sentence
    pub conversation_error: Option<String>,
    /// Drafts the agent is writing whose cards were put away; each is kept when it lands
    pub waiting: Vec<DraftCard>,
}

impl GithubDetail {
    /// A detail on its Overview, cursor at the top.
    pub fn new(subject: Subject) -> Self {
        Self {
            subject,
            conversation: None,
            local: None,
            drafts: Vec::new(),
            tab: DetailTab::Overview,
            cursor: 0,
            open: BTreeSet::new(),
            hide_bots: false,
            clock: None,
            merging: None,
            on_branch: false,
            composing: None,
            conversation_asked: false,
            conversation_error: None,
            waiting: Vec::new(),
        }
    }

    fn number(&self) -> u64 {
        match &self.subject {
            Subject::PullRequest(pr) => pr.number,
            Subject::Issue(issue) => issue.number,
        }
    }

    fn conversation_count(&self) -> usize {
        match &self.subject {
            Subject::PullRequest(pr) => pr.conversation_count,
            Subject::Issue(issue) => issue.conversation_count,
        }
    }

    fn url(&self) -> &str {
        match &self.subject {
            Subject::PullRequest(pr) => &pr.url,
            Subject::Issue(issue) => &issue.url,
        }
    }

    /// The Conversation's cursor stops, in the order they are drawn.
    fn conversation_stops(&self) -> Vec<Stop> {
        let mut stops = Vec::new();
        for (index, entry) in self.conversation.iter().flatten().enumerate() {
            if self.hide_bots && entry.by_bot() {
                continue;
            }
            stops.push(Stop::Entry(index));
            if let ConversationEntry::Thread(thread) = entry {
                let answering = self.drafts.iter().enumerate();
                stops.extend(
                    answering
                        .filter(|(_, draft)| draft.thread.as_deref() == Some(&thread.id))
                        .map(|(at, _)| Stop::Draft(at)),
                );
            }
        }
        let loose = self.drafts.iter().enumerate().filter(|(_, draft)| draft.thread.is_none());
        stops.extend(loose.map(|(at, _)| Stop::Draft(at)));
        stops
    }

    /// Whether a status row's action can be taken from here. A row whose action cannot is still
    /// drawn, with its evidence and no key: a key that does nothing is worse than none.
    pub(crate) fn acts(&self, action: CheckAction) -> bool {
        match action {
            // Work done in this checkout, so only on the pull request it has checked out
            CheckAction::ResolveThreads
            | CheckAction::FixChecks
            | CheckAction::ResolveConflicts => self.on_branch,
            // Asked of GitHub, which works from anywhere the account may
            CheckAction::UpdateBranch
            | CheckAction::MarkReady
            | CheckAction::OpenBelow(_)
            | CheckAction::Merge
            | CheckAction::EnableAutoMerge
            | CheckAction::DisableAutoMerge => true,
        }
    }

    /// The Overview's cursor stops: each status row that acts, then your session, top to bottom.
    fn overview_stops(&self) -> Vec<Option<CheckAction>> {
        match &self.subject {
            Subject::PullRequest(pr) => merge_checks(pr)
                .into_iter()
                .filter_map(|check| check.action.filter(|action| self.acts(*action)))
                .map(Some)
                .collect(),
            Subject::Issue(issue) => {
                let linked = overview::closing_pull_requests(issue).into_iter().map(|_| None);
                linked.chain(self.local.iter().map(|_| None)).collect()
            }
        }
    }
}

/// Pull request `number`'s address, from an issue's on the same repository: the host stays
/// whatever it is, GitHub Enterprise included.
fn pull_request_url(issue_url: &str, number: u64) -> String {
    match issue_url.rsplit_once("/issues/") {
        Some((repository, _)) => format!("{repository}/pull/{number}"),
        None => issue_url.to_owned(),
    }
}

// === Drawing ===

/// Draws the detail into `area`: its header, the tabs, and the open tab scrolled to the cursor.
///
/// Args: `(frame, area, detail, focused, now, styles)` — `now` in seconds since the Unix epoch, so
/// ages are relative to one clock for the whole frame.
pub fn render(
    f: &mut Frame,
    area: Rect,
    detail: &GithubDetail,
    focused: bool,
    now: u64,
    styles: &ThemeStyles,
) {
    if area.width < 20 || area.height < 6 {
        return;
    }
    let width = usize::from(area.width);
    let mut head = header::header(detail, width, now, styles);
    head.push(Line::default());
    let count = detail.conversation_count();
    let conversation = match count {
        0 => "Conversation".to_string(),
        count => format!("Conversation {count}"),
    };
    let open = usize::from(detail.tab == DetailTab::Conversation);
    let tabs = [
        crate::glyphs::icon_label(crate::glyphs::Icon::Overview.glyph(), "Overview"),
        crate::glyphs::icon_label(crate::glyphs::Icon::Conversation.glyph(), &conversation),
    ];
    head.push(crate::picker::tab_line(&[&tabs[0], &tabs[1]], open, styles));
    head.push(Line::from(Span::styled("─".repeat(width), styles.border_muted)));
    let head_rows = u16::try_from(head.len()).unwrap_or(u16::MAX).min(area.height);
    f.render_widget(Paragraph::new(head), Rect { height: head_rows, ..area });

    let body_area = Rect {
        y: area.y + head_rows + 1,
        height: area.height.saturating_sub(head_rows + 1),
        ..area
    };
    // Two cells short of the edge, so a card's right border and the scrollbar never share a column
    let body_width = width.saturating_sub(2);
    let (lines, cursor_line) = match detail.tab {
        DetailTab::Overview => overview::overview(detail, body_width, focused, now, styles),
        DetailTab::Conversation => {
            conversation::conversation_lines(detail, body_width, focused, now, styles)
        }
    };
    let rows = usize::from(body_area.height);
    // Once the cursor passes two thirds of the way down, it rides a third of the way from the top:
    // what is under an entry — an opened thread, the draft answering it — is why it was chosen
    let first = cursor_line
        .filter(|line| *line >= rows * 2 / 3)
        .map_or(0, |line| line.saturating_sub(rows / 3))
        .min(lines.len().saturating_sub(rows));
    let shown: Vec<Line> = lines.iter().skip(first).take(rows).cloned().collect();
    f.render_widget(Paragraph::new(shown), body_area);
    if lines.len() > rows {
        crate::widgets::scrollbar(f, body_area, first, lines.len(), rows, styles);
    }
}

/// Draws what the detail has open over it within `area`: the merge confirmation, or the reply
/// card. The one owner for `/github` and `/pr` alike, since a confirmation that takes keys must be
/// on screen wherever it can take them.
///
/// Args: `(frame, area, detail, spinner frame, styles)`.
pub(crate) fn render_overlays(
    f: &mut Frame,
    area: Rect,
    detail: &GithubDetail,
    frame: usize,
    styles: &ThemeStyles,
) {
    if detail.merging.is_some() {
        crate::widgets::dim_backdrop(f, area, styles);
        merge::render(f, area, detail, styles);
    }
    if let Some(composing) = &detail.composing {
        crate::widgets::dim_backdrop(f, area, styles);
        draft::render(f, area, composing, frame, styles);
    }
}

#[cfg(test)]
mod tests;
