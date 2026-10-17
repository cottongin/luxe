//! Conversation settings — how the transcript is laid out and what it shows.

use super::ActivityBarStyle;
use super::{known_or, Lenient};
use serde::{Deserialize, Serialize};

/// How model reasoning ("thinking") output is presented in the transcript.
///
/// Reasoning is always recorded on the session; this only controls display. `Collapsed` blocks
/// stay expandable at runtime with the same key that expands a tool card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingDisplay {
    /// Show reasoning inline, in full (muted italic).
    Shown,
    /// Show a one-line peek that can be expanded on demand.
    Collapsed,
    /// Don't render reasoning at all.
    Hidden,
    /// Unknown values fall back to shown.
    #[serde(other)]
    Unknown,
}

impl Lenient for ThinkingDisplay {
    const UNKNOWN: Self = Self::Unknown;
    const DEFAULT: Self = Self::Shown;
    const DEFAULT_NAME: &'static str = "shown";
    const NAME: &'static str = "conversation thinking display";
}

/// Default behavior for collapsed tool activity details.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolDetailsPreference {
    /// Expand failed tool calls only.
    Errors,
    /// Expand every tool call.
    All,
    /// Start every tool call collapsed.
    Collapsed,
    /// Unknown values fall back to failed calls only.
    #[serde(other)]
    Unknown,
}

impl Lenient for ToolDetailsPreference {
    const UNKNOWN: Self = Self::Unknown;
    const DEFAULT: Self = Self::Errors;
    const DEFAULT_NAME: &'static str = "errors";
    const NAME: &'static str = "conversation tool_details";
}

/// Horizontal placement of conversation content within the available width.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationAlignment {
    /// Anchor content to the left; readable width caps still apply.
    Left,
    /// Center content within the available width.
    Center,
    /// Fill the full available width, ignoring the readable prose cap.
    Full,
    /// Unknown values fall back to full width.
    #[serde(other)]
    Unknown,
}

impl Lenient for ConversationAlignment {
    const UNKNOWN: Self = Self::Unknown;
    const DEFAULT: Self = Self::Full;
    const DEFAULT_NAME: &'static str = "full";
    const NAME: &'static str = "conversation alignment";
}

/// How streamed reasoning and prose reach the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamReveal {
    /// Paced against the model's measured rate, with the newest characters fading in.
    Smooth,
    /// Painted the instant it arrives, in whatever lumps the provider sends.
    Off,
    /// Unknown values fall back to smooth.
    #[serde(other)]
    Unknown,
}

impl Lenient for StreamReveal {
    const UNKNOWN: Self = Self::Unknown;
    const DEFAULT: Self = Self::Smooth;
    const DEFAULT_NAME: &'static str = "smooth";
    const NAME: &'static str = "conversation stream_reveal";
}

/// How a run of lookups one message asked for is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolRuns {
    /// One line for the whole run, opened with a click.
    Fold,
    /// A card per call, however many the message asked for.
    Cards,
    /// Unknown values fall back to folding.
    #[serde(other)]
    Unknown,
}

impl Lenient for ToolRuns {
    const UNKNOWN: Self = Self::Unknown;
    const DEFAULT: Self = Self::Fold;
    const DEFAULT_NAME: &'static str = "fold";
    const NAME: &'static str = "conversation tool_runs";
}

/// Vertical growth direction of the transcript when content is shorter than the viewport.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationGrowth {
    /// Anchor content to the top, under the header, growing downward.
    Top,
    /// Anchor content to the bottom, just above the input, growing upward (chat-style default).
    Bottom,
    /// Unknown values fall back to bottom.
    #[serde(other)]
    Unknown,
}

impl Lenient for ConversationGrowth {
    const UNKNOWN: Self = Self::Unknown;
    const DEFAULT: Self = Self::Bottom;
    const DEFAULT_NAME: &'static str = "bottom";
    const NAME: &'static str = "conversation growth";
}

/// Conversation width and tool-detail preferences.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ConversationConfig {
    /// Maximum prose width in terminal cells. Only consulted under a capped [`alignment`].
    ///
    /// [`alignment`]: ConversationAlignment
    pub prose_max_width: Option<u16>,
    /// Maximum wide-block width in terminal cells. Only consulted under a capped alignment.
    pub wide_block_max_width: Option<u16>,
    /// Default expansion behavior for tool details.
    pub tool_details: Option<ToolDetailsPreference>,
    /// Horizontal placement of conversation content; omitted values fill the full width.
    pub alignment: Option<ConversationAlignment>,
    /// Vertical growth when content is shorter than the viewport; omitted values anchor bottom.
    pub growth: Option<ConversationGrowth>,
    /// How model reasoning is displayed; omitted values show reasoning inline in full.
    pub thinking: Option<ThinkingDisplay>,
    /// Lines one wheel notch scrolls the transcript; omitted values scroll three.
    pub scroll_lines: Option<u16>,
    /// The activity bar's look; omitted values use `bars`.
    pub activity_bar: Option<ActivityBarStyle>,
    /// Rows an inline image may occupy before it is scaled down to fit. Omitted = 24.
    pub image_max_rows: Option<u16>,
    /// How streamed reasoning and prose land on screen. Omitted = `smooth`.
    pub stream_reveal: Option<StreamReveal>,
    /// How a run of lookups one message asked for is drawn. Omitted = `fold`.
    pub tool_runs: Option<ToolRuns>,
}

impl ConversationConfig {
    /// Resolves the prose width, using 120 cells outside the supported 72–120 range.
    pub fn prose_max_width(&self) -> u16 {
        self.prose_max_width.filter(|width| (72..=120).contains(width)).unwrap_or(120)
    }

    /// Resolves the wide-block width, using 140 cells outside the supported 72–140 range.
    pub fn wide_block_max_width(&self) -> u16 {
        self.wide_block_max_width.filter(|width| (72..=140).contains(width)).unwrap_or(140)
    }

    /// Lines one wheel notch moves the transcript.
    ///
    /// Three by default: one line per notch makes paging a long conversation a chore, and terminals
    /// send one notch per physical click. Capped at 20 so a bad value can't teleport the view.
    pub fn scroll_lines(&self) -> usize {
        usize::from(self.scroll_lines.filter(|lines| (1..=20).contains(lines)).unwrap_or(3))
    }

    /// Rows an inline image may occupy in the transcript.
    ///
    /// Twenty-four by default: tall enough to read a dev-server screenshot at a glance, short
    /// enough that a burst of them still leaves conversation on screen. The floor of 3 keeps a
    /// misconfiguration from producing a one-row smear, and the ceiling is the protocol's own.
    pub fn image_max_rows(&self) -> u16 {
        self.image_max_rows.filter(|rows| (3..=200).contains(rows)).unwrap_or(24)
    }

    /// Resolves the content alignment.
    ///
    /// Defaults to `full`: capping prose at 120 cells left ultrawide terminals mostly empty, and
    /// the people running them chose that width on purpose. `left`/`center` keep the readable cap
    /// for anyone who prefers it.
    pub fn alignment(&self) -> ConversationAlignment {
        known_or(self.alignment)
    }

    /// Resolves the growth direction, safely defaulting unknown/omitted values to bottom-up.
    pub fn growth(&self) -> ConversationGrowth {
        known_or(self.growth)
    }

    /// Resolves how reasoning is displayed, safely defaulting unknown/omitted values to shown.
    pub fn thinking(&self) -> ThinkingDisplay {
        known_or(self.thinking)
    }

    /// Whether streamed reasoning and prose are paced onto the screen.
    ///
    /// On by default: a provider's delivery is nothing like its production — one measured at ~75
    /// characters every ~470 ms — so painting each arrival as it lands is what makes a fast model
    /// look choppy. `off` restores exactly that, for anyone who wants the raw wire.
    pub fn stream_reveal(&self) -> StreamReveal {
        known_or(self.stream_reveal)
    }

    /// Whether a run of lookups collapses into the one line that stands for it.
    ///
    /// Folded by default: a message asking for seven `read` calls fills the screen with headers in
    /// half a second, and the conversation they belonged to goes off the top. `cards` keeps every
    /// call on its own card for anyone who would rather watch each one land.
    pub fn tool_runs(&self) -> ToolRuns {
        known_or(self.tool_runs)
    }

    /// Resolves the default tool-detail behavior, safely falling back for unknown values.
    pub fn tool_details(&self) -> ToolDetailsPreference {
        known_or(self.tool_details)
    }
}
