use super::*;
use crate::theme::Theme;

mod gate_trust;
mod merge;
mod schema;

#[test]
fn the_notification_method_defaults_to_auto_and_an_unknown_value_falls_back_to_it() {
    assert_eq!(Config::default().notification_method(), NotificationMethod::Auto);
    let parse = |json: &str| serde_json::from_str::<Config>(json).expect("parses");
    let osc777 = parse(r#"{ "notification_method": "osc777" }"#);
    assert_eq!(osc777.notification_method(), NotificationMethod::Osc777);
    let typo = parse(r#"{ "notification_method": "osc-nine" }"#);
    assert_eq!(typo.notification_method(), NotificationMethod::Auto);
}

#[test]
fn cloning_repositories_is_on_unless_config_says_otherwise() {
    assert!(ToolsConfig::default().clone_repositories());
    let off: ToolsConfig =
        serde_json::from_str(r#"{ "clone_repositories": false }"#).expect("parses");
    assert!(!off.clone_repositories());
}

/// The per-run flags layer over config in both directions.
#[test]
fn a_run_can_disable_more_tools_than_config_does() {
    let tools = ToolsConfig { disabled: vec!["screenshot".into()], ..Default::default() };
    let effective = tools.effective_disabled(&["web_search".into()], &[]);
    assert_eq!(effective, vec!["screenshot".to_string(), "web_search".to_string()]);
}

/// A standing preference must be liftable for one session, or it stops being a preference and
/// becomes a file you edit twice a day.
#[test]
fn a_run_can_lift_a_tool_its_config_disabled() {
    let tools = ToolsConfig {
        disabled: vec!["screenshot".into(), "web_search".into()],
        ..Default::default()
    };
    assert_eq!(
        tools.effective_disabled(&[], &["screenshot".into()]),
        vec!["web_search".to_string()]
    );
}

/// Naming the same tool twice is a user being emphatic, not a bug to reproduce downstream.
#[test]
fn disabling_a_tool_config_already_disabled_does_not_list_it_twice() {
    let tools = ToolsConfig { disabled: vec!["screenshot".into()], ..Default::default() };
    assert_eq!(
        tools.effective_disabled(&["screenshot".into()], &[]),
        vec!["screenshot".to_string()]
    );
}

/// Asking for both is contradictory; the exception wins, because it is the more specific and
/// more recent instruction.
#[test]
fn enabling_beats_disabling_when_a_run_asks_for_both() {
    let tools = ToolsConfig::default();
    assert!(tools.effective_disabled(&["git".into()], &["git".into()]).is_empty());
}

#[test]
fn tools_deny_list_disables_optional_tools_but_never_essentials() {
    let tools =
        ToolsConfig { disabled: vec!["screenshot".into(), "read".into()], ..Default::default() };
    assert!(!tools.is_enabled("screenshot"), "an optional tool on the deny-list is off");
    assert!(tools.is_enabled("git"), "a tool not on the deny-list stays on");
    assert!(tools.is_enabled("read"), "essential tools can't be disabled");
    assert!(ToolsConfig::default().is_enabled("anything"), "empty deny-list enables all");
}

/// Serde round-trip: default config serializes and deserializes cleanly.
#[test]
fn default_config_round_trips() {
    let config = Config::default();
    let json = serde_json::to_string(&config).unwrap();
    let parsed: Config = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.theme, config.theme);
    assert_eq!(parsed.show_key_hints, config.show_key_hints);
    assert_eq!(parsed.mouse, config.mouse);
    assert_eq!(parsed.statusline, config.statusline);
    assert_eq!(parsed.dock, config.dock);
}

/// Partial config (only some fields set) round-trips correctly.
#[test]
fn partial_config_round_trips() {
    let json = r#"{"theme": "dracula"}"#;
    let config: Config = serde_json::from_str(json).unwrap();
    assert_eq!(config.theme, Some("dracula".to_string()));
    assert_eq!(config.show_key_hints, None); // not set
    assert_eq!(config.mouse, None);
    assert_eq!(config.statusline, StatuslineConfig::default());
    assert_eq!(config.dock, DockConfig::default());
}

/// Every leniently read setting, through its resolver: each value it knows, one from a newer luxe,
/// and none at all.
#[test]
fn every_lenient_setting_resolves_known_unknown_and_absent_values() {
    type Resolve = fn(&Config) -> serde_json::Value;
    fn named(value: impl Serialize) -> serde_json::Value {
        serde_json::to_value(value).expect("serializes")
    }
    let settings: [(&str, &str, &[&str], &str, Resolve); 11] = [
        ("dock", "mode", &["auto", "shown", "hidden"], "auto", |c| named(c.dock.mode())),
        ("dock", "position", &["left", "right"], "left", |c| named(c.dock.position())),
        ("statusline", "visibility", &["auto", "shown", "hidden"], "auto", |c| {
            named(c.statusline.visibility())
        }),
        ("statusline", "style", &["slab", "flat"], "slab", |c| named(c.statusline.style())),
        ("conversation", "alignment", &["left", "center", "full"], "full", |c| {
            named(c.conversation.alignment())
        }),
        ("conversation", "growth", &["top", "bottom"], "bottom", |c| {
            named(c.conversation.growth())
        }),
        ("conversation", "thinking", &["shown", "collapsed", "hidden"], "shown", |c| {
            named(c.conversation.thinking())
        }),
        ("conversation", "stream_reveal", &["smooth", "off"], "smooth", |c| {
            named(c.conversation.stream_reveal())
        }),
        ("conversation", "tool_runs", &["fold", "cards"], "fold", |c| {
            named(c.conversation.tool_runs())
        }),
        ("conversation", "tool_details", &["errors", "all", "collapsed"], "errors", |c| {
            named(c.conversation.tool_details())
        }),
        ("diff", "layout", &["auto", "unified", "split"], "auto", |c| named(c.diff.layout())),
    ];
    for (block, key, known, default, resolve) in settings {
        assert_eq!(resolve(&Config::default()), default, "{block}.{key} unset");
        for value in known.iter().chain(&["from_a_newer_luxe"]) {
            let config: Config =
                serde_json::from_value(serde_json::json!({ block: { key: value } })).unwrap();
            let expected = if known.contains(value) { value } else { default };
            assert_eq!(resolve(&config), *expected, "{block}.{key} = {value}");
        }
    }
}

/// Each leniently read type's default is spelled the way it serializes, so the settings surface,
/// which reads the spelling, offers the value the loader applies.
#[test]
fn every_lenient_default_is_spelled_the_way_it_serializes() {
    fn spelled<T: Lenient + Serialize>() {
        assert_eq!(serde_json::to_value(T::DEFAULT).unwrap(), T::DEFAULT_NAME, "{}", T::NAME);
    }
    spelled::<StatuslineStyle>();
    spelled::<StatuslineVisibility>();
    spelled::<GlyphStyle>();
    spelled::<ImageStyle>();
    spelled::<ActivityBarStyle>();
    spelled::<DockMode>();
    spelled::<DockPosition>();
    spelled::<ThinkingDisplay>();
    spelled::<ToolDetailsPreference>();
    spelled::<ConversationAlignment>();
    spelled::<StreamReveal>();
    spelled::<ToolRuns>();
    spelled::<ConversationGrowth>();
    spelled::<DiffLayout>();
    spelled::<crate::cache_warming::CacheWarming>();

    // the types that also derive `Default`, which the loader and the TUI read, must agree with it
    fn derived<T: Lenient + Default + std::fmt::Debug>() {
        assert_eq!(T::default(), T::DEFAULT, "{}", T::NAME);
    }
    derived::<StatuslineStyle>();
    derived::<GlyphStyle>();
    derived::<ImageStyle>();
    derived::<ActivityBarStyle>();
    derived::<crate::cache_warming::CacheWarming>();
}

#[test]
fn conversation_preferences_validate_and_fall_back() {
    let config: Config = serde_json::from_str(
        r#"{"conversation":{"prose_max_width":110,"wide_block_max_width":130}}"#,
    )
    .unwrap();
    assert_eq!(config.conversation.prose_max_width(), 110);
    assert_eq!(config.conversation.wide_block_max_width(), 130);

    let invalid: Config = serde_json::from_str(
        r#"{"conversation":{"prose_max_width":121,"wide_block_max_width":141}}"#,
    )
    .unwrap();
    assert_eq!(invalid.conversation.prose_max_width(), 120);
    assert_eq!(invalid.conversation.wide_block_max_width(), 140);
}

#[test]
fn the_schema_documents_stream_reveal() {
    let schema: serde_json::Value =
        serde_json::from_str(Config::schema_json()).expect("the schema is valid JSON");
    let values = &schema["properties"]["conversation"]["properties"]["stream_reveal"]["enum"];
    assert_eq!(values, &serde_json::json!(["smooth", "off"]));
}

/// The three values have to *mean* three things. They did not: `layout_for` returned early
/// only for `unified`, so `auto` and `split` were the same setting spelled two ways, and the
/// documented difference between them described behavior no code implemented.
#[test]
fn the_three_layouts_demand_three_different_widths() {
    assert!(DiffLayout::Auto.splits(MIN_SPLIT_COLUMN, true), "auto wants a readable column");
    assert!(!DiffLayout::Auto.splits(MIN_SPLIT_COLUMN - 1, true), "and settles for nothing");

    assert!(DiffLayout::Split.splits(SPLIT_FLOOR_COLUMN, true), "split goes lower");
    assert!(!DiffLayout::Split.splits(SPLIT_FLOOR_COLUMN - 1, true), "but not below a column");
    const { assert!(SPLIT_FLOOR_COLUMN < MIN_SPLIT_COLUMN, "or they are the same setting") };

    assert!(!DiffLayout::Unified.splits(usize::MAX, true), "unified means unified");
    assert!(DiffLayout::Unknown.splits(MIN_SPLIT_COLUMN, true), "an unknown value is auto");
}

/// Not a preference: a second column with nothing to put in it is half a pane of empty wash
/// beside a crammed one, on any width and every setting.
#[test]
fn a_one_sided_change_never_splits() {
    for layout in [DiffLayout::Auto, DiffLayout::Split, DiffLayout::Unified] {
        assert!(!layout.splits(usize::MAX, false), "{layout:?}");
    }
}

#[test]
fn word_wrap_defaults_to_scrolling_where_a_surface_can_scroll() {
    assert!(!Config::default().diff.word_wrap());
    let wrapped: Config = serde_json::from_str(r#"{"diff":{"word_wrap":true}}"#).unwrap();
    assert!(wrapped.diff.word_wrap());
}

#[test]
fn statusline_left_modules_parse_and_merge() {
    let json = r#"{"statusline":{"left":["branch","cwd","activity"]}}"#;
    let config: Config =
        serde_json::from_str(json).expect("left statusline module config should parse");
    assert_eq!(
        config.statusline.left,
        Some(vec![StatuslineModule::Branch, StatuslineModule::Cwd, StatuslineModule::Activity,])
    );

    let unknown: Config = serde_json::from_str(r#"{"statusline":{"left":["future","branch"]}}"#)
        .expect("unknown left modules stay forward-compatible");
    assert_eq!(
        unknown.statusline.left,
        Some(vec![StatuslineModule::Unknown, StatuslineModule::Branch])
    );

    // `mode` was retired, so a config still naming it loads and ignores it rather than
    // failing the whole file.
    let retired: Config = serde_json::from_str(r#"{"statusline":{"left":["mode","branch"]}}"#)
        .expect("a retired module name still parses");
    assert_eq!(
        retired.statusline.left,
        Some(vec![StatuslineModule::Unknown, StatuslineModule::Branch])
    );

    // `changes` was retired once and is back, so a config that named it then gets the module
    // rather than the tombstone — which is what someone who wrote it there wanted all along.
    let revived: Config = serde_json::from_str(r#"{"statusline":{"left":["changes","branch"]}}"#)
        .expect("changes parses");
    assert_eq!(
        revived.statusline.left,
        Some(vec![StatuslineModule::Changes, StatuslineModule::Branch])
    );
}

#[test]
fn statusline_modules_control_order_and_visibility() {
    let json = r#"{"statusline":{"right":["tokens","thinking","model"],"thinking_colors":true}}"#;
    let config: Config = match serde_json::from_str(json) {
        Ok(config) => config,
        Err(error) => panic!("statusline module config should parse: {error}"),
    };
    assert_eq!(
        config.statusline.right,
        Some(vec![StatuslineModule::Tokens, StatuslineModule::Thinking, StatuslineModule::Model,])
    );
    assert_eq!(config.statusline.thinking_colors, Some(true));
}

#[test]
fn unknown_statusline_modules_are_forward_compatible() {
    let json = r#"{"statusline":{"right":["future_module","model"]}}"#;
    let config: Config = match serde_json::from_str(json) {
        Ok(config) => config,
        Err(error) => panic!("unknown statusline modules should parse: {error}"),
    };
    assert_eq!(
        config.statusline.right,
        Some(vec![StatuslineModule::Unknown, StatuslineModule::Model])
    );
}

/// Unknown fields are ignored (forward compatibility).
#[test]
fn unknown_fields_ignored() {
    let json = r#"{"theme": "nord", "unknown_field": true, "another": 42}"#;
    let config: Config = serde_json::from_str(json).unwrap();
    assert_eq!(config.theme, Some("nord".to_string()));
}

/// Malformed JSON returns None (never panics).
#[test]
fn malformed_json_returns_none() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    std::fs::write(&path, "{invalid json").unwrap();
    assert!(read_config_file(&path).is_none());
}

/// A project cannot vote itself the right to run its own binaries.
///
/// Every other field in this file is project-over-global, which is right for a theme and
/// catastrophic for this one: `lsp.project_servers` decides whether a `node_modules/.bin` entry
/// may be launched, so a repository that could set it would be answering the question that
/// exists to be asked *about* it. Read from the global layer only, the same rule that keeps a
/// repository's `.luxe/hooks.json` inert.
#[test]
fn a_project_cannot_grant_itself_its_own_language_servers() {
    let _xdg_config_home = XDG_CONFIG_HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let config_dir = dir.path().join("luxe");
    let project_dir = dir.path().join(".luxe");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::create_dir_all(&project_dir).unwrap();
    std::env::set_var("XDG_CONFIG_HOME", dir.path());

    // The user said never; the repository says always.
    std::fs::write(config_dir.join("config.json"), r#"{"lsp":{"project_servers":"never"}}"#)
        .unwrap();
    std::fs::write(project_dir.join("config.json"), r#"{"lsp":{"project_servers":"always"}}"#)
        .unwrap();
    assert_eq!(
        Config::load(dir.path()).lsp.project_servers,
        Some(ProjectServers::Never),
        "the repository's answer must not survive the merge"
    );

    // And with the user silent, the repository still does not get to speak for them.
    std::fs::write(config_dir.join("config.json"), "{}").unwrap();
    assert_eq!(
        Config::load(dir.path()).lsp.project_servers,
        None,
        "unset stays unset, so the default decides rather than the project"
    );

    // The global layer, on its own, is honored.
    std::fs::write(config_dir.join("config.json"), r#"{"lsp":{"project_servers":"always"}}"#)
        .unwrap();
    assert_eq!(Config::load(dir.path()).lsp.project_servers, Some(ProjectServers::Always));

    // No global file at all — a fresh install — is the user saying nothing, not the project
    // getting the last word. This case used to take the project's file whole.
    std::fs::remove_file(config_dir.join("config.json")).unwrap();
    std::fs::write(
        project_dir.join("config.json"),
        r#"{"lsp":{"project_servers":"always"},"update":{"check":false},"github":{"refresh_secs":1},"theme":"light"}"#,
    )
    .unwrap();
    let alone = Config::load(dir.path());
    assert_eq!(alone.lsp.project_servers, None, "global-only, even with no global file");
    assert_eq!(alone.update.check, None, "global-only, even with no global file");
    assert_eq!(alone.github.refresh_secs, None, "global-only, even with no global file");
    assert_eq!(alone.github.refresh_secs(), 60);
    let names: Vec<String> = alone.github.sections().into_iter().map(|s| s.name).collect();
    assert_eq!(
        names,
        ["Review requested", "Mine", "Assigned issues", "Pull requests", "Issues"],
        "the defaults"
    );
    assert!(alone.github.sections().iter().all(|s| s.query.contains("repo:{repo}")));
    // A project's own sections are searches, run when someone asks: the project may add them
    std::fs::write(
        project_dir.join("config.json"),
        r#"{"github":{"sections":[{"name":"Triage","query":"is:open label:triage repo:{repo}"}]}}"#,
    )
    .unwrap();
    let triage = Config::load(dir.path()).github.sections();
    assert_eq!(triage.len(), 1);
    assert_eq!(triage[0].name, "Triage");

    // A launcher is a key that runs a program: never one a repository named
    std::fs::write(
        project_dir.join("config.json"),
        r#"{"launchers":[{"key":"alt+l","command":"curl","args":["evil.sh"]}]}"#,
    )
    .unwrap();
    assert!(Config::load(dir.path()).launchers.is_empty(), "the project's is ignored");
    std::fs::write(
        config_dir.join("config.json"),
        r#"{"launchers":[{"key":"alt+l","command":"lazygit"}]}"#,
    )
    .unwrap();
    let launchers = Config::load(dir.path()).launchers;
    assert_eq!(launchers.len(), 1, "the reader's own is honored");
    assert_eq!((launchers[0].command.as_str(), launchers[0].args.len()), ("lazygit", 0));

    // A mention source runs a program too: the reader's own, never a repository's
    std::fs::write(
        project_dir.join("config.json"),
        r#"{"mentions":[{"name":"Jira","command":"curl","args":["evil.sh"]}]}"#,
    )
    .unwrap();
    assert!(Config::load(dir.path()).mentions.is_empty(), "the project's is ignored");
    std::fs::write(
        config_dir.join("config.json"),
        r#"{"mentions":[{"name":"Jira Cloud","command":"jira-mentions"}]}"#,
    )
    .unwrap();
    let mentions = Config::load(dir.path()).mentions;
    assert_eq!(mentions.len(), 1, "the reader's own is honored");
    assert_eq!(mentions[0].kind(), "jira-cloud", "named for its tab when no kind is set");
    assert!(!alone.worktree.always(), "off unless someone turns it on");
    assert_eq!(alone.theme.as_deref(), Some("light"), "ordinary keys still apply");

    std::env::remove_var("XDG_CONFIG_HOME");
}

/// Warming spends the user's money, so a repository may turn it down and never up — not past
/// what the user set, and not past the default when the user set nothing.
#[test]
fn a_project_may_turn_cache_warming_down_and_not_up() {
    use crate::cache_warming::CacheWarming;
    let _xdg_config_home = XDG_CONFIG_HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let config_dir = dir.path().join("luxe");
    let project_dir = dir.path().join(".luxe");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::create_dir_all(&project_dir).unwrap();
    std::env::set_var("XDG_CONFIG_HOME", dir.path());
    let write = |path: &std::path::Path, mode: &str| {
        let body = format!(r#"{{"agent":{{"cache_warming":"{mode}"}}}}"#);
        std::fs::write(path.join("config.json"), body).unwrap();
    };
    let load = || Config::load(dir.path()).agent.cache_warming();

    std::fs::write(config_dir.join("config.json"), "{}").unwrap();
    std::fs::write(project_dir.join("config.json"), "{}").unwrap();
    assert_eq!(load(), CacheWarming::Streaming, "on while a turn runs unless anyone says");

    write(&project_dir, "idle");
    assert_eq!(load(), CacheWarming::Streaming, "a project cannot volunteer the user's money");
    write(&project_dir, "off");
    assert_eq!(load(), CacheWarming::Off, "and may decline to spend it");

    write(&config_dir, "idle");
    write(&project_dir, "streaming");
    assert_eq!(load(), CacheWarming::Streaming, "the lower of the two stands");
    write(&project_dir, "sometimes");
    assert_eq!(load(), CacheWarming::Idle, "a project's typo is no instruction");

    std::env::remove_var("XDG_CONFIG_HOME");
}

/// A project may refuse what its own code is allowed to do, and may not grant it.
///
/// Tighten-only, the same rule as its `permissions.json` — and the direction matters: a clone
/// that could set `allow` would be voting to approve the escalations its own hooks raise in an
/// unattended run, which is the exact question `--on-ask deny` exists to let an operator answer.
#[test]
fn a_project_may_tighten_the_unattended_ask_policy_and_not_loosen_it() {
    let _xdg_config_home = XDG_CONFIG_HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let config_dir = dir.path().join("luxe");
    let project_dir = dir.path().join(".luxe");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::create_dir_all(&project_dir).unwrap();
    std::env::set_var("XDG_CONFIG_HOME", dir.path());
    let load = || Config::load(dir.path()).agent.on_ask;

    // The user said deny; the repository would rather be approved.
    std::fs::write(config_dir.join("config.json"), r#"{"agent":{"on_ask":"deny"}}"#).unwrap();
    std::fs::write(project_dir.join("config.json"), r#"{"agent":{"on_ask":"allow"}}"#).unwrap();
    assert_eq!(load(), Some(OnAsk::Deny), "a project cannot widen what it may do");

    // With the user silent it is still not the project's to grant.
    std::fs::write(config_dir.join("config.json"), "{}").unwrap();
    assert_eq!(load(), None, "unset stays unset, so the default decides");

    // Tightening is the direction that is allowed, from either side.
    std::fs::write(project_dir.join("config.json"), r#"{"agent":{"on_ask":"deny"}}"#).unwrap();
    assert_eq!(load(), Some(OnAsk::Deny), "a project may refuse on its own");
    std::fs::write(config_dir.join("config.json"), r#"{"agent":{"on_ask":"allow"}}"#).unwrap();
    assert_eq!(load(), Some(OnAsk::Deny), "and the stricter of the two stands");

    // The global layer alone is honored either way — it is the operator's own answer.
    std::fs::write(project_dir.join("config.json"), "{}").unwrap();
    assert_eq!(load(), Some(OnAsk::Allow));

    std::env::remove_var("XDG_CONFIG_HOME");
}

/// The setting only has to answer two ways, and an unreadable value must not read as `deny` —
/// a typo that silently stopped an unattended run would be indistinguishable from a broken one.
#[test]
fn an_unknown_ask_policy_falls_back_to_the_behavior_that_has_always_applied() {
    let unknown: Config =
        serde_json::from_str(r#"{"agent":{"on_ask":"sometimes"}}"#).expect("parses");
    assert_eq!(unknown.agent.on_ask, Some(OnAsk::Unknown));
    assert_eq!(
        unknown.agent.on_ask.unwrap_or_default().resolution(),
        crate::permission::AskResolution::AutoAllow
    );
    assert_eq!(OnAsk::Deny.resolution(), crate::permission::AskResolution::AutoDeny);
    assert_eq!(OnAsk::parse("DENY "), Some(OnAsk::Deny));
    assert_eq!(OnAsk::parse("dney"), None);
}

/// Missing config file returns defaults.
#[test]
fn missing_config_returns_defaults() {
    let _xdg_config_home = XDG_CONFIG_HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", dir.path());
    let config = Config::load(dir.path());
    assert_eq!(config.theme, None);
    assert_eq!(config.show_key_hints, None); // not set
    assert_eq!(config.mouse, None);
    assert_eq!(config.dock.mode(), DockMode::Auto);
    std::env::remove_var("XDG_CONFIG_HOME");
}

/// An empty command is dropped rather than spawned and reported as a failure.
#[test]
fn an_empty_acceptance_command_is_ignored() {
    let config: Config =
        serde_json::from_str(r#"{"gate":{"commands":[[],["  "],["cargo","test"]]}}"#).unwrap();
    assert_eq!(config.gate.commands(), vec![vec!["cargo".to_string(), "test".to_string()]]);
}

/// With nothing configured the gate has no commands and the default deadline.
#[test]
fn the_gate_defaults_to_nothing_to_run() {
    let config = Config::default();
    assert!(config.gate.commands().is_empty());
    assert_eq!(config.gate.timeout(), std::time::Duration::from_secs(600));
}

/// An empty block is the shipped behavior, field for field — so writing `"compaction": {}` can
/// never be a way to accidentally change it.
#[test]
fn an_unwritten_compaction_block_is_the_default_behavior() {
    let written: Config = serde_json::from_str(r#"{"compaction":{}}"#).unwrap();
    let settings = written.compaction.settings();
    let defaults = crate::compaction::CompactionSettings::default();

    assert_eq!(settings.enabled, defaults.enabled);
    assert_eq!(settings.reserve_tokens, defaults.reserve_tokens);
    assert_eq!(settings.keep_recent_tokens, defaults.keep_recent_tokens);
    assert_eq!(settings.compact_at_fraction, defaults.compact_at_fraction);
    assert_eq!(settings.prune_at_fraction, defaults.prune_at_fraction);
    assert_eq!(settings.prune_protect_tokens, defaults.prune_protect_tokens);
    assert_eq!(settings.prune_min_tokens, defaults.prune_min_tokens);
    assert!(settings.summarizer.is_none());
}

#[test]
fn compaction_settings_follow_what_was_written() {
    let config: Config = serde_json::from_str(
        r#"{"compaction":{"auto":false,"at":0.7,"keep_recent":8000,"reserve":12000,
                "summarizer":"anthropic/claude-haiku-4-5"}}"#,
    )
    .unwrap();
    let settings = config.compaction.settings();

    assert!(!settings.enabled);
    assert_eq!(settings.compact_at_fraction, 0.7);
    assert_eq!(settings.keep_recent_tokens, 8_000);
    assert_eq!(settings.reserve_tokens, 12_000);
    let summarizer = settings.summarizer.expect("a summarizer was named");
    assert_eq!(summarizer.r#ref.provider, "anthropic");
    assert_eq!(summarizer.r#ref.id, "claude-haiku-4-5");
}

/// Eliding is the step *before* summarizing, so moving the threshold moves both. Two
/// independent knobs could be set backwards, and the failure — eliding silently never firing —
/// looks exactly like it working.
#[test]
fn eliding_stays_ahead_of_summarizing_whatever_the_threshold_is() {
    let at = |json: &str| {
        let config: Config = serde_json::from_str(json).unwrap();
        let settings = config.compaction.settings();
        (settings.prune_at_fraction, settings.compact_at_fraction)
    };
    let (prune, compact) = at(r#"{"compaction":{"at":0.6}}"#);
    assert!(prune < compact, "{prune} vs {compact}");
    assert!((prune - 0.5).abs() < f64::EPSILON);

    // And a threshold low enough to underflow floors at zero rather than wrapping.
    let (prune, _) = at(r#"{"compaction":{"at":0.05}}"#);
    assert_eq!(prune, 0.0);

    // Switched off is a threshold nothing reaches, so the one gate stays the only decision.
    let (prune, _) = at(r#"{"compaction":{"prune":false}}"#);
    assert!(prune.is_infinite());
}

/// A typo in one field must not cost someone their session — the same rule the rest of this
/// file follows.
#[test]
fn a_malformed_summarizer_is_ignored_rather_than_fatal() {
    for written in ["claude-haiku", "", "   ", "/id", "provider/", "anthropic / "] {
        let config = CompactionConfig { summarizer: Some(written.to_string()), ..<_>::default() };
        assert!(config.settings().summarizer.is_none(), "{written:?} is not provider/id");
    }
}

/// A bell is an attention request the desktop may answer by taking focus, so it stays off
/// until someone asks for it by name.
#[test]
fn the_bell_is_silent_until_it_is_asked_for() {
    assert!(!Config::default().bell());
    let asked: Config = serde_json::from_str(r#"{"bell": true}"#).unwrap();
    assert!(asked.bell());
    let refused: Config = serde_json::from_str(r#"{"bell": false}"#).unwrap();
    assert!(!refused.bell());
}

/// `true` is every moment, a list is those moments, and nothing a reader could write takes the
/// rest of the config down with it.
#[test]
fn notifications_are_every_moment_some_of_them_or_none() {
    use NotificationEvent::{Permission, Question, Review, Turn};
    let parse = |json: &str| serde_json::from_str::<Config>(json).expect("loads");
    let off = Config::default();
    assert!(!off.any_notifications());
    assert!([Turn, Permission, Question, Review].iter().all(|event| !off.notifies(*event)));

    let all = parse(r#"{"notifications": true}"#);
    assert!([Turn, Permission, Question, Review].iter().all(|event| all.notifies(*event)));

    let some = parse(r#"{"notifications": ["permission", "Question", "telepathy"]}"#);
    assert!(some.notifies(Permission) && some.notifies(Question));
    assert!(!some.notifies(Turn) && !some.notifies(Review));
    assert!(some.any_notifications());
    assert!(!parse(r#"{"notifications": []}"#).any_notifications());

    let typo = parse(r#"{"notifications": "yes", "bell": true}"#);
    assert!(!typo.any_notifications(), "not a boolean or a list: off");
    assert!(typo.bell(), "and the keys around it still load");
}

/// A project's setting wins over the global one, list or boolean.
#[test]
fn a_project_notification_list_wins_over_the_global_switch() {
    let _xdg_config_home = XDG_CONFIG_HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let config_dir = dir.path().join("luxe");
    let project_dir = dir.path().join(".luxe");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::create_dir_all(&project_dir).unwrap();
    std::env::set_var("XDG_CONFIG_HOME", dir.path());
    std::fs::write(config_dir.join("config.json"), r#"{"notifications": true}"#).unwrap();
    std::fs::write(project_dir.join("config.json"), r#"{"notifications": ["question"]}"#).unwrap();
    let merged = Config::load(dir.path());
    assert!(merged.notifies(NotificationEvent::Question));
    assert!(!merged.notifies(NotificationEvent::Turn));
    std::fs::write(project_dir.join("config.json"), "{}").unwrap();
    assert!(Config::load(dir.path()).notifies(NotificationEvent::Turn), "global stands");
    std::env::remove_var("XDG_CONFIG_HOME");
}

#[test]
fn herdr_reporting_is_on_until_it_is_turned_off() {
    assert!(Config::default().herdr());
    let off: Config = serde_json::from_str(r#"{"herdr": false}"#).unwrap();
    assert!(!off.herdr());
}

/// Sessions are joinable until turned off, and a project can turn that off but not back on.
///
/// The break this catches: merging `joinable` like any other key, so a cloned repository's
/// `.luxe/config.json` reopens the socket the user closed.
#[test]
fn a_project_can_close_joining_but_not_reopen_it() {
    use super::merge::Layered;
    let parse = |json: &str| serde_json::from_str::<Config>(json).unwrap();
    assert_eq!(Config::default().joinable, None, "on, by default");
    let reopened = Config::layered(parse(r#"{"joinable": false}"#), parse(r#"{"joinable": true}"#));
    assert_eq!(reopened.joinable, Some(false), "the user's off stands");
    let closed = Config::layered(Config::default(), parse(r#"{"joinable": false}"#));
    assert_eq!(closed.joinable, Some(false), "the project's off stands");
    let left = Config::layered(parse(r#"{"joinable": true}"#), Config::default());
    assert_eq!(left.joinable, Some(true));
}

#[test]
fn ensure_config_file_bootstraps_schema_without_overwriting_config() {
    let directory = tempfile::tempdir().unwrap();
    let path = ensure_config_file(directory.path()).unwrap();
    assert_eq!(path, directory.path().join("config.json"));
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "{\n  \"$schema\": \"./config.schema.json\"\n}\n"
    );
    assert_eq!(
        std::fs::read_to_string(directory.path().join("config.schema.json")).unwrap(),
        Config::schema_json()
    );

    std::fs::write(&path, "{\"theme\":\"nord\"}\n").unwrap();
    ensure_config_file(directory.path()).unwrap();
    assert_eq!(std::fs::read_to_string(path).unwrap(), "{\"theme\":\"nord\"}\n");
}

/// resolved_theme: configured theme resolves against the registry.
#[test]
fn resolved_theme_uses_configured_name() {
    let registry = ThemeRegistry::new();
    let config = Config { theme: Some("light".to_string()), ..Default::default() };
    let resolved = config.resolved_theme(&registry);
    assert_eq!(resolved.len(), Theme::ALL_TOKENS.len());
    // light's accent resolves to a concrete Rgb (not an alias)
    assert!(matches!(resolved.get("accent"), Some(ColorValue::Rgb(_, _, _))));
}

/// resolved_theme: falls back to "dark" when no theme is set.
#[test]
fn resolved_theme_falls_back_to_dark() {
    let registry = ThemeRegistry::new();
    let config = Config::default(); // theme = None
    let resolved = config.resolved_theme(&registry);
    assert_eq!(resolved.len(), Theme::ALL_TOKENS.len());
}

/// resolved_theme: unknown theme name falls back to "dark" (never panics).
#[test]
fn resolved_theme_unknown_falls_back() {
    let registry = ThemeRegistry::new();
    let config = Config { theme: Some("nonexistent-theme".to_string()), ..Default::default() };
    // Should not panic; falls back to dark
    let resolved = config.resolved_theme(&registry);
    assert_eq!(resolved.len(), Theme::ALL_TOKENS.len());
}

/// resolved_theme: per-token `colors` overrides win over the theme; unparseable entries are
/// skipped without blanking the token.
#[test]
fn resolved_theme_applies_color_overrides() {
    let registry = ThemeRegistry::new();
    let config = Config {
        theme: Some("dark".to_string()),
        colors: Some(HashMap::from([
            ("diffAddBg".to_string(), "#123456".to_string()),
            ("accent".to_string(), "not-a-color".to_string()),
        ])),
        ..Default::default()
    };
    let resolved = config.resolved_theme(&registry);
    assert_eq!(resolved.get("diffAddBg"), Some(&ColorValue::Rgb(0x12, 0x34, 0x56)));
    // The unparseable accent override is ignored, keeping the theme's real accent.
    assert!(matches!(resolved.get("accent"), Some(ColorValue::Rgb(_, _, _))));
}
