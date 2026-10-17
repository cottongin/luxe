//! Request — one completion request, and the events its stream answers with.

use super::*;

/// Stop reason from a completed LLM turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// The model produced an end-of-turn token.
    EndTurn,
    /// The model emitted a tool call.
    ToolCall,
    /// Token limit reached.
    Length,
    /// Content filter triggered.
    ContentFilter,
    /// A non-LLM reason (e.g. user cancel, timeout).
    Other(String),
}

/// A tool schema (JSON Schema) for function calling.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSchema {
    pub name: String,
    pub description: String,
    /// JSON Schema for the tool's parameters.
    pub parameters: serde_json::Value,
}

/// Whether anything will ever read this request's prefix back.
///
/// A cache write costs about 1.25x what the same tokens cost as fresh input and pays for itself the
/// first time it is read. A prompt that is built, sent once and thrown away never gets that far, so
/// asking for a breakpoint on one is a surcharge for nothing — which is why the summarizer and the
/// review's guide pass declare themselves. Pi's compaction disables cache writes for the same
/// reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PromptReuse {
    /// A turn in a conversation the next request will extend.
    #[default]
    Conversation,
    /// Assembled for this one call, and never sent again.
    OneOff,
}

impl PromptReuse {
    /// Whether a cache breakpoint on this prompt could ever pay for itself.
    pub fn worth_caching(self) -> bool {
        matches!(self, Self::Conversation)
    }
}

/// Completion request to a provider.
#[derive(Debug, Clone, Default)]
pub struct CompletionRequest {
    pub model: ModelInfo,
    pub messages: Vec<Message>,
    pub system: Option<String>,
    pub tools: Vec<ToolSchema>,
    /// Every tool the model may load mid-session through `activate_tools`, activated or not.
    ///
    /// `tools` already carries the activated ones, which is all an adapter needs when the only way
    /// it has to add a tool is to grow that list. Growing it moves the start of every cached
    /// prefix, though, so an adapter that can load a tool *where the activation happened*
    /// (Anthropic's `tool_reference`, OpenAI's `additional_tools`) takes these names out of `tools`
    /// and loads them in place instead, and `tools` stays byte-identical all session. Empty unless
    /// lazy tools are on, and on any pass that withholds tools.
    pub deferred_tools: Vec<ToolSchema>,
    pub thinking: ThinkingLevel,
    pub max_tokens: Option<u32>,
    /// Whether a later request will share this one's prefix.
    pub reuse: PromptReuse,
    /// Bytes at the start of `system` that every session of this build shares: the harness and
    /// its tool guidance, before anything about the project, the directory or the session. An
    /// adapter that caches by explicit breakpoint can end one there, so a session somewhere else
    /// still reads that much from cache. `0` when unknown.
    pub shared_system_len: usize,
}

/// Events emitted by a provider during streaming.
#[derive(Debug, Clone)]
pub enum ProviderEvent {
    /// Stream started.
    Start,
    /// Text generation started at index.
    TextStart { index: usize },
    /// Text delta at index.
    TextDelta { index: usize, delta: String },
    /// Text generation ended at index.
    TextEnd { index: usize },
    /// Thinking/reasoning started at index.
    ThinkingStart { index: usize },
    /// Thinking delta at index.
    ThinkingDelta { index: usize, delta: String },
    /// The provider's opaque signature for the thinking block at index (Anthropic
    /// `signature_delta`). Must be preserved and replayed unmodified.
    ThinkingSignature { index: usize, signature: String },
    /// OpenAI Responses reasoning-item metadata at index: the item `id` (`rs_...`) and its
    /// `encrypted_content` (present when requested via `include`). Replayed verbatim.
    ReasoningItem { index: usize, id: Option<String>, encrypted: Option<String> },
    /// GitHub Copilot's `reasoning_opaque` at index: the model's reasoning state on chat
    /// completions, replayed verbatim on the assistant message it came with.
    ReasoningOpaque { index: usize, opaque: String },
    /// An encrypted (redacted) reasoning block at index, carrying opaque `data` instead of text.
    RedactedThinking { index: usize, data: String },
    /// Thinking ended at index.
    ThinkingEnd { index: usize },
    /// Tool call started.
    ToolCallStart { index: usize, id: String, name: String },
    /// Tool call JSON delta.
    ToolCallDelta { index: usize, json_delta: String },
    /// Tool call ended.
    ToolCallEnd { index: usize },
    /// Usage statistics.
    Usage(Usage),
    /// How much of a subscription's allowance this account has used, as the provider reported it.
    PlanUsage(PlanUsage),
    /// Stream done with stop reason.
    Done { reason: StopReason },
    /// Error.
    Error { message: String, kind: ProviderErrorKind },
}
