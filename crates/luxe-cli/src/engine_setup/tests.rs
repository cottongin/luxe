#[cfg(unix)]
use super::config::perms_are_loose;
use super::*;

/// A repository root for the tests that only need *a* path.
///
/// Nothing below touches the filesystem — `build_tool_registry` and `collect_tooling_summary`
/// only carry the root into what they build — so this has to be portable, not real, and a
/// literal `/tmp/…` is neither on Windows.
fn unused_repo_root() -> std::path::PathBuf {
    std::env::temp_dir().join("luxe-registry-shape")
}

/// Closing waits for the engine's way out, which is where `SessionEnd` runs — and gives up at
/// the grace rather than holding an exit hostage.
///
/// Headless used to send `Close` and return from `main`, so the runtime dropped the engine
/// mid-hook and not one `SessionEnd` hook ever fired there.
#[tokio::test]
async fn closing_waits_for_the_engine_but_not_forever() {
    let (actions, mut received) = mpsc::channel::<Action>(1);
    let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = finished.clone();
    let task = tokio::spawn(async move {
        received.recv().await;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
        luxe_core::engine::Carryover::default()
    });
    EngineCloser { task, grace: std::time::Duration::from_secs(5) }.close(&actions).await;
    assert!(finished.load(std::sync::atomic::Ordering::SeqCst), "the way out was waited for");

    let (actions, _received) = mpsc::channel::<Action>(1);
    let stuck = tokio::spawn(std::future::pending::<luxe_core::engine::Carryover>());
    let started = std::time::Instant::now();
    EngineCloser { task: stuck, grace: std::time::Duration::from_millis(100) }
        .close(&actions)
        .await;
    assert!(started.elapsed() < std::time::Duration::from_secs(2), "bounded by the grace");
}

#[tokio::test]
async fn tooling_summary_has_builtins_and_paths() {
    // The summary reads the global MCP config and, for an OAuth server, the token store: this
    // machine's, unless both are pointed somewhere empty
    let (config, state) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    std::env::set_var("XDG_CONFIG_HOME", config.path());
    std::env::set_var("XDG_STATE_HOME", state.path());
    let root = &unused_repo_root();
    let summary = collect_tooling_summary(root, &build_lsp_service(), &shared_dap_service()).await;
    // Built-ins are always present regardless of what's installed on the host.
    let langs: Vec<&str> = summary.servers.iter().map(|s| s.language.as_str()).collect();
    for lang in ["rust", "typescript", "python", "go", "cpp", "bash"] {
        assert!(langs.contains(&lang), "summary missing built-in server {lang}: {langs:?}");
    }
    let adapters: Vec<&str> = summary.adapters.iter().map(|a| a.id.as_str()).collect();
    for id in ["debugpy", "codelldb", "dlv"] {
        assert!(adapters.contains(&id), "summary missing built-in adapter {id}: {adapters:?}");
    }
    // No server has been started for a root nothing has opened, so nothing is running yet.
    assert!(summary.servers.iter().all(|s| !s.running));
    assert!(summary.lsp_config_path.is_some());
    assert!(summary.adapters_config_path.is_some());
}

/// Warm a real language server (clangd) and confirm `/lsp` reports it running. Gated behind
/// `LUXE_TEST_LSP_RUNNING=1` (needs clangd on PATH; starts a server process) so it stays out
/// of the default suite.
#[tokio::test]
async fn running_server_is_reported_after_warming() {
    if std::env::var("LUXE_TEST_LSP_RUNNING").as_deref() != Ok("1") {
        eprintln!("skipping: set LUXE_TEST_LSP_RUNNING=1 (needs clangd) to run");
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    std::fs::write(root.join("compile_commands.json"), "[]").unwrap();
    let source = root.join("main.c");
    std::fs::write(&source, "int main(void){return 0;}\n").unwrap();

    let lsp = build_lsp_service();
    // Warming inserts the client into the service's map, which is what running-state reads.
    let warmed = lsp.client_for_path(&source).await;
    assert!(warmed.is_some(), "clangd should start for a .c file");

    let summary = collect_tooling_summary(root, &lsp, &shared_dap_service()).await;
    let cpp = summary.servers.iter().find(|s| s.language == "cpp").expect("cpp entry");
    assert!(cpp.installed, "clangd is on PATH in this gated env");
    assert!(cpp.running, "cpp should be reported running after warming: {summary:?}");

    // Leaving the project shuts the server down, so it's no longer reported running.
    lsp.shutdown_all().await;
    let after = collect_tooling_summary(root, &lsp, &shared_dap_service()).await;
    let cpp = after.servers.iter().find(|s| s.language == "cpp").expect("cpp entry");
    assert!(!cpp.running, "cpp should stop running after shutdown_all: {after:?}");
}

#[test]
fn tool_registry_has_builtins_and_lsp() {
    let reg =
        build_tool_registry(&unused_repo_root(), &unused_repo_root(), build_lsp_service(), false);
    let names = reg.names();
    for expected in [
        "read",
        "write",
        "edit",
        "bash",
        "read_symbol",
        "module_report",
        "symbol_search",
        "grep",
        "glob",
        "git",
        "todo",
        "lsp_diagnostics",
        "lsp_navigation",
        "web_fetch",
    ] {
        assert!(names.contains(&expected), "tool registry missing '{expected}': {names:?}");
    }
    // `debug` is deliberately conditional — it carries the largest schema in the set and is
    // registered only where an adapter exists. Asserting it unconditionally was an assertion
    // about the machine, and it failed on any host without a debugger installed.
    assert_eq!(
        names.contains(&"debug"),
        shared_dap_service().has_adapter(),
        "debug is registered exactly when this machine can debug: {names:?}"
    );
}

/// The loop is opt-in, so nothing it owns may reach an ordinary session.
///
/// `loop_control` was registered unconditionally, and since it is not a core tool it landed in
/// the "available on demand" catalog of every prompt Luxe sent — offering a plain chat session a
/// tool for advancing phases of an autonomous run that was not happening.
#[test]
fn loop_control_is_absent_unless_a_loop_is_driving() {
    let repo = &unused_repo_root();

    let ordinary = build_tool_registry(repo, repo, build_lsp_service(), false);
    assert!(
        !ordinary.names().contains(&"loop_control"),
        "an ordinary session was offered loop_control: {:?}",
        ordinary.names()
    );

    let looped = build_tool_registry(repo, repo, build_lsp_service(), true);
    assert!(
        looped.names().contains(&"loop_control"),
        "...and the loop still gets it: {:?}",
        looped.names()
    );
}

/// Resolved settings never opt into the loop; only the `loop` subcommand sets that.
#[test]
fn resolved_settings_are_not_loop_driven() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let settings = AgentSettings::resolve(PathBuf::from("."), CliOverrides::default());
    assert!(!settings.loop_driven);
}

use crate::engine_setup::test_env::{isolated_env, ENV_LOCK};

#[cfg(unix)]
/// File modes are a unix idea; Windows has no bits to be loose about.
#[test]
#[cfg(unix)]
fn loose_secret_perms_are_flagged() {
    // 0600/0400 are fine; any group/other bit is loose.
    assert!(!perms_are_loose(0o600));
    assert!(!perms_are_loose(0o400));
    assert!(perms_are_loose(0o640)); // group-readable
    assert!(perms_are_loose(0o604)); // other-readable
    assert!(perms_are_loose(0o644));
    assert!(perms_are_loose(0o660));
}

/// Re-picking a model you already chose is a tax on the person who already decided.
#[test]
fn a_remembered_model_beats_the_configured_default_but_not_a_flag() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = tempfile::tempdir().expect("temp home");
    let state = home.path().join("state");
    std::fs::create_dir_all(&state).expect("state dir");
    luxe_core::runtime_state::RuntimeState {
        schema_version: luxe_core::runtime_state::RUNTIME_STATE_SCHEMA_VERSION,
        model: Some(luxe_core::runtime_state::RememberedModel {
            provider: "anthropic".into(),
            id: "claude-opus-5".into(),
        }),
        thinking: Some(luxe_core::provider::ThinkingLevel::High),
        recent_commands: Vec::new(),
        panes: Default::default(),
    }
    .save_to(&state.join("state.json"))
    .expect("write state");

    // Scoped env: these tests share a process, so the guard restores what it found.
    let _state_dir = EnvGuard::set("LUXE_STATE_DIR", state.to_str().expect("utf-8 path"));
    let project = tempfile::tempdir().expect("temp project");

    let remembered = AgentSettings::resolve(project.path().to_path_buf(), CliOverrides::default());
    assert_eq!(remembered.provider, "anthropic");
    assert_eq!(remembered.chosen().r#ref.id, "claude-opus-5");
    assert_eq!(remembered.initial_thinking, Some(luxe_core::provider::ThinkingLevel::High));

    // A one-off flag stays a one-off.
    let overridden = AgentSettings::resolve(
        project.path().to_path_buf(),
        CliOverrides { model: Some("claude-sonnet-5".into()), ..CliOverrides::default() },
    );
    assert_eq!(overridden.chosen().r#ref.id, "claude-sonnet-5");
}

/// A remembered model must not be pasted onto a different provider.
#[test]
fn a_remembered_model_is_ignored_when_the_provider_is_forced_elsewhere() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = tempfile::tempdir().expect("temp home");
    let state = home.path().join("state");
    std::fs::create_dir_all(&state).expect("state dir");
    luxe_core::runtime_state::RuntimeState {
        schema_version: luxe_core::runtime_state::RUNTIME_STATE_SCHEMA_VERSION,
        model: Some(luxe_core::runtime_state::RememberedModel {
            provider: "anthropic".into(),
            id: "claude-opus-5".into(),
        }),
        thinking: None,
        recent_commands: Vec::new(),
        panes: Default::default(),
    }
    .save_to(&state.join("state.json"))
    .expect("write state");
    let _state_dir = EnvGuard::set("LUXE_STATE_DIR", state.to_str().expect("utf-8 path"));
    // A machine with a `default_model` of its own would otherwise answer for the remembered one
    let config = home.path().join("config");
    let _config_dir = EnvGuard::set("LUXE_CONFIG_DIR", config.to_str().expect("utf-8 path"));
    let project = tempfile::tempdir().expect("temp project");

    let settings = AgentSettings::resolve(
        project.path().to_path_buf(),
        CliOverrides { provider: Some("openai_compat".into()), ..CliOverrides::default() },
    );

    assert_eq!(settings.provider, "openai_compat");
    // And nothing takes its place: the fallback that used to stand here was a literal.
    assert_eq!(settings.model, None);
}

/// The key `settings`' provider would send in `header`, by the chain a request resolves it with.
fn sent_key(settings: &AgentSettings, header: &str) -> Option<String> {
    let config = super::config::load_luxe_config();
    let auth = super::provider::active_auth(settings, config.providers.get(&settings.provider));
    let runtime =
        tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
    let resolved = runtime.block_on(auth.request_auth()).ok()?;
    resolved
        .headers
        .into_iter()
        .find(|(name, _)| name == header)
        .map(|(_, key)| key.expose().into())
}

/// A reviewer's model on another provider brings that provider's credential and endpoint.
///
/// The break this catches, measured on a real run: the loop's reviewer took the session's
/// settings with only `provider` and `model` swapped, so a Claude reviewer behind a local
/// developer inherited LM Studio's (absent) key and its `127.0.0.1:1234` base URL. Every review
/// failed with "no credential for 'anthropic'", every failure counted as *unavailable* rather
/// than a refusal, and four phases advanced in three minutes with nothing reviewed at all.
#[test]
fn a_role_model_on_another_provider_is_pointed_at_that_provider() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = isolated_env();
    std::fs::create_dir_all(home.path().join("luxe")).expect("config dir");
    std::fs::write(
        home.path().join("luxe").join("models.json"),
        r#"{
              "providers": {
                "lmstudio": { "base_url": "http://127.0.0.1:1234/v1" },
                "anthropic": { "api_key": "sk-ant-fixture" }
              }
            }"#,
    )
    .expect("write models.json");

    let local = AgentSettings::resolve(
        PathBuf::from("."),
        CliOverrides { provider: Some("lmstudio".into()), ..Default::default() },
    );
    assert_eq!(local.base_url.as_deref(), Some("http://127.0.0.1:1234/v1"));
    assert_eq!(sent_key(&local, "authorization"), None, "a local endpoint needs no key");

    let reviewer = local.for_model(luxe_core::provider::ModelInfo {
        r#ref: luxe_core::provider::ModelRef::new("anthropic", "claude-sonnet-5"),
        ..Default::default()
    });

    assert_eq!(reviewer.provider, "anthropic");
    assert_eq!(reviewer.chosen().r#ref.id, "claude-sonnet-5");
    assert_eq!(
        sent_key(&reviewer, "x-api-key").as_deref(),
        Some("sk-ant-fixture"),
        "the reviewer must carry anthropic's own credential"
    );
    assert_ne!(
        reviewer.base_url.as_deref(),
        Some("http://127.0.0.1:1234/v1"),
        "a hosted reviewer must not be sent to the local developer's port"
    );
    assert_eq!(reviewer.state_root, local.state_root, "the project carries over");
    assert_eq!(reviewer.work_root, local.work_root, "and so does the checkout it works in");
}

/// Staying on one provider changes the model and nothing else.
#[test]
fn a_role_model_on_the_same_provider_keeps_the_session_s_endpoint() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let _home = isolated_env();
    let local = AgentSettings::resolve(
        PathBuf::from("."),
        CliOverrides {
            provider: Some("lmstudio".into()),
            base_url: Some("http://127.0.0.1:1234/v1".into()),
            ..Default::default()
        },
    );

    let switched = local.for_model(luxe_core::provider::ModelInfo {
        r#ref: luxe_core::provider::ModelRef::new("lmstudio", "another-local-model"),
        ..Default::default()
    });

    assert_eq!(switched.chosen().r#ref.id, "another-local-model");
    assert_eq!(switched.base_url, local.base_url, "same provider, same endpoint");
}

/// Restores an environment variable to whatever it was, so sibling tests are unaffected.
struct EnvGuard {
    key: &'static str,
    previous: Option<String>,
}

impl EnvGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let previous = std::env::var(key).ok();
        std::env::set_var(key, value);
        Self { key, previous }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(value) => std::env::set_var(self.key, value),
            None => std::env::remove_var(self.key),
        }
    }
}

/// Nothing configured means nothing chosen. Luxe used to reach for whichever id the provider
/// catalog named first, which is a literal in the binary: it goes stale as models turn over,
/// and the first thing a fresh install did with it was send it.
#[test]
fn nothing_configured_chooses_no_model() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let _tmp = isolated_env();
    let s = AgentSettings::resolve(PathBuf::from("."), CliOverrides::default());
    assert_eq!(s.provider, "openai_compat");
    assert_eq!(s.model, None);
    assert!(s.display_model().is_none());
    assert!(s.require_model().is_err(), "a run with nothing chosen must not start");
    assert_eq!(sent_key(&s, "authorization"), None, "and no key to send");

    // And what an engine is built with instead names no model at all, so the turn is refused
    // rather than sent somewhere plausible.
    assert!(s.launch_model().r#ref.is_unset());
    assert_eq!(s.launch_model().r#ref.provider, "openai_compat");
}

/// Forcing a provider does not conjure one of its models either.
#[test]
fn forcing_a_provider_chooses_no_model() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let _tmp = isolated_env();
    let cli = CliOverrides { provider: Some("anthropic".into()), ..Default::default() };
    let s = AgentSettings::resolve(PathBuf::from("."), cli);
    assert_eq!(s.provider, "anthropic");
    assert_eq!(s.model, None);
    // The stand-in still resolves through the catalog layers, so the Anthropic API's own floors
    // apply and the engine is doing arithmetic with real numbers.
    assert!(s.launch_model().reasoning);
    assert!(s.launch_model().max_tokens > 0);
}

/// `--model provider/id` picks the provider too, the form the "no model selected" error asks
/// for; an id that merely contains a slash, or a prefix another provider was named over, stays
/// whole.
#[test]
fn a_model_can_name_its_provider() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let _tmp = isolated_env();
    let flags = |provider: Option<&str>, model: &str| CliOverrides {
        provider: provider.map(str::to_string),
        model: Some(model.to_string()),
        ..Default::default()
    };
    let s = AgentSettings::resolve(PathBuf::from("."), flags(None, "openai-codex/gpt-5.4"));
    assert_eq!((s.provider.as_str(), s.chosen().r#ref.id.as_str()), ("openai-codex", "gpt-5.4"));

    let s =
        AgentSettings::resolve(PathBuf::from("."), flags(Some("openrouter"), "anthropic/claude-x"));
    assert_eq!(s.provider, "openrouter");
    assert_eq!(s.chosen().r#ref.id, "anthropic/claude-x", "openrouter's own id");

    let s = AgentSettings::resolve(PathBuf::from("."), flags(None, "deepseek-ai/r1"));
    assert_eq!(s.chosen().r#ref.id, "deepseek-ai/r1", "not a provider, so not split");

    std::env::set_var("LUXE_MODEL", "anthropic/claude-opus-5-5");
    let s = AgentSettings::resolve(PathBuf::from("."), CliOverrides::default());
    std::env::remove_var("LUXE_MODEL");
    assert_eq!(
        (s.provider.as_str(), s.chosen().r#ref.id.as_str()),
        ("anthropic", "claude-opus-5-5")
    );
}

#[test]
fn cli_overrides_win_over_env() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let _tmp = isolated_env();
    std::env::set_var("LUXE_PROVIDER", "openai_compat");
    std::env::set_var("LUXE_MODEL", "gpt-4o");
    std::env::set_var("LUXE_API_KEY", "env-key");
    let cli = CliOverrides {
        provider: Some("anthropic".into()),
        model: Some("claude-x".into()),
        api_key: Some("cli-key".into()),
        base_url: Some("https://proxy/v1".into()),
    };
    let s = AgentSettings::resolve(PathBuf::from("."), cli);
    assert_eq!(s.provider, "anthropic");
    assert_eq!(s.chosen().r#ref.id, "claude-x");
    assert_eq!(s.api_key.as_ref().map(|k| k.expose()), Some("cli-key"));
    assert_eq!(s.base_url.as_deref(), Some("https://proxy/v1"));
    std::env::remove_var("LUXE_PROVIDER");
    std::env::remove_var("LUXE_MODEL");
    std::env::remove_var("LUXE_API_KEY");
}

#[test]
fn nested_auth_json_is_read() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let tmp = isolated_env();
    let luxe_dir = tmp.path().join("luxe");
    std::fs::create_dir_all(&luxe_dir).unwrap();
    std::fs::write(
        luxe_dir.join("auth.json"),
        r#"{
                "providers": {
                    "anthropic": { "api_key": "sk-ant-from-file" }
                },
                "default_provider": "anthropic",
                "default_model": "claude-3-5-haiku-latest"
            }"#,
    )
    .unwrap();
    let s = AgentSettings::resolve(PathBuf::from("."), CliOverrides::default());
    assert_eq!(s.provider, "anthropic");
    assert_eq!(s.chosen().r#ref.id, "claude-3-5-haiku-latest");
    assert_eq!(sent_key(&s, "x-api-key").as_deref(), Some("sk-ant-from-file"));
}

#[test]
fn model_thinking_map_reaches_runtime_settings() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let tmp = isolated_env();
    let luxe_dir = tmp.path().join("luxe");
    std::fs::create_dir_all(&luxe_dir).unwrap();
    std::fs::write(
        luxe_dir.join("models.json"),
        r#"{
                "default_provider": "openai_compat",
                "default_model": "gpt-5.6-sol",
                "models": {
                    "openai_compat/gpt-5.6-sol": {
                        "reasoning": true,
                        "thinking_level_map": {
                            "minimal": "low",
                            "xhigh": "xhigh",
                            "max": "max"
                        }
                    }
                }
            }"#,
    )
    .unwrap();

    let settings = AgentSettings::resolve(PathBuf::from("."), CliOverrides::default());
    assert!(settings.chosen().reasoning);
    let map = &settings.chosen().thinking_level_map;
    assert_eq!(map.get("minimal").and_then(|value| value.as_deref()), Some("low"));
    assert_eq!(map.get("max").and_then(|value| value.as_deref()), Some("max"));
}

/// A model that declares where it starts is not handed the level another model was left on.
///
/// The startup half of the same rule the engine enforces on a switch, and the reason the
/// remembered level is *dropped* rather than applied and overridden: applying it is what writes
/// it back, so a model's own default would become permanently remembered the first time it
/// fired — for every other model too.
#[test]
fn a_models_own_thinking_level_outranks_the_remembered_one() {
    use luxe_core::provider::ThinkingLevel;
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let tmp = isolated_env();
    let luxe_dir = tmp.path().join("luxe");
    std::fs::create_dir_all(&luxe_dir).unwrap();
    std::fs::write(
        luxe_dir.join("models.json"),
        r#"{
                "default_provider": "lmstudio",
                "default_model": "qwen3.8-27b",
                "models": {
                    "lmstudio/qwen3.8-27b": {
                        "reasoning": true,
                        "thinking": "medium",
                        "thinking_level_map": {
                            "off": "none", "low": "low", "medium": "medium",
                            "high": "high", "xhigh": "xhigh"
                        }
                    },
                    "lmstudio/some-other-build": { "reasoning": true }
                }
            }"#,
    )
    .unwrap();
    let state = tmp.path().join("state");
    std::fs::create_dir_all(&state).unwrap();
    luxe_core::runtime_state::RuntimeState {
        schema_version: luxe_core::runtime_state::RUNTIME_STATE_SCHEMA_VERSION,
        model: None,
        thinking: Some(ThinkingLevel::XHigh),
        recent_commands: Vec::new(),
        panes: Default::default(),
    }
    .save_to(&state.join("state.json"))
    .unwrap();
    let _state_dir = EnvGuard::set("LUXE_STATE_DIR", state.to_str().unwrap());

    let declared = AgentSettings::resolve(PathBuf::from("."), CliOverrides::default());
    assert_eq!(declared.chosen().default_thinking, Some(ThinkingLevel::Medium));
    assert_eq!(declared.initial_thinking, None, "the remembered level is not sent over it");
    assert_eq!(declared.starting_thinking(), ThinkingLevel::Medium);

    // An explicit flag still wins — it is set on top of the resolved settings, as `run` does.
    let mut flagged = declared.clone();
    flagged.initial_thinking = Some(ThinkingLevel::XHigh);
    assert_eq!(flagged.starting_thinking(), ThinkingLevel::XHigh);

    // A sibling on the same server that declares nothing still gets the remembered level.
    let undeclared = AgentSettings::resolve(
        PathBuf::from("."),
        CliOverrides { model: Some("some-other-build".into()), ..CliOverrides::default() },
    );
    assert_eq!(undeclared.chosen().default_thinking, None);
    assert_eq!(undeclared.initial_thinking, Some(ThinkingLevel::XHigh));
    // Clamped for display, because that build advertises no thinking map and `xhigh` is opt-in
    // — the level a request would really carry, which is the point of reporting it here.
    assert_eq!(undeclared.starting_thinking(), ThinkingLevel::High);
}

#[test]
fn permission_config_loads_global_and_project_tighten_only() {
    use luxe_core::permission::{build_request, PermissionAction};
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let tmp = isolated_env();
    // Global config: bash asks.
    let global_dir = tmp.path().join("luxe");
    std::fs::create_dir_all(&global_dir).unwrap();
    std::fs::write(global_dir.join("permissions.json"), r#"{ "permission": { "bash": "ask" } }"#)
        .unwrap();
    // Project config tries to LOOSEN bash to allow and TIGHTEN edit to deny.
    let repo = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(repo.path().join(".luxe")).unwrap();
    std::fs::write(
        repo.path().join(".luxe").join("permissions.json"),
        r#"{ "permission": { "bash": "allow", "edit": "deny" } }"#,
    )
    .unwrap();

    let bash = build_request("bash", &serde_json::json!({"command": "ls"}), repo.path());
    let edit = build_request("edit", &serde_json::json!({"path": "src/x.rs"}), repo.path());

    // Untrusted: project can tighten (edit->deny) but not loosen (bash stays ask).
    let untrusted = load_permission_engine(repo.path(), None, false);
    assert_eq!(untrusted.decide(&bash), PermissionAction::Ask, "project can't loosen bash");
    assert_eq!(untrusted.decide(&edit), PermissionAction::Deny, "project tightened edit");

    // Trusted: project may loosen (bash->allow).
    let trusted = load_permission_engine(repo.path(), None, true);
    assert_eq!(trusted.decide(&bash), PermissionAction::Allow, "trust lets project loosen");
}

#[test]
fn permission_mode_override_beats_config() {
    use luxe_core::permission::{build_request, PermissionAction, PermissionMode};
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let _tmp = isolated_env();
    let repo = tempfile::tempdir().unwrap();
    // No config files: read-only override should deny a mutating tool.
    let engine = load_permission_engine(repo.path(), Some(PermissionMode::ReadOnly), false);
    let edit = build_request("edit", &serde_json::json!({"path": "src/x.rs"}), repo.path());
    assert_eq!(engine.decide(&edit), PermissionAction::Deny);
}

/// A loop's `--compact-percent` is the threshold its engine compacts at, over the project's own,
/// and eliding moves with it; without one the project's stands.
#[test]
fn the_loop_s_mark_is_the_engine_s_compaction_threshold() {
    let config = luxe_core::config::CompactionConfig { at: Some(0.25), ..Default::default() };

    let looped = super::compaction_settings(Some(0.4), &config);
    assert_eq!(looped.compact_at_fraction, 0.4);
    assert!((looped.prune_at_fraction - 0.3).abs() < 1e-9, "{}", looped.prune_at_fraction);
    assert_eq!(super::compaction_settings(None, &config).compact_at_fraction, 0.25);
}

/// A model named in `compaction.summarizer` comes with the catalog's rates and window, so `/cost`
/// prices the summary call like any other instead of showing its spend as unpriced.
#[test]
fn a_named_summarizer_is_priced_like_any_other_model() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let tmp = isolated_env();
    // the embedded catalog only, not this machine's refreshed copy
    let no_cache = tmp.path().join("no-cache.json");
    let _catalog = EnvGuard::set("LUXE_MODELS_DEV_PATH", &no_cache.to_string_lossy());
    let config = luxe_core::config::CompactionConfig {
        summarizer: Some("anthropic/claude-haiku-4-5".into()),
        ..Default::default()
    };
    let settings = super::compaction_settings(None, &config);
    let active = luxe_core::provider::ModelInfo {
        r#ref: luxe_core::provider::ModelRef::new("openai", "gpt-5.6-sol"),
        ..Default::default()
    };

    let summarizer = luxe_core::compaction::summarizer_model(settings.summarizer.as_ref(), &active);
    assert_eq!(summarizer.r#ref.id, "claude-haiku-4-5");
    assert!(summarizer.cost.input > 0.0 && summarizer.cost.output > 0.0, "{:?}", summarizer.cost);
    assert_ne!(
        summarizer.context_window,
        luxe_core::provider::ModelInfo::default().context_window,
        "and its own window, for the summary's cap"
    );
}

/// `"herdr": false` turns the engine's Herdr reports off too, not only the interface's.
#[test]
fn herdr_reports_follow_the_config_switch() {
    let _guard = test_env::ENV_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    std::env::set_var("HERDR_ENV", "1");
    std::env::set_var("HERDR_PANE_ID", "w1:p1");
    std::env::remove_var("LUXE_HERDR");
    let on = herdr_hook(&luxe_core::config::Config::default()).is_some();
    let off: luxe_core::config::Config =
        serde_json::from_str(r#"{ "herdr": false }"#).expect("a config");
    let off = herdr_hook(&off).is_some();
    std::env::remove_var("HERDR_ENV");
    std::env::remove_var("HERDR_PANE_ID");
    assert!(on, "inside Herdr, on by default");
    assert!(!off, "switched off in config");
}

/// `plan.thinking` holds over a level remembered from the last session; only `--thinking` beats it.
#[test]
fn plan_thinking_wins_over_a_remembered_level() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let _env = test_env::isolated_env();
    let config: luxe_core::config::Config =
        serde_json::from_str(r#"{ "plan": { "thinking": "high" } }"#).expect("a config");
    let mut settings = AgentSettings::resolve(PathBuf::from("."), CliOverrides::default());
    settings.initial_thinking = Some(luxe_core::provider::ThinkingLevel::Low);
    let plan = |settings: &AgentSettings| {
        build_modes(settings, &config).get(luxe_core::mode::PLAN).and_then(|mode| mode.thinking)
    };
    assert_eq!(plan(&settings), Some(luxe_core::provider::ThinkingLevel::High), "remembered");
    settings.thinking_forced = true;
    assert_eq!(plan(&settings), None, "--thinking on the command line");
}

/// An engine that is still running when the grace runs out is stopped, not left behind.
///
/// The break this catches: the closer gave up waiting and dropped the task's handle, which detaches
/// it, so the TUI started the next session's engine while the last one still ran, possibly in a
/// checkout the process had just left.
#[tokio::test]
async fn an_engine_that_outlives_its_grace_is_stopped() {
    struct Dropped(std::sync::Arc<std::sync::atomic::AtomicBool>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }
    let stopped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = Dropped(stopped.clone());
    let task = tokio::spawn(async move {
        let _flag = flag;
        std::future::pending::<luxe_core::engine::Carryover>().await
    });
    let (actions, _rx) = tokio::sync::mpsc::channel(4);
    let closer = EngineCloser { task, grace: std::time::Duration::from_millis(20) };

    let carryover = closer.close(&actions).await;

    assert!(stopped.load(std::sync::atomic::Ordering::SeqCst), "the engine's task was stopped");
    assert!(carryover.is_none(), "and a stopped engine hands nothing over");
}

/// An engine that closes in time hands over what the reader set it to, for the next one.
#[tokio::test]
async fn an_engine_that_closes_hands_over_its_carryover() {
    let task = tokio::spawn(async { luxe_core::engine::Carryover::default() });
    let (actions, _rx) = tokio::sync::mpsc::channel(4);
    let closer = EngineCloser { task, grace: std::time::Duration::from_secs(5) };
    assert!(closer.close(&actions).await.is_some());
}
