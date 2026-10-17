//! Settings — what a run resolved to before the engine exists: provider, model, key, endpoint.

use std::path::PathBuf;

use luxe_providers::config::{resolve_value, ApiKey, CompatFlags, ProviderConfig};

use super::config::{env_nonempty, load_luxe_config};
use super::provider::{provider_serves, resolve_base_url, resolve_compat, split_model_ref};
use super::{IssueBinding, ResumeMode, WorktreeBinding};

/// The active provider's endpoint: the CLI flag, then the environment, then the provider's own
/// config and the catalog's default.
fn active_base_url(
    flag: Option<String>,
    provider: &str,
    config: Option<&ProviderConfig>,
) -> Option<String> {
    flag.or_else(|| env_nonempty("LUXE_BASE_URL"))
        .or_else(|| {
            (provider == "openai_compat").then(|| env_nonempty("OPENAI_BASE_URL")).flatten()
        })
        .map(|url| resolve_value(&url))
        .filter(|url| !url.trim().is_empty())
        .or_else(|| resolve_base_url(provider, config))
}

/// CLI-supplied overrides. Any set value wins over env and the config files.
#[derive(Debug, Clone, Default)]
pub(crate) struct CliOverrides {
    pub(crate) provider: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) api_key: Option<String>,
    pub(crate) base_url: Option<String>,
}

/// Resolved runtime settings for the agent.
///
/// Precedence, highest first: CLI flag → environment variable → what the last session picked →
/// `~/.config/luxe/auth.json` (+ `models.json`). There is no layer under that one: nothing here
/// invents a model. Local endpoints that need no key still work; a missing key for a hosted
/// provider surfaces as an auth error at request time.
#[derive(Debug, Clone)]
pub(crate) struct AgentSettings {
    /// The project: sessions, todos, reviews, logs and the rest of `.luxe/` live here, and its
    /// config and permissions are read from here, whatever checkout the agent works in.
    pub(crate) state_root: PathBuf,
    /// Where the agent works: tools run here, reviews and diffs are taken here, and the language
    /// servers and the code graph index it. The same path as `state_root` unless the session has a
    /// worktree of its own.
    pub(crate) work_root: PathBuf,
    /// The session's own worktree, when it has one; `work_root` is its path
    pub(crate) worktree: Option<WorktreeBinding>,
    /// The issue taken up with `--issue`, and the brief its first message carries
    pub(crate) issue: Option<IssueBinding>,
    /// The pull request checked out with `--pr`, which the session records working on
    pub(crate) pull_request: Option<(u64, String)>,
    /// Where this engine's spend is reported on top of its own session ledger.
    ///
    /// Carried in the settings rather than passed to `build_engine` so that an engine built from a
    /// clone of them — the phase reviewer's — reports into the same run without being told to.
    pub(crate) spend_sink: Option<std::sync::Arc<dyn luxe_core::usage::SpendSink>>,
    /// Provider identifier (`openai_compat` or `anthropic`).
    pub(crate) provider: String,
    /// Fully resolved metadata for the chosen model, or `None` when nobody has chosen one.
    ///
    /// The embedded models.dev snapshot and the refreshed cache layered under the user's config:
    /// the wire id (`model.ref.id`), context window, output cap, reasoning levels, and pricing the
    /// engine runs on.
    ///
    /// `None` is a real state rather than a gap to be filled: falling back to whichever id the
    /// provider catalog names first is a literal in the binary, and model ids age out faster than
    /// releases do, so the guess goes stale and then gets sent. Use
    /// [`Self::require_model`] where a model is needed to proceed and [`Self::launch_model`] where
    /// an engine has to be built either way.
    pub(crate) model: Option<luxe_core::provider::ModelInfo>,
    /// Base URL override (must include the API version segment, e.g. `.../v1`).
    pub(crate) base_url: Option<String>,
    /// The key named for this run (`--api-key`, else `LUXE_API_KEY`), which outranks a sign-in.
    ///
    /// Held as written, as a redacted secret; a `$ENV` or `!command` in it is expanded per request.
    /// `None` leaves the provider's own chain in charge (see `provider::provider_auth`).
    pub(crate) api_key: Option<ApiKey>,
    /// The provider's compat flags (from config, else defaults).
    pub(crate) compat: CompatFlags,
    /// Which session to run (new / latest / specific).
    pub(crate) resume: ResumeMode,
    /// Permission-mode override from the CLI (`None` defers to config / built-in default).
    pub(crate) permission_mode: Option<luxe_core::permission::PermissionMode>,
    /// Whether the project's `.luxe/permissions.json` may loosen policy (`--trust-project`).
    pub(crate) trust_project: bool,
    /// Reasoning level to apply once at startup (`--thinking`, else what the last session chose).
    ///
    /// `None` leaves the engine on the level the model itself declares (`models.json`'s
    /// `thinking`), which is why a remembered level is dropped here rather than sent and
    /// overridden: the send is what gets written back to the remembered level, so applying a
    /// model's own default through it would make the default permanent the first time it fired.
    pub(crate) initial_thinking: Option<luxe_core::provider::ThinkingLevel>,
    /// Per-turn tool round-trip cap (`agent.max_tool_iterations`, overridable by
    /// `LUXE_MAX_TOOL_ITERATIONS`). `None` — the default — means no cap.
    pub(crate) max_tool_iterations: Option<usize>,
    /// Whether the model was named on *this* command line (`--model`) or in `LUXE_MODEL`.
    ///
    /// Distinct from `model` being `Some`, which is true for a remembered or configured model as
    /// well. What this answers is narrower and is asked in exactly one kind of place: may plan
    /// mode's configured model apply? A flag is a one-off and outranks configuration; anything a
    /// config file decided does not.
    pub(crate) model_forced: bool,
    /// Whether `--thinking` set [`Self::initial_thinking`], which outranks `plan.thinking` the way
    /// [`Self::model_forced`] outranks `plan.model`. A level remembered from the last session does
    /// not.
    pub(crate) thinking_forced: bool,
    /// The mode this engine starts in (`--plan`, else `agent.default_mode`), when not `build`.
    pub(crate) initial_mode: Option<String>,
    /// Whether this engine is a reviewer's: one mode that changes nothing, the reviewer's tools and
    /// no others, and no reviewer of its own to ask.
    pub(crate) review_engine: bool,
    /// Whether an autonomous loop is driving this engine (`luxe loop`).
    ///
    /// Set by the loop subcommand and nowhere else. The loop's machinery is opt-in, so nothing it
    /// owns may reach an ordinary session: its hooks are registered only by
    /// `loop_hooks::build_default_hooks`, and this flag keeps `loop_control` — a tool with nothing
    /// to control outside a loop — out of the tool catalog every other session's prompt carries.
    pub(crate) loop_driven: bool,
    /// Tools to leave unregistered for this run (`--no-tool`), on top of the config deny-list.
    pub(crate) disable_tools: Vec<String>,
    /// Tools to register despite the config deny-list, for this run (`--with-tool`).
    pub(crate) enable_tools: Vec<String>,
    /// What an `ask` resolves to where nobody can answer it (`--on-ask`, else `agent.on_ask`).
    ///
    /// Read by the front-ends that cannot prompt — headless, the loop, a review agent — through
    /// [`Self::unattended_ask`]. The TUI and RPC ignore it: there the answer is the reader's.
    pub(crate) on_ask: luxe_core::config::OnAsk,
    /// Whether an interactive session starts in walk-away mode (`--auto-approve`).
    pub(crate) auto_approve: bool,
    /// What the TUI's last engine was set to, for the next one to start from: a session left for
    /// another keeps its mode, model and thinking level, as one loaded in place would. Overrides
    /// [`Self::initial_mode`] and [`Self::initial_thinking`], which are for a launch.
    pub(crate) carryover: Option<luxe_core::engine::Carryover>,
    /// What startup found wrong before the engine existed, carried through to
    /// [`luxe_core::engine::EngineConfig::startup_notices`] so every front-end reads it the same way.
    pub(crate) startup_notices: Vec<String>,
    /// The fraction of the usable window this engine compacts at, over `compaction.at`: the loop's
    /// `--compact-percent`
    pub(crate) compact_at: Option<f64>,
}

/// The mode `agent.default_mode` starts a session in, when it is not `build`, which needs no switch.
fn starting_mode(config: &luxe_core::config::Config) -> Option<String> {
    config.agent.default_mode.clone().filter(|mode| mode.as_str() != luxe_core::mode::BUILD)
}

impl AgentSettings {
    /// Resolve settings, letting `cli` overrides win over env and config.
    pub(crate) fn resolve(repo_root: PathBuf, cli: CliOverrides) -> Self {
        let config = load_luxe_config();
        // What the last session picked, unless the user asked us not to remember. It sits *below*
        // the CLI and the environment (a one-off `--model` must stay a one-off) and *above* the
        // configured default, which is the point: the default is where you start, not where you are
        // pinned.
        let ui_config = luxe_core::config::Config::load(&repo_root);
        let remembered = ui_config
            .agent
            .remember_model()
            .then(luxe_core::runtime_state::RuntimeState::load)
            .unwrap_or_default();

        let named_provider = cli.provider.or_else(|| env_nonempty("LUXE_PROVIDER"));
        let from_flag_or_env = cli.model.clone().or_else(|| env_nonempty("LUXE_MODEL"));
        let (provider_in_model, from_flag_or_env) = match from_flag_or_env {
            Some(model) => {
                let (provider, id) = split_model_ref(&model, named_provider.as_deref(), &config);
                (provider, Some(id))
            }
            None => (None, None),
        };
        let provider = named_provider
            .or(provider_in_model)
            .or_else(|| remembered.model.as_ref().map(|model| model.provider.clone()))
            .or_else(|| config.default_provider.clone())
            .unwrap_or_else(|| "openai_compat".to_string());

        // A remembered model only applies to the provider it was chosen for: pairing last session's
        // Anthropic model id with a locally forced provider would resolve to nothing.
        let remembered_model = remembered
            .model
            .as_ref()
            .filter(|model| model.provider == provider)
            .map(|model| model.id.clone());
        // A configured `default_model` belongs to the provider it was configured alongside. Pairing
        // it with a provider forced on the command line produces combinations that don't exist —
        // `claude-sonnet-5` on Codex — so it only applies when that provider can actually serve it.
        let configured_model = config
            .default_model
            .clone()
            .filter(|model| provider_serves(&provider, model, config.default_provider.as_deref()));
        let model_from_flag_or_env = from_flag_or_env.is_some();
        let chosen_model = from_flag_or_env.or(remembered_model).or(configured_model);

        let provider_cfg = config.providers.get(&provider);

        // Only the *explicit* sources land here. A sign-in, the provider's variable and its
        // configured key resolve per request in the adapter's auth source, by the chain a
        // `/model` switch uses too, so logging in mid-session takes effect without a relaunch.
        let api_key = cli
            .api_key
            .or_else(|| env_nonempty("LUXE_API_KEY"))
            .filter(|key| !key.trim().is_empty())
            .map(ApiKey::new);

        let base_url = active_base_url(cli.base_url, &provider, provider_cfg);

        let model =
            chosen_model.map(|id| crate::model_catalog::resolve_model(&config, &provider, &id).0);
        let max_tool_iterations =
            luxe_core::engine::max_tool_iterations_from_env(ui_config.agent.max_tool_iterations);

        // A model that declares its own starting level is not overruled by the level some *other*
        // model was last set to — that carry-over is the whole reason the field exists. `--thinking`
        // still wins, at the call site.
        let initial_thinking = match model.as_ref().and_then(|model| model.default_thinking) {
            Some(_) => None,
            None => remembered.thinking,
        };

        Self {
            worktree: None,
            issue: None,
            pull_request: None,
            work_root: repo_root.clone(),
            state_root: repo_root,
            spend_sink: None,
            compat: resolve_compat(&provider, provider_cfg),
            provider,
            model,
            // Only the two sources that belong to this invocation; a remembered or configured
            // model is a default, and a mode may override a default.
            model_forced: model_from_flag_or_env,
            thinking_forced: false,
            initial_mode: starting_mode(&ui_config),
            review_engine: false,
            base_url,
            api_key,
            resume: ResumeMode::New,
            permission_mode: None,
            trust_project: false,
            initial_thinking,
            max_tool_iterations,
            // Opt-in, and only `luxe loop` opts in.
            loop_driven: false,
            // Per-run tool overrides; applied over the config deny-list at registry assembly.
            disable_tools: Vec::new(),
            enable_tools: Vec::new(),
            on_ask: ui_config.agent.on_ask.unwrap_or_default(),
            auto_approve: false,
            carryover: None,
            startup_notices: Vec::new(),
            compact_at: None,
        }
    }

    /// These settings pointed at `model`, with everything provider-specific re-resolved.
    ///
    /// The reviewer or plan mode may name a model on a *different* provider than the session's — a
    /// local model building with a hosted one reviewing is the configuration the loop exists for.
    /// Swapping only the provider name and the id leaves the *session* provider's key, base URL and
    /// compat flags attached to it: Anthropic requests at LM Studio's port with no credential, and
    /// every phase advancing on an unavailable review. Everything that is not provider-specific —
    /// repo root, permissions, spend sink, resume mode — carries over untouched.
    ///
    /// The session's `--api-key` and `--base-url` deliberately do not: they were given for the
    /// active provider, and handing one provider's key to another is worse than having none.
    pub(super) fn for_model(&self, model: luxe_core::provider::ModelInfo) -> Self {
        let provider = model.r#ref.provider.clone();
        if provider == self.provider {
            return Self { model: Some(model), ..self.clone() };
        }

        let config = load_luxe_config();
        let provider_cfg = config.providers.get(&provider);

        Self {
            provider: provider.clone(),
            model: Some(model),
            // the provider's own chain finds its key, as it would on a `/model` switch
            api_key: None,
            base_url: resolve_base_url(&provider, provider_cfg),
            compat: resolve_compat(&provider, provider_cfg),
            ..self.clone()
        }
    }

    /// The chosen model, or the reason there is nothing to run.
    ///
    /// For the entry points that cannot ask anybody: a headless run and a loop. An interactive
    /// front-end does the opposite — it opens, and choosing is one of the things you do there.
    /// Called *before* an engine is built, so a run with nothing chosen ends with a sentence the
    /// reader can act on rather than a provider rejecting an empty model id.
    pub(crate) fn require_model(&self) -> anyhow::Result<&luxe_core::provider::ModelInfo> {
        self.model.as_ref().ok_or_else(|| {
            // The config path is asked for rather than spelled: it is `~/.config` on one platform
            // and nowhere near it on the others.
            let models = luxe_core::user_dirs::config_path_display("models.json");
            anyhow::anyhow!(
                "no model selected — pass --model <provider/id>, set LUXE_MODEL, or add \
                 \"default_model\" to {models}. `luxe models list` shows what this machine can \
                 reach; in the TUI, `/model` picks one and remembers it."
            )
        })
    }

    /// How this run resolves an `ask`, for the front-ends that have nobody to ask.
    ///
    /// Headless, `luxe loop` and a review agent call this instead of naming
    /// [`luxe_core::permission::AskResolution::AutoAllow`] themselves, so the setting reaches every
    /// unattended surface at once rather than three places agreeing by hand. A `deny` rule is
    /// enforced either way; this only settles the middle answer.
    pub(crate) fn unattended_ask(&self) -> luxe_core::permission::AskResolution {
        self.on_ask.resolution()
    }

    /// The model an engine is built with: the chosen one, or an **unset** stand-in.
    ///
    /// The stand-in exists for the TUI, which opens on a dashboard where choosing a model is one of
    /// the things you do — so it needs an engine before there is an answer. It carries an empty id
    /// ([`luxe_core::provider::ModelRef::is_unset`]) rather than a plausible one, which is the whole
    /// point: the engine refuses the turn, and a gap in that refusal would fail at the provider
    /// instead of billing a guess. Resolved through the same catalog layers as a real model, so the
    /// window and output cap the engine does arithmetic with are sane numbers.
    pub(crate) fn launch_model(&self) -> luxe_core::provider::ModelInfo {
        self.model.clone().unwrap_or_else(|| {
            crate::model_catalog::resolve_model(&load_luxe_config(), &self.provider, "").0
        })
    }

    /// Name to show in the UI: the resolved display name, or nothing when no model is chosen.
    pub(crate) fn display_model(&self) -> Option<String> {
        self.model.as_ref().map(|model| model.display().to_string())
    }

    /// The reasoning level this session begins on, as the model would actually run it.
    ///
    /// The same answer the engine reaches from [`Self::initial_thinking`] and the model's own
    /// configured level, so a front-end can paint its first frame without waiting for the engine's
    /// first `StateChanged` — and cannot disagree with it. With no model chosen there is nothing to
    /// clamp against, so the requested level stands.
    pub(crate) fn starting_thinking(&self) -> luxe_core::provider::ThinkingLevel {
        let Some(model) = &self.model else {
            return self.initial_thinking.unwrap_or_default();
        };
        let requested = self.initial_thinking.or(model.default_thinking).unwrap_or_default();
        model.effective_thinking_level(requested)
    }
}

#[cfg(test)]
impl AgentSettings {
    /// The model these settings chose, for a test that set one up.
    ///
    /// The panic is the assertion: every caller is checking *which* model was resolved, and "none"
    /// is a different test.
    pub(crate) fn chosen(&self) -> &luxe_core::provider::ModelInfo {
        self.model.as_ref().expect("these settings chose a model")
    }
}
