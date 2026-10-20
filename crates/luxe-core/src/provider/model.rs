//! Model — which model a request goes to, what it can do, and how its provider speaks.

use super::*;

/// Model reference — what the agent is speaking through.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModelRef {
    /// Provider name (e.g. `openai`, `anthropic`, `gemini`).
    pub provider: String,
    /// Model identifier (e.g. `gpt-4o`, `claude-sonnet-4-20250514`).
    pub id: String,
}

impl ModelRef {
    /// Convenience: build a reference.
    pub fn new(provider: impl Into<String>, id: impl Into<String>) -> Self {
        Self { provider: provider.into(), id: id.into() }
    }

    /// Whether this names no model at all.
    ///
    /// luxe has no default model — model ids age out faster than releases, so a literal in the
    /// binary is a guess that goes stale and then gets billed. With nothing chosen the front-end
    /// resolves an **empty id**, which no provider serves, and the engine refuses the turn
    /// ([`crate::engine::EngineError::NoModel`]).
    ///
    /// A representable "unchosen" rather than an `Option` through the engine: context occupancy and
    /// compaction are arithmetic over a model's window, and a `None` there has to be guessed at —
    /// which is the class of bug where a wrong zero reports a full session as empty.
    pub fn is_unset(&self) -> bool {
        self.id.trim().is_empty()
    }
}

impl fmt::Display for ModelRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.provider, self.id)
    }
}

/// Input modality.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Modality {
    Text,
    Image,
}

/// Provider-neutral reasoning/thinking level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingLevel {
    #[default]
    Off,
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
    /// Maximum available reasoning. Providers commonly call this `max`.
    #[serde(alias = "max")]
    Ultra,
}

impl ThinkingLevel {
    /// Levels ordered from least to most reasoning.
    pub const ALL: [Self; 7] =
        [Self::Off, Self::Minimal, Self::Low, Self::Medium, Self::High, Self::XHigh, Self::Ultra];

    /// Returns luxe's stable configuration and display name for this level.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Ultra => "ultra",
        }
    }
}

/// The reasoning dials a model exposes, as declared by its catalog entry.
///
/// Not cosmetic: Anthropic's generations disagree on how thinking is requested. A model that only
/// advertises discrete effort levels rejects `thinking: {type: "enabled", budget_tokens: N}` with a
/// 400 and has to be steered with `thinking: {type: "adaptive"}` + `output_config.effort`, while
/// Claude 4.5 and earlier accept only the token budget. All-false means nothing told us, which
/// leaves each adapter on its historical default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThinkingControls {
    /// Discrete effort levels (`low` … `max`)
    #[serde(default)]
    pub effort: bool,
    /// A caller-chosen thinking-token budget
    #[serde(default)]
    pub budget_tokens: bool,
    /// Whether reasoning can be switched off at all.
    ///
    /// False also means "nothing said", and the two have to be treated alike: a fifth of
    /// OpenRouter's catalog answers 400 to every way of asking it to stop thinking, and a
    /// third of those declare no dials for the catalog to read.
    #[serde(default)]
    pub can_disable: bool,
}

impl ThinkingControls {
    /// Whether effort is the only dial, i.e. a token budget is not on the table.
    pub fn effort_only(&self) -> bool {
        self.effort && !self.budget_tokens
    }
}

/// Compatibility flags for provider/model quirks — how a request has to be *shaped* for an
/// endpoint to accept it.
///
/// Lives here, beside [`ThinkingControls`] and [`ModelInfo::sampling_params`], for one reason: a
/// [`ModelInfo`] is the only metadata that travels per request, and these differ per *model* as
/// often as per provider. One local server hosting several builds is the ordinary case — two chat
/// templates on one port, one handling a `developer` role and the other raising on it. Nothing here
/// performs I/O; an adapter reads it and decides what to write.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompatFlags {
    /// Whether the provider supports the `developer` role.
    #[serde(default)]
    pub supports_developer_role: bool,
    /// Whether the provider supports `reasoning_effort`.
    #[serde(default)]
    pub supports_reasoning_effort: bool,
    /// Max tokens field name (`max_tokens` or `max_completion_tokens`).
    #[serde(default = "default_max_tokens_field")]
    pub max_tokens_field: String,
    /// Thinking format (`openai`, `qwen`, `deepseek`, `openrouter`, `together`, `chat_template`).
    #[serde(default)]
    pub thinking_format: Option<String>,
    /// Payload for `chat_template_kwargs` under the chat-template thinking formats: a map of
    /// literal values and `{"$var": "thinking.enabled"|"thinking.level"}` references resolved per
    /// request, with `"omit_when_off": true` to drop a key when the turn doesn't think. Empty
    /// sends Qwen's `enable_thinking` + `preserve_thinking` pair, which is what luxe always sent.
    #[serde(default)]
    pub chat_template_kwargs: serde_json::Map<String, serde_json::Value>,
    /// Cache control format.
    #[serde(default)]
    pub cache_control_format: Option<String>,
    /// Ask Anthropic to keep this session's cache for an hour instead of five minutes.
    ///
    /// Off by default because it is a trade rather than a free win, and because not every
    /// Anthropic-compatible endpoint takes the field. Writes cost 2x base input instead of 1.25x,
    /// which on a turn's small delta is fractions of a cent — against re-writing the *whole*
    /// conversation every time the user steps away for six minutes. Worth it for interactive work,
    /// not for an unattended burst that never idles.
    #[serde(default)]
    pub supports_long_cache_retention: bool,
    /// Whether the provider requires tool result names.
    #[serde(default)]
    pub requires_tool_result_name: bool,
    /// Whether the provider reports token usage in a streaming response. On by default:
    /// luxe sends `stream_options: {include_usage: true}` on OpenAI-compatible requests so
    /// token/cost display, context-window fullness, and compaction thresholds actually work.
    /// Standard and widely supported (OpenAI, LM Studio, vLLM, llama.cpp, DashScope, …); set
    /// `false` only for a strict server that rejects the field.
    #[serde(default = "default_true")]
    pub supports_usage_in_streaming: bool,
    /// Force adaptive thinking mode (Anthropic).
    #[serde(default)]
    pub force_adaptive_thinking: bool,
    /// Ask Anthropic to stream a `write` call's arguments as the model writes them.
    ///
    /// On by default, and it is the difference between watching a file being written and staring
    /// at a card that says nothing for twelve seconds. Anthropic buffers tool input server-side
    /// unless asked not to: measured against `claude-sonnet-4-5`, **0 of 55** TCP reads landed in
    /// the middle half of the response without this, and 103 of 200 with it. The Azure Foundry
    /// passthrough honors it too.
    ///
    /// It is asked per tool, `eager_input_streaming` on `write` alone
    /// ([`crate::activity::streams_arguments`]), because what it trades is Anthropic's check that
    /// the arguments are valid JSON. Claude models garble `edit`'s nested arguments either way, but
    /// only a validated garble arrives in a shape `edit` can read back. Set `false` for an
    /// Anthropic-compatible endpoint that rejects the field.
    #[serde(default = "default_true")]
    pub stream_tool_arguments: bool,
    /// Load a deferred tool where `activate_tools` asked for it, rather than by adding it to the
    /// request's tool list.
    ///
    /// On by default because the list sits at the very front of the cached prompt: growing it
    /// mid-session re-bills the whole conversation at the cache-write rate, where loading in place
    /// (Anthropic's `defer_loading` and `tool_reference`, the Responses API's `additional_tools`)
    /// costs about the size of the schema. An endpoint that refuses either is noticed from its 400
    /// and dropped back to the list for the rest of the session, so `false` is only for one that
    /// mishandles them without saying so. Chat completions has no way to do it and ignores this.
    #[serde(default = "default_true")]
    pub defer_tool_loading: bool,
    /// Use the OpenAI Responses API (`/v1/responses`) instead of chat/completions.
    /// Required for newer models such as `gpt-5` and `gpt-5-codex`.
    ///
    /// Provider-level only: it picks the *transport*, and an adapter is built once per provider, so
    /// a model entry that sets it is refused at resolution rather than half-honored.
    #[serde(default)]
    pub use_responses_api: bool,
    /// Send GitHub Copilot's per-request editor headers (`X-Initiator`, `Openai-Intent`, and a
    /// vision flag when the turn carries images). Copilot's endpoint expects them from a chat
    /// client; every other provider ignores them, so this stays opt-in.
    #[serde(default)]
    pub copilot_headers: bool,
    /// Name luxe to OpenRouter with the `HTTP-Referer` / `X-Title` headers it publishes for app
    /// attribution. They carry nothing about the user or the turn — OpenRouter already has the
    /// key, the model and the prompt — and set this to `false` to drop them anyway.
    #[serde(default)]
    pub openrouter_headers: bool,
}

fn default_max_tokens_field() -> String {
    "max_tokens".to_string()
}

fn default_true() -> bool {
    true
}

impl Default for CompatFlags {
    /// Conservative defaults except `supports_usage_in_streaming`, `stream_tool_arguments` and
    /// `defer_tool_loading`, which are on so usage/cost/context accounting, a live tool card and a
    /// cache that survives a tool activation work out of the box.
    /// Kept in sync with the field-level serde defaults so a code-constructed default matches a
    /// deserialized empty config.
    fn default() -> Self {
        Self {
            supports_developer_role: false,
            supports_reasoning_effort: false,
            max_tokens_field: default_max_tokens_field(),
            thinking_format: None,
            chat_template_kwargs: serde_json::Map::new(),
            cache_control_format: None,
            supports_long_cache_retention: false,
            requires_tool_result_name: false,
            supports_usage_in_streaming: true,
            force_adaptive_thinking: false,
            stream_tool_arguments: true,
            defer_tool_loading: true,
            use_responses_api: false,
            copilot_headers: false,
            openrouter_headers: false,
        }
    }
}

/// Flags a model entry may not set, because they choose the *transport* rather than the shape of a
/// request — and the adapter that speaks it is built once per provider.
pub const PROVIDER_ONLY_COMPAT_KEYS: &[&str] = &["use_responses_api"];

/// Information about a model (from models.dev or explicit config).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelInfo {
    /// Model reference.
    pub r#ref: ModelRef,
    /// Human-friendly name for the UI (statusline + `/model` picker), overriding the raw
    /// `ref.id`. Cosmetic only — the provider still receives `ref.id` on the wire.
    #[serde(default)]
    pub display_name: Option<String>,
    /// Context window size in tokens.
    pub context_window: u32,
    /// Max output tokens.
    pub max_tokens: u32,
    /// Whether the model supports reasoning/thinking.
    pub reasoning: bool,
    /// luxe level to provider/model value; `None` marks an unsupported level.
    #[serde(default)]
    pub thinking_level_map: HashMap<String, Option<String>>,
    /// Reasoning level a session starts at on this model (`models.json`'s `thinking`).
    ///
    /// Only a user's config sets this — models.dev publishes no such opinion — and it exists
    /// because the remembered level is one value for every model: a level that is cheap on a
    /// frontier model can be minutes of restating the question on a local one. A ceiling is a
    /// different claim and belongs in [`Self::thinking_level_map`] as a `null`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_thinking: Option<ThinkingLevel>,
    /// Sampling parameters merged verbatim into an OpenAI-shaped request body, after the fields
    /// the adapter sets itself — so a model card's recipe is the last word on how it is sampled.
    /// Carries the samplers the OpenAI schema has no field for (`top_k`, `min_p`,
    /// `repeat_penalty`), whose server-side defaults are not neutral. Ignored by Anthropic.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub sampling_params: serde_json::Map<String, serde_json::Value>,
    /// Reasoning dials the model declares (effort levels, a token budget, or neither).
    #[serde(default)]
    pub thinking_controls: ThinkingControls,
    /// Request-shaping quirks this model needs, when they differ from its provider's.
    ///
    /// `None` — the usual case — means "nothing model-specific", and an adapter keeps the flags it
    /// was built with. `Some` is the fully resolved set for this model (the provider's flags with
    /// the model's own keys merged over them), which is what makes one endpoint able to serve two
    /// builds whose chat templates disagree. Never set by a catalog: models.dev has no opinion about
    /// a local server's templates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compat: Option<CompatFlags>,
    /// Supported input modalities.
    pub input: Vec<Modality>,
    /// Cost per million tokens.
    pub cost: Cost,
    /// Whether those rates are a charge or a plan-equivalent.
    #[serde(default)]
    pub billing: Billing,
}

impl Default for ModelInfo {
    fn default() -> Self {
        Self {
            r#ref: ModelRef::new("unknown", "unknown"),
            display_name: None,
            context_window: 128_000,
            max_tokens: 16_000,
            reasoning: false,
            thinking_level_map: HashMap::new(),
            default_thinking: None,
            sampling_params: serde_json::Map::new(),
            thinking_controls: ThinkingControls::default(),
            compat: None,
            input: vec![Modality::Text],
            cost: Cost::default(),
            billing: Billing::default(),
        }
    }
}

impl ModelInfo {
    /// The name to show in the UI: the `display_name` override when set, else the raw model id.
    pub fn display(&self) -> &str {
        self.display_name.as_deref().unwrap_or(&self.r#ref.id)
    }

    /// Resolves a requested level to one this model supports.
    ///
    /// Explicit `null` mappings disable a level. `xhigh` and `ultra` are opt-in because
    /// providers vary most at the top end; other levels use their luxe names by default.
    /// Unsupported levels clamp upward first, then downward, matching Pi's behavior.
    pub fn effective_thinking_level(&self, requested: ThinkingLevel) -> ThinkingLevel {
        if !self.reasoning {
            return ThinkingLevel::Off;
        }
        if self.supports_thinking_level(requested) {
            return requested;
        }

        let requested_index =
            ThinkingLevel::ALL.iter().position(|level| *level == requested).unwrap_or_default();
        ThinkingLevel::ALL[requested_index..]
            .iter()
            .chain(ThinkingLevel::ALL[..requested_index].iter().rev())
            .copied()
            .find(|level| self.supports_thinking_level(*level))
            .unwrap_or(ThinkingLevel::Off)
    }

    /// Returns an explicit provider value for a level, if configured.
    ///
    /// `ultra` also accepts `max` as a config alias so Pi model maps can be reused.
    pub fn thinking_level_value(&self, level: ThinkingLevel) -> Option<&str> {
        self.thinking_level_entry(level).and_then(|value| value.as_deref())
    }

    fn supports_thinking_level(&self, level: ThinkingLevel) -> bool {
        match self.thinking_level_entry(level) {
            Some(value) => value.is_some(),
            None => !matches!(level, ThinkingLevel::XHigh | ThinkingLevel::Ultra),
        }
    }

    fn thinking_level_entry(&self, level: ThinkingLevel) -> Option<&Option<String>> {
        if level == ThinkingLevel::Ultra {
            self.thinking_level_map.get("ultra").or_else(|| self.thinking_level_map.get("max"))
        } else {
            self.thinking_level_map.get(level.as_str())
        }
    }
}
