//! Fixture scenarios — the stable names screenshots, recordings and checks ask for.

use std::str::FromStr;

/// Declares the scenarios once: the enum, [`FixtureScenario::ALL`] and each one's name come out of
/// one list, so a scenario cannot be added to one of them and left out of another.
macro_rules! scenarios {
    ($($(#[doc = $doc:literal])* $variant:ident => $name:literal,)+) => {
        /// Stable screenshot scenario names supported by the fixture runner.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum FixtureScenario {
            $($(#[doc = $doc])* $variant,)+
        }

        impl FixtureScenario {
            /// Every scenario, so a check can walk all of them rather than the ones it
            /// remembers.
            pub const ALL: &'static [FixtureScenario] = &[$(Self::$variant,)+];

            /// The name the capture scripts, the runner's title and its usage line all spell it
            /// with.
            pub fn name(self) -> &'static str {
                match self {
                    $(Self::$variant => $name,)+
                }
            }
        }
    };
}

scenarios! {
    /// Startup dashboard with fixed sessions and model metadata.
    Dashboard => "dashboard",
    /// The context-file list, open over the dashboard.
    DashboardContextFiles => "dashboard-context-files",
    /// The dashboard with a draft typed, listing what it matches.
    DashboardTyping => "dashboard-typing",
    /// The dashboard's minimal style: the prompt, the list folded away.
    DashboardMinimal => "dashboard-minimal",
    /// The Configuration surface, as a machine with nothing configured opens it.
    Configuration => "configuration",
    /// The first run's setup, on its theme step.
    Setup => "setup",
    /// The first run's setup, on its tools step.
    SetupTools => "setup-tools",
    /// The settings pages, on Appearance, over a config with a few keys set.
    Settings => "settings",
    /// The settings pages, on Tools, with one tool turned off here and one by the project.
    SettingsTools => "settings-tools",
    /// Idle chat with a fixed short exchange.
    ChatIdle => "chat-idle",
    /// Chat showing model reasoning: one expanded and one collapsed thinking block.
    ChatThinking => "chat-thinking",
    /// Chat caught mid-reveal: a streaming answer with its newest characters still fading in.
    ChatStreaming => "chat-streaming",
    /// Chat with deterministic structured tool outcomes.
    ChatTools => "chat-tools",
    /// Chat with the three cards that carry a body: a written file, a side-by-side edit, a heredoc.
    ChatCardBodies => "chat-card-bodies",
    /// Chat with prose, fenced code, and a table for canvas-width review.
    ChatWideProse => "chat-wide-prose",
    /// Chat with an expanded failed tool for error-state review.
    ChatToolError => "chat-tool-error",
    /// A run of lookups folded into one live line, mid-batch, beside the cards that never fold.
    ChatToolRun => "chat-tool-run",
    /// Chat with the interactive question modal open (multi-select + free-text).
    ChatQuestion => "chat-question",
    /// Plan mode, mid-interview: the composer and the agent's question in the plan color.
    ChatPlan => "chat-plan",
    /// Plan mode's plan written, and the question of what to do with it.
    ChatPlanReady => "chat-plan-ready",
    /// A finished turn: lookups folded into a line, an edit with its diff, a test run, the reply.
    ChatTurn => "chat-turn",
    /// The same turn, stopped on a commit that waits for approval.
    ChatPermission => "chat-permission",
    /// The Command Palette over the conversation, the commands used last on top.
    ChatPalette => "chat-palette",
    /// A `screenshot` card showing its capture inline, at whatever fidelity the terminal has.
    ChatScreenshot => "chat-screenshot",
    /// Guided Review source failure rendered through the production event reducer.
    GuidedReviewError => "guided-review-error",
    /// Accepted Guided Review rendered through the production event reducer.
    GuidedReviewReady => "guided-review-ready",
    /// Responsive wide Guided Review capture using the standard review corpus.
    GuidedReviewWide => "guided-review-wide",
    /// The demos' review: a change large enough to need the chapters it is shown in.
    GuidedReviewDemo => "guided-review-demo",
    /// The demos' review with the guide pane put away, so the diff has the screen.
    GuidedReviewGuideHidden => "guided-review-guide-hidden",
    /// The working-tree review, open on its brief.
    GuidedReviewBrief => "guided-review-brief",
    /// The working-tree review with its send preview open on the seeded comments.
    GuidedReviewSend => "guided-review-send",
    /// The working-tree review with its commit preview open.
    GuidedReviewCommit => "guided-review-commit",
    /// A generated review large enough to exercise scrolling, folding and scale.
    GuidedReviewLarge => "guided-review-large",
    /// The demos' change, arriving as a pull request opened by a bot.
    GuidedReviewPullRequest => "guided-review-pull-request",
    /// That pull request's review in the preview before it is posted to GitHub.
    GuidedReviewPullRequestPost => "guided-review-pull-request-post",
    /// Your own pull request's review, answering its threads on GitHub.
    GuidedReviewOwnPullRequest => "guided-review-own-pull-request",
    /// A branch's review, its pull request drafted in the preview before it opens.
    GuidedReviewOpenPullRequest => "guided-review-open-pull-request",
    /// Someone else's pull request, offered to be fixed in a worktree with its comments.
    GuidedReviewFixPullRequest => "guided-review-fix-pull-request",
    /// A branch's review, drafted as a stack of pull requests before it opens.
    GuidedReviewOpenStack => "guided-review-open-stack",
    /// A comment the agent claims to have fixed and the reviewer's answer, open in its details.
    GuidedReviewThread => "guided-review-thread",
    /// Guided Review with the empty Findings drawer open.
    GuidedReviewFindingsEmpty => "guided-review-findings-empty",
    /// Guided Review with persisted findings ordered for triage.
    GuidedReviewFindings => "guided-review-findings",
    /// Guided Review with a remapped finding awaiting explicit verification.
    GuidedReviewNeedsVerification => "guided-review-needs-verification",
    /// Guided Review with the blocking-accept confirmation visible.
    GuidedReviewFinish => "guided-review-finish",
    /// Responsive medium Guided Review capture using the standard review corpus.
    GuidedReviewMedium => "guided-review-medium",
    /// Responsive narrow Guided Review capture using the standard review corpus.
    GuidedReviewNarrow => "guided-review-narrow",
    /// Accepted Guided Review with split presentation preferred for wide captures.
    GuidedReviewSplit => "guided-review-split",
    /// Guided Review with an initially folded formatting hunk.
    GuidedReviewNoise => "guided-review-noise",
    /// In-flight Guided Review refresh rendered through the production event reducer.
    GuidedReviewRefreshing => "guided-review-refreshing",
    /// Find-session picker over the dashboard with a deterministic session catalog.
    SessionPicker => "session-picker",
    /// Running turn with mid-turn submissions pinned above the composer.
    ChatPending => "chat-pending",
    /// A submitted message carrying a paste long enough to be previewed.
    ChatPaste => "chat-paste",
    /// Wide side-by-side Context Dock with deterministic workspace data.
    DockWide => "dock-wide",
    /// The wide dock on a branch whose pull request has a failing check and open threads.
    DockPullRequest => "dock-pull-request",
    /// Standard-width Context overlay with deterministic workspace data.
    DockOverlay => "dock-overlay",
    /// Compact Context replacement with deterministic workspace data.
    DockCompact => "dock-compact",
    /// One file's diff beside the dock that opened it.
    ChangesWide => "changes-wide",
    /// The same pane with the dock away and the statusline carrying the change instead.
    ChangesNarrow => "changes-narrow",
    /// The pane on a file that could not be captured.
    ChangesFailed => "changes-failed",
    /// The pane on a file this session changed outside the project, beside the rail that opened it.
    ChangesExternal => "changes-external",
}

impl FromStr for FixtureScenario {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .iter()
            .copied()
            .find(|scenario| scenario.name() == value)
            .ok_or_else(|| format!("unknown fixture scenario: {value}"))
    }
}
