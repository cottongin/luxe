//! The [`Msg`] state-machine input for [`super::App`].

use super::*;

/// Messages that drive the app state machine.
#[derive(Debug)]
pub enum Msg {
    // === Input and engine events ===
    Key(crossterm::event::KeyEvent),
    Engine(Event),
    Tick,
    /// Opens the login wizard, optionally straight into one provider. Args: `(provider_id)`
    OpenAuthWizard(Option<String>),
    Auth(crate::auth_wizard::AuthUiMsg),
    /// Terminal dimensions changed; retains responsive layout and review viewport dimensions.
    TerminalResized {
        width: u16,
        height: u16,
    },

    // === Review: moving through the diff ===
    ExitGuidedReview,
    GuidedReviewLineNext,
    GuidedReviewLinePrevious,
    GuidedReviewPageNext,
    GuidedReviewPagePrevious,
    /// Move the full-screen review diff half a viewport, forward when `true`.
    GuidedReviewHalfPage(bool),
    GuidedReviewHunkNext,
    GuidedReviewHunkPrevious,
    GuidedReviewChapterNext,
    GuidedReviewChapterPrevious,
    GuidedReviewFileNext,
    GuidedReviewFilePrevious,
    GuidedReviewLineStart,
    GuidedReviewColumnNext,
    GuidedReviewColumnPrevious,

    // === Review: what the surface shows ===
    GuidedReviewToggleGuide,
    GuidedReviewToggleNoise,
    GuidedReviewToggleNarration,
    GuidedReviewToggleNoiseDetails,
    GuidedReviewToggleSplit,
    GuidedReviewToggleWordWrap,

    // === Review: committing chapters, help and retry ===
    /// Opens the commit preview for the reviewed chapters.
    GuidedReviewOpenCommitPreview,
    GuidedReviewToggleCommitSquash,
    GuidedReviewConfirmCommit,
    GuidedReviewCancelCommit,
    GuidedReviewToggleHelp,
    /// Opens, shuts or scrolls the pull request's whole description.
    ///
    /// Args: what to do with it.
    GuidedReviewDescription(DescriptionStep),
    GuidedReviewRetry,

    // === Review: choosing a source ===
    GuidedReviewToggleSourcePicker,
    GuidedReviewSourcePickerNext,
    GuidedReviewSourcePickerPrevious,
    GuidedReviewSelectSourcePicker,
    GuidedReviewSourceReferenceInsert(char),
    GuidedReviewSourceReferenceBackspace,
    GuidedReviewSubmitSourceReference,
    GuidedReviewCancelSourceReference,

    // === Review: comments ===
    GuidedReviewBeginFinding,
    GuidedReviewEditSelectedFinding,
    GuidedReviewToggleFindings,
    GuidedReviewSelectFinding(luxe_core::guided_review::FindingId),
    /// Moves the diff cursor/anchor to an absolute visible-row index (mouse click on a diff line).
    GuidedReviewSelectDiffRow(usize),
    GuidedReviewSelectFile(luxe_core::guided_review::FileId),
    GuidedReviewToggleFileFold(luxe_core::guided_review::FileId),

    // === Actions routed through dispatch ===
    /// Dispatches a keymap `UiAction` (mouse click on an affordance that mirrors a key action,
    /// e.g. the dock "Review changes" row). Routes through the same `dispatch_action` as the key.
    DispatchUiAction(luxe_core::keymap::UiAction),
    /// Hands the pull request's description to the reader's editor
    GuidedReviewEditPullRequestExternally,
    /// Scrolls the pull request's description. Args: `(rows)`, down when positive
    GuidedReviewScrollPullRequest(i32),

    // === Answers to asynchronous reads ===
    /// A mention source answered: its rows, or why it listed none.
    MentionsListed {
        /// The source's kind
        kind: String,
        /// Its rows, or the reason as a sentence
        answer: Result<Vec<luxe_core::mention_source::MentionRow>, String>,
    },
    /// The project's files, read again for the `@` picker; `None` when the read failed.
    FilesListed(Option<Vec<String>>),
    /// The project's sessions, read for the session picker `/sessions` opened.
    SessionsListed {
        /// Which read this answers, so a picker opened since takes only its own
        request: u64,
        /// The sessions, newest first
        catalog: Vec<crate::dashboard::SessionSummary>,
    },
    /// Text came back from the reader's editor.
    TextEdited {
        /// Where it goes
        target: crate::launcher::EditTarget,
        /// What the editor left, trailing whitespace trimmed
        text: String,
    },

    // === Conversation, dock and scrollbars ===
    ToggleConversationBlockDetails(usize),
    /// Scrolls the Context Dock by one step (mouse wheel over the panel).
    ScrollDock {
        up: bool,
    },
    /// Walks the project-context list one row (mouse wheel over it).
    ScrollContextFiles {
        up: bool,
    },
    /// Walks a modal list's cursor one row (mouse wheel over it).
    ScrollOverlayList {
        list: crate::mouse::OverlayList,
        up: bool,
    },
    /// Scrolls one of the dock's lists by one step (mouse wheel over that section).
    ScrollDockList {
        list: crate::workspace_dock::DockList,
        up: bool,
    },
    /// Jumps one of the dock's lists to where its thumb was dragged.
    ///
    /// Args: `row` within a `height`-tall track, which is the track's own geometry as the render
    /// pass published it.
    DragDockList {
        list: crate::workspace_dock::DockList,
        row: u16,
        height: u16,
    },
    /// Points a scrollbar's surface at `row` of its `height`-tall track: a press on the bar, and
    /// every drag after it, for the surfaces that are not the transcript or a dock list.
    ///
    /// Args: the surface, then the row within the track as the render pass published it.
    DragScrollbar {
        region: crate::mouse::ScrollRegionId,
        row: u16,
        height: u16,
    },

    // === Pane widths ===
    /// Sizes a pane to where its divider was dragged.
    ResizePane {
        pane: crate::workspace_layout::Pane,
        width: u16,
    },
    ResetPaneWidth(crate::workspace_layout::Pane),
    FinishPaneResize,

    // === Changes pane ===
    /// Shows one path's diff in the conversation column, opening the pane. Project-relative for
    /// the project's own files, absolute for one it changed somewhere else.
    OpenChanges(String),
    /// Folds or unfolds one ELSEWHERE place. Args: `(root)`, the root it is filed under.
    ToggleExternalPlace(String),
    CloseChanges,
    /// Steps to the next or previous changed file. Args: `(forward)`
    ChangesStepFile(bool),
    /// Scrolls the visible diff by `lines`.
    ScrollChanges {
        up: bool,
        lines: usize,
    },
    /// Scrolls the visible diff by one rendered screenful. Args: `(forward)`
    PageChanges(bool),
    /// Jumps to the start or end of the visible diff. Args: `(to_end)`
    JumpChanges(bool),
    ToggleChangesWordWrap,
    ToggleChangesSplit,
    ChangesOpenReview,
    /// Shows a project file in the pane: its diff when it changed, its text when it did not.
    OpenProjectFile(String),
    TogglePeekView,
    /// Starts picking lines in the File view at this 1-based line: a click picks one line, a
    /// second click on the only line picked lets it go.
    StartPeekPick(u32),
    /// Moves the far end of the pick to this line, as a drag passes over it.
    ExtendPeekPick(u32),
    PeekSelectionAction(crate::mouse::PeekSelectionAction),

    // === FILES ===
    ToggleFolder(String),
    ToggleFilesScope,
    /// Shows what `.gitignore` ignores in the tree too, or stops.
    ToggleIgnoredFiles,
    /// Walks the keyboard on: from the composer to the tree, from the tree to the preview when one
    /// is open, and from the preview back to the tree.
    FocusFiles,
    FocusPeek,
    /// Does what an open context menu's item at this index offers, and closes the menu.
    ChooseMenuItem(usize),
    /// Starts a mouse text selection at `hit`, claiming `span` of the text under it.
    BeginTextSelection {
        hit: crate::mouse::TextHit,
        span: crate::selection::SelectionSpan,
    },
    /// Extends the in-flight mouse selection to `hit`.
    ExtendTextSelection {
        hit: crate::mouse::TextHit,
    },
    ClearTextSelection,
    CopySelection,
    /// Moves the hover highlight to the target under the pointer (`None` when over empty space).
    HoverTarget(Option<crate::mouse::MouseTarget>),
    /// Opens the model picker. Args: the step it opens on.
    OpenModelPicker(crate::model_picker::PickerStart),
    CopyText(String),
    /// Hands an image to the desktop's own viewer, for a terminal that cannot draw it inline.
    OpenImageExternally(crate::graphics::ImageKey),
    /// Fills the window with one image (double-click, or `Alt+I` on a selected message).
    OpenImageViewer(crate::graphics::ImageKey),
    CloseImageViewer,
    /// Put the chat's key card away
    CloseKeysCard,
    GuidedReviewBeginSearch,
    GuidedReviewSearchInput(char),
    GuidedReviewSearchBackspace,
    GuidedReviewCancelSearch,
    GuidedReviewSearchNext,
    GuidedReviewSearchPrevious,
    /// Validates and saves the local feedback draft, keeping it on failure.
    GuidedReviewSaveFinding,
    GuidedReviewToggleSelectedFindingResolved,
    GuidedReviewDeleteSelectedFindingDraft,
    GuidedReviewReanchorSelectedFinding,
    GuidedReviewSetOutcome(luxe_core::guided_review::ReviewOutcome),
    GuidedReviewRetryFeedback(luxe_core::guided_review::SubmissionId),
    /// Removes one saved queued submission while retaining its reviewer findings.
    GuidedReviewCancelQueuedFeedback(luxe_core::guided_review::SubmissionId),
    /// Persists selected immutable finding snapshots before feedback may reach the agent.
    /// Queues one feedback submission. `outcome` is `Some` only from the Finish dialog: sending
    /// mid-review must not declare the review over.
    GuidedReviewQueueFeedback {
        /// Dispatch intent for the immutable snapshots.
        mode: luxe_core::guided_review::ReviewFeedbackMode,
        /// Findings selected for this submission.
        finding_ids: Vec<luxe_core::guided_review::FindingId>,
        /// Local outcome to record, or `None` for a mid-review send.
        outcome: Option<luxe_core::guided_review::ReviewOutcome>,
    },
    /// Opens whatever the review cursor is on that has more to show — a collapsed run, or the
    /// comment anchored under it.
    GuidedReviewOpenSelection,
    GuidedReviewBeginQuestion,
    GuidedReviewOpenInBrowser,
    /// Scroll the guide pane itself, leaving the review's position alone.
    ///
    /// Args: `{ up }`
    GuidedReviewScrollGuide {
        /// Toward the top of the document.
        up: bool,
    },
    GuidedReviewBeginReply,
    /// Has the agent draft the summary of the review being posted
    GuidedReviewDraftPostSummary,
    /// Opens the summary of the review being posted in its composer
    GuidedReviewEditPostSummary,
    /// Asks another question on the selected question's thread
    GuidedReviewBeginFollowUp,

    // === Review: sending, stacking and opening pull requests ===
    GuidedReviewOpenSendPreview,
    /// Moves the send-preview cursor. Args: `(forward)`
    GuidedReviewMoveSendPreview(bool),
    GuidedReviewToggleSendPreviewRow,
    GuidedReviewConfirmSend,
    GuidedReviewCancelSend,
    GuidedReviewOpenStack,
    GuidedReviewToggleStackDraft,
    GuidedReviewConfirmStack,
    /// Closes the stack preview, unless it is opening.
    GuidedReviewCancelStack,
    /// Asks whether someone else's pull request can be checked out to fix, and offers it.
    GuidedReviewOpenPullRequestFix,
    GuidedReviewConfirmPullRequestFix,
    GuidedReviewCancelPullRequestFix,
    GuidedReviewOpenPullRequest,
    GuidedReviewEditPullRequest(crate::guided_review::OpeningField),
    GuidedReviewSavePullRequestEdit,
    GuidedReviewCancelPullRequestEdit,
    GuidedReviewTogglePullRequestDraft,
    GuidedReviewConfirmPullRequest,
    GuidedReviewCancelPullRequest,

    // === Review: answering and posting on GitHub ===
    GuidedReviewOpenThreadAnswers,
    /// Moves the thread-answers cursor. Args: `(forward)`
    GuidedReviewMoveThreadAnswers(bool),
    GuidedReviewToggleThreadResolve,
    GuidedReviewBeginThreadReply,
    GuidedReviewSaveThreadReply,
    /// Drops the reply being written, keeping the one before it.
    GuidedReviewCancelThreadReply,
    GuidedReviewConfirmThreadAnswers,
    /// Closes the answers preview, unless answers are in flight.
    GuidedReviewCancelThreadAnswers,
    GuidedReviewOpenGithubPost,
    GuidedReviewBeginPostSummary,
    /// Steps the verdict the review will post with. Args: `(forward)`
    GuidedReviewMoveGithubVerdict(bool),
    /// Moves the post preview's cursor to the next or previous thing it will post. Args: `(forward)`
    GuidedReviewMoveGithubPostItem(bool),
    /// Scrolls the text of the thing under the post preview's cursor. Args: `(rows)`.
    GuidedReviewScrollGithubPost(i32),
    /// Posts exactly what the preview shows, or asks GitHub again after a failed read.
    GuidedReviewConfirmGithubPost,
    /// Closes the post preview, unless a post is in flight.
    GuidedReviewCancelGithubPost,
    /// Points the review at one chapter, from a click on its row in the guide.
    GuidedReviewOpenChapter(luxe_core::guided_review::ChapterId),

    // === Paste and desktop ===
    PasteText(String),
    /// Image data received from the clipboard.
    ImagePasted {
        pixels: Vec<u8>,
        width: u32,
        height: u32,
    },
    DesktopThemeChanged,
    /// A model catalog rebuilt off the frame, after a local server was asked what it serves.
    /// Args: `(catalog)`.
    ModelCatalogRefreshed(Vec<crate::model_picker::ModelEntry>),
}

/// What a key or the wheel does to the pull request's description reader.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DescriptionStep {
    /// Opens it at the top, or shuts it
    Toggle,
    /// Scrolls it, down when positive. Args: the rows.
    Scroll(i32),
}
