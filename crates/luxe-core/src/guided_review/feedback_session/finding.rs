//! Finding — a comment and its thread: the vocabulary a session file is written in.

use super::*;

/// A user finding attached to one exact review location.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewFinding {
    /// Durable machine identity, rendered as a short local label by the UI.
    pub id: FindingId,
    /// Stable short reference (`R1`, `R2`, …) assigned once and never reused.
    ///
    /// A position-based label would renumber when one is deleted, and the agent — which only sees
    /// the durable ID — could not be talked to about "R2".
    #[serde(default)]
    pub label: String,
    /// Immutable or conservatively remapped location.
    pub anchor: FindingAnchor,
    /// User-selected importance for agent and reviewer triage.
    pub severity: FindingSeverity,
    /// Concise finding summary.
    pub title: String,
    /// Full user finding text.
    pub body: String,
    /// Optional narrow remediation suggested by the reviewer.
    pub proposed_fix: Option<String>,
    /// Current user-verifiable lifecycle state.
    pub state: FindingState,
    /// User, agent, and system evidence retained with this finding.
    pub thread: Vec<FindingMessage>,
    /// Where it was posted, once it has been; a posted comment is never posted again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub posted: Option<PostedComment>,
    /// The review thread on the reader's own pull request this finding came from, when it did
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github_thread: Option<GitHubThread>,
}

/// A comment that went to GitHub as part of a review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PostedComment {
    /// GitHub's id for the review it went in
    pub review_id: u64,
    /// Where that review can be read
    pub url: String,
}

/// A review thread on the reader's own pull request, carried by the finding it became.
///
/// What answering it needs, and what has been answered. The words are the reviewer's, not the
/// reader's: they reach the agent as a stranger's to weigh, never as instructions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitHubThread {
    /// GitHub's id for the thread
    pub id: String,
    /// Where it can be read
    pub url: String,
    /// Who opened it
    pub author: String,
    /// Whether the reader may reply to it
    pub can_reply: bool,
    /// Whether the reader may resolve it
    pub can_resolve: bool,
    /// Where the reader's reply can be read, once there is one
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_url: Option<String>,
    /// Whether the reader resolved it on GitHub
    #[serde(default)]
    pub resolved: bool,
    /// Whether it was said in the pull request's conversation rather than in a review thread: a
    /// reply is a new comment there, and there is nothing to resolve
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub conversation: bool,
}

/// Longest a title derived from a comment's opening line runs, in characters.
const MAX_DERIVED_TITLE_CHARS: usize = 80;

/// The title a comment carries when nobody wrote one: its opening line, cut when long.
///
/// There is no title field to fill in. The durable record still has a title, because the drawer,
/// the send preview and the agent's message all need something short to name a comment by, and
/// deriving it costs the reviewer nothing.
pub fn title_from_body(body: &str) -> String {
    let Some(line) = body.lines().map(str::trim).find(|line| !line.is_empty()) else {
        return String::new();
    };
    if line.chars().count() <= MAX_DERIVED_TITLE_CHARS {
        return line.to_owned();
    }
    let head: String = line.chars().take(MAX_DERIVED_TITLE_CHARS.saturating_sub(1)).collect();
    format!("{}…", head.trim_end())
}

impl ReviewFinding {
    /// Rejects malformed user content before it becomes durable review state.
    pub fn validate(&self) -> Result<(), ReviewSessionValidationError> {
        if self.title.trim().is_empty() {
            return Err(ReviewSessionValidationError::EmptyFindingTitle);
        }
        if self.body.trim().is_empty() {
            return Err(ReviewSessionValidationError::EmptyFindingBody);
        }
        self.anchor.validate()
    }

    /// The numeric part of an `R<n>` label, when it has one.
    pub(super) fn label_number(&self) -> Option<u32> {
        self.label.strip_prefix('R')?.parse().ok()
    }
}

/// What a piece of feedback asks of the agent.
///
/// Two of these are reachable: the verb that opens the composer picks one. `c` writes an
/// [`Issue`](Self::Issue) — do something about this — and `a` writes a
/// [`Question`](Self::Question), which is the only value that flips the submission into
/// [`ReviewFeedbackMode::Question`], whose turn is held to reading. One thing changes it
/// afterwards, and only one way: `c` on a question whose answer says the code should change makes
/// it an issue ([`ReviewSession::ask_for_fix`]), so the fix goes with the thread that found it.
///
/// The other three are read-only history. A composer that asked for a severity turned every
/// comment into a small act of triage — mid-thought, on a line you were still reading, for a
/// distinction the agent can take from your prose. They stay in the vocabulary because sessions on
/// disk carry them, and a reviewer whose session will not load has lost their afternoon; the
/// ordering and the submission-mode rules below still read them exactly as they were written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingSeverity {
    /// A request for an explanation, forbidding an edit.
    ///
    /// Asking is the primary verb when a reviewer is building a theory of code an agent wrote, so
    /// it is a first-class severity rather than a note the agent has to interpret.
    Question,
    /// Informational feedback that does not request a change. Legacy; nothing writes it.
    Note,
    /// A small optional improvement. Legacy; nothing writes it.
    Suggestion,
    /// A change the reviewer expects the agent to address.
    Issue,
    /// A serious issue requiring explicit acknowledgement to accept. Legacy; nothing writes it.
    Blocking,
}

/// Lifecycle state controlled by user review, never inferred from a missing line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingState {
    /// Local finding not included in a submission yet.
    Draft,
    /// Submitted finding awaiting remediation or verification.
    Open,
    /// A refresh or agent report requires user confirmation.
    NeedsVerification,
    /// Explicitly resolved by the reviewer.
    Resolved,
    /// Original anchor no longer maps conservatively.
    Stale,
}

/// One item in a persisted finding thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindingMessage {
    /// Origin of this evidence.
    pub author: FindingMessageAuthor,
    /// Bounded display-safe message text.
    pub body: String,
    /// Creation time in milliseconds since Unix epoch.
    pub created_at_ms: u64,
}

/// Origin of a finding-thread entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingMessageAuthor {
    /// The local reviewer.
    Reviewer,
    /// A remediation report from the active agent.
    Agent,
    /// luxe-generated lifecycle evidence.
    System,
}

impl FindingMessageAuthor {
    /// Who said it, for the agent-facing transcript and the details overlay.
    pub fn label(self) -> &'static str {
        match self {
            Self::Reviewer => "Reviewer",
            Self::Agent => "Agent",
            Self::System => "luxe",
        }
    }
}

/// Local review completion state; it never performs Git side effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewOutcome {
    /// The reviewer accepted the local change.
    Accepted,
    /// The reviewer submitted a request for fixes.
    ChangesRequested,
    /// The reviewer submitted comments without a fix request.
    Commented,
}
