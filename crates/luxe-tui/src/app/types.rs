//! Conversation and view value types for [`super::App`].

use super::*;

/// Current view mode.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ViewMode {
    /// Startup dashboard (shown when no prompt/resume is forced).
    Dashboard,
    /// Chat view (default).
    Chat,
    /// Full-screen immutable Guided Review workspace.
    GuidedReview,
    /// Full-screen setup surface: how to reach a model, and the files behind it.
    Configuration,
    /// Session picker overlay.
    SessionPicker,
}

impl ViewMode {
    /// Whether a composer is on screen: the conversation's, or the start screen's prompt.
    pub(crate) fn shows_composer(self) -> bool {
        matches!(self, Self::Chat | Self::Dashboard)
    }
}

/// Stable identity for one semantic conversation item.
///
/// Persisted items are scoped by the session entry that owns their source content block. Live
/// items stay turn-scoped until the engine commits that entry.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ConversationItemId {
    /// A content block replayed from a persisted session entry.
    Persisted { session_id: String, entry_id: String, block_index: usize },
    /// A tool call whose lifecycle is keyed by its active session and call ID.
    Tool { session_id: String, call_id: String },
    /// A live content block awaiting a matching persisted entry.
    Provisional { session_id: String, turn_id: u64, block_index: usize },
}

/// Semantic viewport position retained across conversation reflow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationAnchor {
    /// Stable identity of the first visible conversation item.
    pub item_id: ConversationItemId,
    /// Rendered line offset within that item at capture time.
    pub visual_line_within_item: usize,
    /// Conversation item order at capture time, used if the item is removed.
    pub item_index: usize,
    /// Terminal row occupied by the anchored line at capture time.
    pub screen_row: u16,
    /// Scroll offset in effect when the anchor was captured.
    ///
    /// Restoring an anchor re-derives the offset from the transcript, which would silently undo
    /// any scrolling the user did since. Comparing against this baseline recovers that intent:
    /// the restore fixes the semantic position, then the user's delta is re-applied on top.
    pub scroll_offset: usize,
}

/// How a submission made while a turn is running should be delivered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MidTurnDelivery {
    /// Nudge the running turn; lands at its next checkpoint.
    Steer,
    /// Hold for a separate turn after this one ends.
    Queue,
}

/// A review the reader asked for while a turn was running.
///
/// The engine will not capture one mid-turn and should not: a review pins an immutable revision,
/// and the change the agent is still writing is a moving target. `turn.rs` re-queues the request
/// for after the turn either way — so this exists to make that wait *visible*, in the tray where
/// steers and follow-ups already wait, rather than leaving the reader on a full-screen review that
/// claims to be capturing something it has not started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedReview {
    /// What will be captured once the turn is out of the way.
    pub target: luxe_core::guided_review::ReviewTarget,
    /// How far along it is.
    pub state: QueuedReviewState,
}

/// Where a queued review has got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueuedReviewState {
    /// Asked for, waiting on the turn to finish before the engine will look at it.
    Waiting,
    /// The turn is done and the capture is in flight.
    Capturing,
    /// Captured and pinned; the model is writing the guide, which is most of the wait.
    Writing {
        /// When the guide pass began, so the row can count
        since: std::time::Instant,
    },
    /// Everything the review will show exists.
    Ready,
}

/// A submission made mid-turn that hasn't entered the conversation yet.
///
/// Admitting one straight into the transcript would drop it wherever the stream happened to be
/// when it was typed, then let the turn's own output pile on top and carry it out of view — a
/// message the user is still waiting on, scrolled away. Held submissions instead sit pinned above
/// the composer, muted, and join the transcript at the point they actually take effect.
#[derive(Debug, Clone)]
pub struct PendingSubmission {
    /// Text the transcript will show (paste-expanded, mentions folded to chips).
    pub display: String,
    /// Attachment chips shown with it.
    pub attached: Vec<String>,
    /// Where in `display` each reference chip sits, carried to the card it becomes.
    pub chips: Vec<std::ops::Range<usize>>,
    /// Expanded prompt handed to the engine.
    pub content: String,
    /// What was typed, as the composer would need it back.
    ///
    /// Neither of the other two can be edited and re-sent: `content` has each `@mention` expanded
    /// into a whole context block, and `display` has them folded into chips that no longer read as
    /// mentions. This is the draft itself, so restoring it is the same message rather than a
    /// lossy copy of one.
    pub draft: String,
    /// Image attachments submitted with it.
    pub images: Vec<crate::editor::AttachedImage>,
    /// Whether it steers the running turn or waits for its own.
    pub delivery: MidTurnDelivery,
}

/// A mouse text selection over the transcript.
///
/// The grid's coordinates are rendered-line indices, which the transcript invalidates every time it
/// reflows — so each end also carries the identity of the line it landed on, and the render pass
/// re-derives the indices from those. A selection made mid-stream survives the rest of the stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationSelection {
    /// Anchor + cursor in rendered-line space.
    pub grid: crate::selection::GridSelection,
    /// Where each end of the grid actually is, in the transcript's own terms.
    pub pins: (crate::mouse::TextPin, crate::mouse::TextPin),
}

/// A number that changes whenever any conversation block is created or edited.
///
/// Strictly monotonic and process-wide, which is what lets the transcript's layout memo ask "did
/// this change?" without looking at the content: hashing a megabyte of tool output to find out
/// that nothing moved costs more than the layout the answer saves. Monotonic rather than
/// per-item, so a popped item's replacement can never inherit its predecessor's number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlockRevision(u64);

impl BlockRevision {
    /// Stands for "no block at all" — the revision of an empty range.
    pub const ZERO: Self = Self(0);

    /// A fixed revision, for tests that need two blocks to compare in a known order.
    #[cfg(test)]
    pub(crate) fn for_test(value: u64) -> Self {
        Self(value)
    }

    /// The next unused revision.
    fn next() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }
}

/// Conversation data paired with its stable semantic identity.
///
/// The block is private on purpose. Every edit has to go through [`Self::block_mut`], which is what
/// keeps [`Self::revision`] honest — and the render path trusts that revision to decide whether it
/// may reuse the rows it laid out last frame.
#[derive(Debug, Clone)]
pub struct ConversationItem {
    /// Stable identity used for selection and semantic anchoring.
    pub id: ConversationItemId,
    block: ConvoBlock,
    revision: BlockRevision,
}

impl ConversationItem {
    /// A new item holding `block`.
    pub fn new(id: ConversationItemId, block: ConvoBlock) -> Self {
        Self { id, block, revision: BlockRevision::next() }
    }

    /// The renderable content.
    pub fn block(&self) -> &ConvoBlock {
        &self.block
    }

    /// The renderable content, for editing. Marks the item changed.
    pub fn block_mut(&mut self) -> &mut ConvoBlock {
        self.revision = BlockRevision::next();
        &mut self.block
    }

    /// Replaces the content outright.
    pub fn set_block(&mut self, block: ConvoBlock) {
        *self.block_mut() = block;
    }

    /// What the content is at, as far as anything caching a render of it is concerned.
    pub fn revision(&self) -> BlockRevision {
        self.revision
    }

    /// Unwraps the item into the content it held.
    pub fn into_block(self) -> ConvoBlock {
        self.block
    }
}

/// Structured activity shown for one tool call.
#[derive(Debug, Clone)]
pub struct ToolItem {
    /// Revisioned lifecycle snapshot supplied by the runtime projector.
    pub activity: ToolActivity,
    /// Whether bounded activity details are visible.
    pub expanded: bool,
    /// Whether a user explicitly changed the detail expansion state.
    pub details_toggled: bool,
    /// Milliseconds this call spent blocked on a person rather than on the tool.
    ///
    /// The projector times a call from dispatch to result, which for a call gated by a permission
    /// prompt is mostly the reader deciding. A card reporting `31s` for a command that ran in one
    /// is telling them about themselves.
    pub waited_on_user_ms: u64,
}

/// A block in the conversation view.
#[derive(Debug, Clone)]
pub enum ConvoBlock {
    /// A user message with optional attached files.
    ///
    /// `chips` are the byte ranges of `text` that stand for a reference (`acme/todo#12`, a skill),
    /// so the card can draw them the way the composer did rather than as more of the prose.
    User { text: String, attached: Vec<String>, chips: Vec<std::ops::Range<usize>> },
    /// An image attached to a user message.
    ///
    /// The pixels live in [`crate::graphics::ImageStore`], which owns every image's lifetime;
    /// carrying only the key keeps a megabyte of base64 out of a transcript that is walked and
    /// laid out on every frame.
    Image { key: crate::graphics::ImageKey, width: u32, height: u32 },
    /// An assistant message (may be streaming).
    Assistant(String),
    /// Model reasoning ("thinking"). `expanded` controls whether the body shows or just a peek.
    Thinking { text: String, expanded: bool },
    /// A revisioned tool lifecycle item, boxed.
    ///
    /// It dwarfs every other variant, and the transcript holds one block per message — so inline it
    /// costs every user message and every line of prose the size of a tool card.
    Tool(Box<ToolItem>),
    /// A system message.
    System(String),
    /// A `/context` report: what is in the window, and what to do about it.
    ///
    /// Its own variant rather than [`ConvoBlock::System`] text because the rows carry meters and
    /// severity hues, and boxed for the same reason [`ConvoBlock::Tool`] is.
    Context(Box<luxe_core::context_report::ContextComposition>),
}

/// How the `@` picker's Files tab keeps up with the disk.
///
/// The list is read off the render thread — as the picker opens, every
/// [`FileListing::EVERY`] while it stays open on Files, when the workspace moves, and on `Ctrl+R` —
/// and one read at a time: a request made while one runs waits for it, then starts its own.
#[derive(Debug, Clone, Default)]
pub struct FileListing {
    /// A read is wanted, and the main loop has not started it yet
    pub requested: bool,
    /// A read is running
    pub in_flight: bool,
    /// When the last read started
    pub started: Option<std::time::Instant>,
}

impl FileListing {
    /// How often an open picker reads the files again by itself.
    pub const EVERY: std::time::Duration = std::time::Duration::from_secs(5);

    /// Asks for a read.
    pub fn request(&mut self) {
        self.requested = true;
    }

    /// Whether the loop should start a read now; marks it running when so.
    pub fn start(&mut self, now: std::time::Instant) -> bool {
        if self.in_flight || !std::mem::take(&mut self.requested) {
            return false;
        }
        self.in_flight = true;
        self.started = Some(now);
        true
    }

    /// Whether an open picker is due a read of its own.
    pub fn due(&self, now: std::time::Instant) -> bool {
        !self.in_flight
            && !self.requested
            && self.started.is_none_or(|started| now.duration_since(started) >= Self::EVERY)
    }
}

/// Where leaving the current session goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NextSession {
    /// A fresh session
    New,
    /// The session with this id
    Load(String),
    /// Whichever session was used last
    ResumeLatest,
}
