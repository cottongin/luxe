//! Behavior settings — the agent, the gate, plan mode, the reviewer, compaction and tools.

use serde::{Deserialize, Serialize};

/// Tool enablement preferences.
///
/// A deny-list: every built-in tool is enabled unless named here, so a new release's tools stay
/// on by default. The truly essential tools (`read`/`write`/`edit`/`bash`) are never disabled even
/// if listed — the agent can't function without them. Written by `luxe init`; example:
/// `{ "disabled": ["screenshot", "web_search"] }`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ToolsConfig {
    /// Names of built-in tools to leave unregistered.
    pub disabled: Vec<String>,
    /// Whether an edited file goes through its formatter. On when absent.
    pub format_on_save: Option<bool>,
    /// Whether `web_fetch` clones a GitHub repository URL into `.luxe/gh-cache/` rather than
    /// reading it as a page. On when absent.
    pub clone_repositories: Option<bool>,
}

/// Built-in tools that can never be disabled — the agent's irreducible core.
pub const ESSENTIAL_TOOLS: [&str; 4] = ["read", "write", "edit", "bash"];

impl ToolsConfig {
    /// Whether `web_fetch` clones a GitHub repository URL (default: yes).
    pub fn clone_repositories(&self) -> bool {
        self.clone_repositories.unwrap_or(true)
    }

    /// Whether a tool is enabled (not on the deny-list, or essential regardless).
    pub fn is_enabled(&self, name: &str) -> bool {
        ESSENTIAL_TOOLS.contains(&name) || !self.disabled.iter().any(|d| d == name)
    }

    /// The deny-list for one run: config, plus `disable`, minus `enable`.
    ///
    /// Config is the standing preference and the flags are this session's exception, which is why
    /// `enable` can lift a config entry — someone who disabled `screenshot` on principle still
    /// needs it on the afternoon they are doing UI work, and editing a file to get through the
    /// afternoon is how a standing preference stops being one.
    ///
    /// Essential tools are not filtered here: the caller reports each refusal by name, and a
    /// silent drop is how a user comes to believe `--no-tool read` worked.
    pub fn effective_disabled(&self, disable: &[String], enable: &[String]) -> Vec<String> {
        let mut out: Vec<String> = self.disabled.clone();
        for name in disable {
            if !out.iter().any(|existing| existing == name) {
                out.push(name.clone());
            }
        }
        out.retain(|name| !enable.iter().any(|lifted| lifted == name));
        out
    }
}

/// How a session behaves when it starts to fill the model's context window.
///
/// Four questions a real user asks — stop doing this automatically, do it sooner, keep more of the
/// recent conversation, and use a cheaper model to write the summary — plus the one switch that
/// turns off eliding stale tool output. The tuning constants behind those stay internal: they are
/// answers to questions nobody has, and every one of them is a way to make caching worse.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct CompactionConfig {
    /// Summarize automatically once the window fills. `None` = on. `false` leaves it to `/compact`,
    /// and a session that overflows will fail its next request rather than shorten itself.
    pub auto: Option<bool>,
    /// Share of the usable window (0.0–1.0) at which that happens. `None` = 0.9.
    pub at: Option<f64>,
    /// Tokens of the newest conversation kept verbatim rather than summarized. `None` = 20000.
    /// Capped at half the usable window, so a figure larger than the model cannot stall compaction
    /// on the sessions that need it most.
    pub keep_recent: Option<u32>,
    /// Tokens held back for the reply when deciding whether the window is full. `None` = 20000.
    pub reserve: Option<u32>,
    /// Elide stale tool output as a last step before summarizing. `None` = on. Off goes straight to
    /// a summary, which keeps less detail but never rewrites history mid-session.
    pub prune: Option<bool>,
    /// Model that writes the summary, as `provider/id`. `None` = whichever model is active. A
    /// smaller one here is usually the single best value in this file: summarizing is the most
    /// expensive call a long session makes and the least demanding.
    pub summarizer: Option<String>,
}

/// How far ahead of the summarize threshold eliding starts.
///
/// Not a knob, because the two are not independent: eliding is the last cheap thing tried *before*
/// summarizing, so a pair a user could set in either order is a pair they could set backwards — at
/// which point eliding never fires and the setting that switched it off is invisible.
const PRUNE_LEAD: f64 = 0.1;

impl CompactionConfig {
    /// The engine's settings, with every unset field left at its default.
    ///
    /// `summarizer` is parsed as `provider/id`; anything else is ignored rather than fatal, on the
    /// same rule as the rest of this file — one typo must not cost someone their session. It
    /// carries only the id: the front-end fills in its window and price from the model catalog.
    pub fn settings(&self) -> crate::compaction::CompactionSettings {
        let defaults = crate::compaction::CompactionSettings::default();
        let compact_at = self.at.filter(|at| *at > 0.0).unwrap_or(defaults.compact_at_fraction);
        crate::compaction::CompactionSettings {
            enabled: self.auto.unwrap_or(defaults.enabled),
            reserve_tokens: self.reserve.unwrap_or(defaults.reserve_tokens),
            keep_recent_tokens: self.keep_recent.unwrap_or(defaults.keep_recent_tokens),
            compact_at_fraction: compact_at,
            // Off is expressed as a threshold nothing reaches, so the one gate in `prune_tool_outputs`
            // stays the only place that decides.
            prune_at_fraction: match self.prune {
                Some(false) => f64::INFINITY,
                _ => (compact_at - PRUNE_LEAD).max(0.0),
            },
            prune_protect_tokens: defaults.prune_protect_tokens,
            prune_min_tokens: defaults.prune_min_tokens,
            summarizer: self
                .summarizer
                .as_deref()
                .and_then(parse_model_ref)
                .map(|r#ref| crate::provider::ModelInfo { r#ref, ..Default::default() }),
        }
    }
}

/// `provider/id`, or `None` for anything that isn't.
fn parse_model_ref(raw: &str) -> Option<crate::provider::ModelRef> {
    let (provider, id) = raw.trim().split_once('/')?;
    let (provider, id) = (provider.trim(), id.trim());
    (!provider.is_empty() && !id.is_empty()).then(|| crate::provider::ModelRef::new(provider, id))
}

/// What a claimed phase has to clear.
///
/// The loop's gate has two stages: these commands, then a reviewer. Both are opt-in, and with
/// neither configured a claim is taken at its word — which is what a loop that has not been told how
/// to check itself deserves.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct GateConfig {
    /// Commands that must succeed, each as `["program", "arg", …]`.
    ///
    /// No shell is interposed — same rule as `hooks.json`, and for the same reason: a string that
    /// gets word-split is a string that breaks on the first path with a space in it. Use
    /// `["sh", "-c", "…"]` when you want shell features, and remember that spells a platform.
    pub commands: Vec<Vec<String>>,
    /// How long any one command may run, in seconds. Defaults to ten minutes.
    pub timeout_secs: Option<u64>,
    /// Whether a reviewer reads each claimed phase after the commands pass.
    ///
    /// The reviewer runs in a fresh context, on `reviewer.model` when one is named and on the run's
    /// own model otherwise. Unset means on exactly when `reviewer.model` is named, which is what
    /// naming a reviewer has always meant.
    pub review: Option<bool>,
}

impl GateConfig {
    /// Whether a claimed phase is read by a reviewer, given the reviewer settings.
    pub fn reviews(&self, reviewer: &ReviewerConfig) -> bool {
        self.review.unwrap_or_else(|| reviewer.model_ref().is_some())
    }

    /// The per-command deadline.
    pub fn timeout(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.timeout_secs.unwrap_or(600))
    }

    /// Commands with anything unusable dropped, and a reason logged for each.
    ///
    /// An empty entry would spawn nothing and be reported as a failed command, which reads as "the
    /// gate refused" rather than "your config has a stray comma".
    pub fn commands(&self) -> Vec<Vec<String>> {
        self.commands
            .iter()
            .filter(|command| {
                if command.iter().any(|part| !part.trim().is_empty()) {
                    true
                } else {
                    tracing::warn!("ignoring an empty acceptance command");
                    false
                }
            })
            .cloned()
            .collect()
    }
}

/// Agent-loop preferences.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AgentConfig {
    /// Cap on tool-call round-trips within one turn. Omitted (the default) means no cap: a turn
    /// runs until the model stops calling tools or you cancel it. Set a number to bound cost;
    /// the final round-trip then runs without tools so the model wraps up in prose.
    pub max_tool_iterations: Option<usize>,
    /// Whether an in-session model / thinking choice becomes the next launch's default.
    ///
    /// On unless set to `false`. Re-picking a model you already chose is a tax on the person who
    /// already decided; anyone who wants a fixed default sets `default_model` and turns this off.
    pub remember_model: Option<bool>,
    /// What an `ask` resolves to where nobody can answer it.
    ///
    /// Headless, `luxe loop` and a review agent have no human to prompt, so every `ask` rule — the
    /// built-in guardrails on out-of-project work and a force push, a hook's escalation, anything
    /// `permissions.json` marks — collapses to one answer. `allow` unless set, because that is
    /// what an unattended run has always done and nobody should find their automation stopped by
    /// an upgrade.
    ///
    /// A project may set `deny` and may not set `allow`: tighten-only, the same rule as its
    /// `permissions.json`, because a repository you cloned does not get to widen what its own code
    /// may do. Interactive runs ignore it — there the answer is the reader's.
    pub on_ask: Option<OnAsk>,
    /// When the prompt cache is kept warm through a long wait; `streaming` unless set.
    ///
    /// A project may turn it down and may not turn it up, since warming spends the user's money.
    pub cache_warming: Option<crate::cache_warming::CacheWarming>,
    /// The mode a session starts in: `build` unless set. `--plan` overrides it for one run.
    pub default_mode: Option<String>,
}

/// What an `ask` resolves to in a run with nobody to ask (`agent.on_ask`, `--on-ask`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnAsk {
    /// Let it through, which is what an unattended run has always done (default).
    #[default]
    Allow,
    /// Refuse it, and tell the model the run is unattended so it takes another route.
    Deny,
    /// Unknown values fall back to `allow`.
    #[serde(other)]
    Unknown,
}

impl OnAsk {
    /// The spelling this parses from, for a flag and for a message that quotes one.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "allow" => Some(Self::Allow),
            "deny" => Some(Self::Deny),
            _ => None,
        }
    }

    /// How an unattended run resolves an `ask` under this setting.
    pub fn resolution(self) -> crate::permission::AskResolution {
        match self {
            Self::Deny => crate::permission::AskResolution::AutoDeny,
            Self::Allow | Self::Unknown => crate::permission::AskResolution::AutoAllow,
        }
    }
}

/// Plan mode: which files it may write, and what it runs on.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(default)]
pub struct PlanConfig {
    /// Project-relative files, or globs, plan mode may write. `PLAN.md` when empty.
    ///
    /// The first plain file is where the agent is told to put the plan, so a list that starts with
    /// a glob still names a file to write. A list of globs alone asks for a file matching the first.
    pub files: Vec<String>,
    /// Model plan mode runs on, as `provider/id`. `None` keeps the session's.
    ///
    /// Each model keeps its own prompt cache, so switching to plan mode on another model reads the
    /// whole conversation again at full price. Worth it for a long plan on a stronger model, not
    /// for a quick question.
    pub model: Option<String>,
    /// Reasoning level in plan mode, clamped to what its model supports.
    pub thinking: Option<crate::provider::ThinkingLevel>,
}

impl PlanConfig {
    /// `model` split into its parts, ignoring a spelling that is not `provider/id`.
    pub fn model_ref(&self) -> Option<(&str, &str)> {
        split_model(self.model.as_deref()?)
    }
}

/// The reviewer: a second opinion in a fresh context, for `/review` and the loop's gate.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(default)]
pub struct ReviewerConfig {
    /// Model the reviewer runs on, as `provider/id`. `None` uses the session's own model.
    ///
    /// The fresh context is most of a review's value, so the session's model is a real reviewer.
    /// A different vendor's adds a second set of blind spots, which is worth it when you have one.
    pub model: Option<String>,
    /// Reasoning level for a review, clamped to what its model supports.
    pub thinking: Option<crate::provider::ThinkingLevel>,
    /// Tools a review may call, all of them read-only. Absent or empty means the built-in list.
    pub tools: Option<Vec<String>>,
}

impl ReviewerConfig {
    /// `model` split into its parts, ignoring a spelling that is not `provider/id`.
    pub fn model_ref(&self) -> Option<(&str, &str)> {
        split_model(self.model.as_deref()?)
    }
}

/// `provider/id` split at its first slash, so an id that contains one keeps it.
///
/// A bare `gpt-5.6` has no provider, and guessing one would silently send the call to whichever
/// provider happened to be configured. `None` lets the caller say so instead.
fn split_model(raw: &str) -> Option<(&str, &str)> {
    let (provider, id) = raw.trim().split_once('/')?;
    (!provider.is_empty() && !id.is_empty()).then_some((provider, id))
}

impl AgentConfig {
    /// Whether the last in-session model / thinking choice is remembered (default: yes).
    pub fn remember_model(&self) -> bool {
        self.remember_model.unwrap_or(true)
    }

    /// When the prompt cache is kept warm (default: while a turn runs).
    pub fn cache_warming(&self) -> crate::cache_warming::CacheWarming {
        self.cache_warming.unwrap_or_default().resolved()
    }
}
