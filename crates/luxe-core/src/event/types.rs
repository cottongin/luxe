//! Event types — the state, identity and failure shapes `Action` and `Event` carry.

use crate::guided_review::{GuidedReviewError, ReviewSourceUnavailable};
use crate::message::Message;
use crate::session::EntryId;
use serde::{Deserialize, Serialize};

/// Model reference — what the agent is speaking through.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelRef {
    /// Provider name (e.g. `openai`, `anthropic`, `gemini`).
    pub provider: String,
    /// Model identifier (e.g. `gpt-4o`, `claude-sonnet-4-20250514`).
    pub model: String,
    /// Optional human-friendly name for the UI, overriding `model` in display. Cosmetic only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Optional override for the API base URL (e.g. a local server).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
}

impl ModelRef {
    /// Convenience: build a reference with no base-url override.
    pub fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self { provider: provider.into(), model: model.into(), display_name: None, base_url: None }
    }

    /// Full builder-style construction.
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = Some(url.into());
        self
    }

    /// Attach an optional UI display name (builder-style).
    pub fn with_display(mut self, display_name: Option<String>) -> Self {
        self.display_name = display_name;
        self
    }

    /// The name to show in the UI: the `display_name` override when set, else the raw id.
    pub fn display(&self) -> &str {
        self.display_name.as_deref().unwrap_or(&self.model)
    }
}

/// One model a client can switch to, as `list_models` answers. Carries no endpoint: a base URL can
/// hold credentials, and switching needs only the two ids.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelChoice {
    /// Provider id, as `select_model` takes it
    pub provider: String,
    /// Model id, as `select_model` takes it
    pub model: String,
    /// The name to show
    pub name: String,
    /// The thinking levels it honors, lowest first: only `off` for a model that doesn't reason
    pub thinking_levels: Vec<crate::provider::ThinkingLevel>,
}

/// Stop reason from a completed LLM turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// The model produced an end-of-turn token (e.g. `<|end|>`).
    EndTurn,
    /// The model emitted a tool call and the loop will dispatch it.
    ToolCall,
    /// A non-LLM reason (e.g. user cancel, timeout).
    Other(String),
}

/// Engine state snapshot — what the UI should render at this moment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineState {
    /// Current active model.
    pub model: ModelRef,
    /// Whether a turn is currently in flight.
    pub streaming: bool,
    /// Human-readable status message (e.g. "running…", "waiting").
    pub status: String,
    /// The mode the session is in (`build`, `plan`).
    #[serde(default = "default_mode_name")]
    pub mode: String,
    /// Current model thinking level.
    #[serde(default)]
    pub thinking: crate::provider::ThinkingLevel,
}

impl Default for EngineState {
    fn default() -> Self {
        Self {
            model: ModelRef::new("unknown", "unknown"),
            streaming: false,
            status: "idle".into(),
            mode: crate::mode::BUILD.into(),
            thinking: crate::provider::ThinkingLevel::Off,
        }
    }
}

fn default_mode_name() -> String {
    crate::mode::BUILD.into()
}

/// What a requested review concluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestedReviewVerdict {
    /// Nothing worth holding the work for
    Continue,
    /// Findings, which went back to the agent as a turn
    Fix,
    /// No usable review came back; the findings say why
    Unavailable,
}

/// One persisted transcript message paired with its session entry identity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionMessage {
    /// Persisted entry which owns this message.
    pub entry_id: EntryId,
    /// Message content stored in that entry.
    pub message: Message,
}

/// A bounded, serializable failure from immutable Guided Review capture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuidedReviewFailure {
    /// Broad source category suitable for UI recovery guidance.
    pub kind: GuidedReviewFailureKind,
    /// Display-safe, bounded diagnostic detail.
    pub message: String,
}

impl GuidedReviewFailure {
    /// Converts a source error without exposing raw diffs or unbounded subprocess output.
    pub fn from_error(error: &GuidedReviewError) -> Self {
        let kind = match error {
            GuidedReviewError::Unavailable(ReviewSourceUnavailable::MissingTurnBaseline)
            | GuidedReviewError::Unavailable(ReviewSourceUnavailable::ExcludedUntracked {
                ..
            }) => GuidedReviewFailureKind::Unavailable,
            GuidedReviewError::InvalidReference { .. } => GuidedReviewFailureKind::InvalidTarget,
            _ => GuidedReviewFailureKind::Capture,
        };
        let message = error
            .to_string()
            .chars()
            .filter(|character| !character.is_control() || matches!(character, '\n' | '\t'))
            .take(512)
            .collect::<String>();
        Self { kind, message }
    }

    /// The requested target cannot produce a source at all — no retained baseline, no configured
    /// project root. Unlike [`Self::capture`], retrying the same request will not help.
    pub fn unavailable(message: impl Into<String>) -> Self {
        Self { kind: GuidedReviewFailureKind::Unavailable, message: message.into() }
    }

    /// Capture, parsing, or persistence failed on a target that was otherwise reachable.
    pub fn capture(message: impl Into<String>) -> Self {
        Self { kind: GuidedReviewFailureKind::Capture, message: message.into() }
    }
}

/// Broad category for a Guided Review load failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuidedReviewFailureKind {
    /// The requested target cannot produce a complete source.
    Unavailable,
    /// The requested revision or ref was invalid locally.
    InvalidTarget,
    /// Local capture, parsing, or persistence failed.
    Capture,
}

/// The answer to `show_open_review`: the review open now and its session, or neither.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpenReview {
    /// Correlates this answer with its request
    pub request_id: crate::guided_review::ReviewLoadRequestId,
    /// The open review, with the latest guide applied
    pub review: Option<crate::guided_review::GuidedReview>,
    /// Its feedback session as the desk holds it
    pub session: Option<crate::guided_review::ReviewSession>,
    /// What each chapter asks of the reader, decided from lines a client need not keep
    #[serde(default)]
    pub reading: Vec<crate::guided_review::ChapterReading>,
}
