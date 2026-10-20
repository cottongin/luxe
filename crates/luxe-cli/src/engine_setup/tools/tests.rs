use super::{is_known_tool, DeniedTool, DenySource, ToolDenyReport, DEFAULT_CORE_TOOLS};
use luxe_core::ToolAvailability;

/// The catalog is the only thing that can tell a typo from a tool this machine cannot run, and
/// the list every settings surface offers, so it has to cover what the registry actually
/// produces — including the four that register conditionally and are therefore absent from a
/// CI box's registry.
#[test]
fn the_catalog_covers_the_registry() {
    let lsp = super::super::services::build_lsp_service();
    let mut registry =
        super::build_tool_registry(std::path::Path::new("."), std::path::Path::new("."), lsp, true);
    super::apply_lazy_tools(&mut registry, std::path::Path::new("."));

    for name in registry.names() {
        assert!(is_known_tool(name), "{name} is registered but not in the tool catalog");
    }
    for conditional in ["debug", "screenshot", "ast_grep", "mcp"] {
        assert!(
            is_known_tool(conditional),
            "{conditional} registers only sometimes, which is exactly why it must be listed"
        );
    }
}

/// The near miss this exists for: `--no-tool websearch` disabled nothing and said nothing.
#[test]
fn a_near_miss_is_pointed_at_the_tool_it_meant() {
    assert_eq!(super::nearest_tool("websearch"), Some("web_search"));
    assert_eq!(super::nearest_tool("web-search"), Some("web_search"));
    assert_eq!(super::nearest_tool("webSearch"), Some("web_search"));
    assert_eq!(super::nearest_tool("lsp_diagnostic"), Some("lsp_diagnostics"));
    assert_eq!(super::nearest_tool("xyzzy"), None, "a guess at nothing helps nobody");
}

/// The four outcomes, each with the line a reader gets — and a removal, which is silent
/// because it worked and `/tools` lists it.
#[test]
fn the_deny_report_tells_the_four_outcomes_apart() {
    let denied = |name: &str, source| DeniedTool { name: name.to_string(), source };
    let report = ToolDenyReport {
        removed: vec![denied("web_fetch", DenySource::Flag)],
        refused: vec![denied("bash", DenySource::Flag)],
        unavailable: vec![denied("screenshot", DenySource::Config)],
        unknown: vec![
            (denied("websearch", DenySource::Flag), Some("web_search")),
            (denied("xyzzy", DenySource::Config), None),
        ],
    };

    assert_eq!(
        report.removals(),
        vec![("web_fetch".to_string(), "--no-tool".to_string())],
        "what /tools renders is unchanged"
    );
    let notices = report.notices();
    assert_eq!(notices.len(), 4, "a removal that worked is not news: {notices:?}");
    assert!(notices[0].starts_with("--no-tool bash: refused"), "{}", notices[0]);
    assert!(notices[1].contains("Did you mean web_search?"), "{}", notices[1]);
    assert!(
        notices[2].starts_with("tools.disabled \"xyzzy\":") && !notices[2].contains("mean"),
        "a config typo names the config key, and invents no suggestion: {}",
        notices[2]
    );
    assert!(
        notices[3].contains("not registered in this run"),
        "a tool that is known but absent is not a typo: {}",
        notices[3]
    );
}

/// The switch reaches the tool the model is offered, not just the config: with cloning off,
/// `web_fetch`'s description stops promising a path it will not produce.
#[test]
fn turning_cloning_off_in_config_reaches_web_fetch() {
    let project = tempfile::tempdir().expect("temp project");
    std::fs::create_dir_all(project.path().join(".luxe")).unwrap();
    std::fs::write(
        project.path().join(".luxe").join("config.json"),
        r#"{ "tools": { "clone_repositories": false } }"#,
    )
    .unwrap();
    let lsp = super::super::services::build_lsp_service();
    let registry = super::build_tool_registry(project.path(), project.path(), lsp, false);
    let description = registry.get("web_fetch").expect("web_fetch").schema().description;
    assert!(!description.contains("gh-cache"), "{description}");
}

/// And the whole path: a run that disables an essential tool, a real one, a conditional one
/// and a typo gets one report that separates them.
#[test]
fn the_deny_list_classifies_what_a_real_run_asks_for() {
    let lsp = super::super::services::build_lsp_service();
    let mut registry = super::build_tool_registry(
        std::path::Path::new("."),
        std::path::Path::new("."),
        lsp,
        false,
    );
    let project = tempfile::tempdir().expect("temp project");
    let disable: Vec<String> =
        ["bash", "web_fetch", "mcp", "websearch"].iter().map(|s| s.to_string()).collect();

    let report = super::apply_tool_deny_list(&mut registry, project.path(), &disable, &[]);

    assert_eq!(report.refused.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["bash"]);
    assert_eq!(report.removed.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["web_fetch"]);
    assert_eq!(
        report.unavailable.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(),
        ["mcp"],
        "a tool luxe knows that this run never registered is not a typo"
    );
    assert_eq!(report.unknown.len(), 1);
    assert_eq!(report.unknown[0].0.name, "websearch");
    assert_eq!(report.unknown[0].1, Some("web_search"));
    assert!(registry.names().contains(&"bash"), "and bash is still there");
}

/// The status list is what `/tools` renders, so it has to be built from the *tiered* registry.
/// Reading it earlier would report every tool as active and hide the distinction the view
/// exists to draw.
#[test]
fn tool_statuses_separate_core_from_deferred_and_carry_the_disabled() {
    let lsp = super::super::services::build_lsp_service();
    let mut registry = super::build_tool_registry(
        std::path::Path::new("."),
        std::path::Path::new("."),
        lsp,
        false,
    );
    // As production does it: the tool is really gone before its absence is described.
    assert!(registry.unregister("web_search"), "web_search is a built-in");
    super::apply_lazy_tools(&mut registry, std::path::Path::new("."));

    let statuses = super::collect_tool_statuses(
        &registry,
        vec![("web_search".to_string(), "--no-tool".to_string())],
    );
    assert_eq!(
        statuses.iter().filter(|s| s.name == "web_search").count(),
        1,
        "a tool appears once, whatever state it is in"
    );
    let state =
        |name: &str| statuses.iter().find(|s| s.name == name).map(|s| s.availability.clone());

    assert_eq!(state("read"), Some(ToolAvailability::Active), "a core tool ships every time");
    assert_eq!(
        state("lsp_navigation"),
        Some(ToolAvailability::Deferred),
        "an expensive, rarely-used tool is deferred rather than absent"
    );
    assert_eq!(
        state("web_search"),
        Some(ToolAvailability::Disabled { source: "--no-tool".to_string() }),
        "an unregistered tool is still reported, with what removed it"
    );
    assert!(
        statuses.windows(2).all(|pair| pair[0].name <= pair[1].name),
        "sorted, so the view does not reorder between snapshots"
    );
}

/// Untiered (`LUXE_LAZY_TOOLS=0`) there is no deferred tier, so nothing may be reported as
/// deferred — it would send the reader to `activate_tools` for a tool that is already loaded.
#[test]
fn nothing_is_deferred_when_tiering_is_off() {
    let lsp = super::super::services::build_lsp_service();
    let registry = super::build_tool_registry(
        std::path::Path::new("."),
        std::path::Path::new("."),
        lsp,
        false,
    );
    let statuses = super::collect_tool_statuses(&registry, Vec::new());
    assert!(!statuses.is_empty());
    assert!(statuses.iter().all(|s| s.availability == ToolAvailability::Active));
}

/// The split is an evidence claim, so it gets a test. Deferring these four changed nothing
/// about which tools the model reached for; deferring `test` did. The symbol tools were kept
/// for a while on gpt-6.1-sol's use, then measured as earning nothing by being in the request.
#[test]
fn the_expensive_unused_tools_are_deferred_and_the_used_ones_are_not() {
    for deferred in ["debug", "lsp_navigation", "screenshot", "ast_grep"] {
        assert!(
            !DEFAULT_CORE_TOOLS.contains(&deferred),
            "{deferred} is 31% of the tool block between them and fired once in 58 runs"
        );
    }
    for core in ["read", "write", "edit", "bash", "grep", "test"] {
        assert!(DEFAULT_CORE_TOOLS.contains(&core), "{core} has demonstrated use");
    }
    for deferred in ["impact", "module_report"] {
        assert!(!DEFAULT_CORE_TOOLS.contains(&deferred), "{deferred}: no call in 248 runs");
    }
    for deferred in ["symbol_search", "read_symbol"] {
        assert!(
            !DEFAULT_CORE_TOOLS.contains(&deferred),
            "{deferred}: every arm of the sweep was as right without it"
        );
    }
}

/// Deferring `question` would cost an activation round-trip at the exact moment the user is
/// already waiting to be asked something.
#[test]
fn question_stays_core_for_the_users_sake() {
    assert!(DEFAULT_CORE_TOOLS.contains(&"question"));
}

/// `skill` is core exactly where there are skills, and deferred where there are none.
///
/// The break this catches is invisible from inside a session and costs real money: a deferred
/// tool's first use goes through `activate_tools`, which on a provider that has to list it
/// *changes the tool list*, which invalidates the cached prompt prefix and re-bills everything
/// in front of it — measured on the wire as `#4 → #5: tools CHANGED`. The skill index is already in the prompt of anyone
/// who has skills, so the tool it points at is not optional for them; for everyone else the
/// schema is dead weight and stays deferred.
///
/// Both halves are asserted against the same fixture, because the interesting failure is
/// promoting it unconditionally — which no test of the skills case alone can see.
#[test]
fn skill_is_core_when_there_are_skills_and_deferred_when_there_are_not() {
    // The real global skills directories would otherwise decide this test's answer on a machine
    // that has skills installed — including this developer's. Home as well as config, since
    // `~/.agents/skills` hangs off home, and Windows reads `USERPROFILE` for that.
    let home = tempfile::tempdir().expect("temp home");
    std::env::set_var("XDG_CONFIG_HOME", home.path());
    std::env::set_var("HOME", home.path());
    std::env::set_var("USERPROFILE", home.path());
    let project = tempfile::tempdir().expect("temp project");

    let tier_of_skill = |root: &std::path::Path| {
        let lsp = super::super::services::build_lsp_service();
        let mut registry = super::build_tool_registry(root, root, lsp, false);
        super::apply_lazy_tools(&mut registry, root);
        let statuses = super::collect_tool_statuses(&registry, Vec::new());
        statuses
            .into_iter()
            .find(|status| status.name == "skill")
            .map(|status| status.availability)
            .expect("skill is a built-in")
    };

    assert_eq!(
        tier_of_skill(project.path()),
        ToolAvailability::Deferred,
        "with no skills the schema is dead weight"
    );

    let skill = project.path().join(".luxe").join("skills").join("canary");
    std::fs::create_dir_all(&skill).expect("skill dir");
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: canary\ndescription: a skill that exists\n---\n\nBody.\n",
    )
    .expect("SKILL.md");

    assert_eq!(
        tier_of_skill(project.path()),
        ToolAvailability::Active,
        "the index is in the prompt, so the tool it points at ships with it"
    );

    // An explicit list is explicit, and outranks both answers.
    std::env::set_var("LUXE_CORE_TOOLS", "read,write");
    assert_eq!(tier_of_skill(project.path()), ToolAvailability::Deferred);
    std::env::remove_var("LUXE_CORE_TOOLS");
    std::env::remove_var("XDG_CONFIG_HOME");
}

/// `mcp` ships in every request when this run has a server, and not at all when it has none —
/// through the real sequence, registration and then tiering, since tiering first would defer it
/// again without a word.
#[tokio::test]
async fn mcp_is_core_when_a_server_is_registered_and_absent_when_none_is() {
    let home = tempfile::tempdir().expect("temp home");
    std::env::set_var("XDG_CONFIG_HOME", home.path());
    let project = tempfile::tempdir().expect("temp project");
    let tier_of_mcp = |root: std::path::PathBuf| async move {
        let lsp = super::super::services::build_lsp_service();
        let mut registry = super::build_tool_registry(&root, &root, lsp, false);
        super::register_mcp_tools(&mut registry, &root).await;
        super::apply_lazy_tools(&mut registry, &root);
        let statuses = super::collect_tool_statuses(&registry, Vec::new());
        let catalog: Vec<String> =
            registry.deferred_catalog().into_iter().map(|(name, _)| name).collect();
        let tier = statuses.into_iter().find(|s| s.name == "mcp").map(|status| status.availability);
        (tier, catalog.contains(&"mcp".to_string()))
    };

    assert_eq!(
        tier_of_mcp(project.path().to_path_buf()).await,
        (None, false),
        "no server: no tool, and no catalog line either"
    );

    // A global server, lazy, so nothing is started to answer this
    let config = home.path().join("luxe");
    std::fs::create_dir_all(&config).expect("config dir");
    std::fs::write(
        config.join("mcp.json"),
        r#"{ "mcpServers": { "canary": { "command": "true", "lifecycle": "lazy" } } }"#,
    )
    .expect("mcp.json");
    assert_eq!(
        tier_of_mcp(project.path().to_path_buf()).await,
        (Some(ToolAvailability::Active), false),
        "a server this run may reach puts the gateway in every request"
    );

    std::env::set_var("LUXE_CORE_TOOLS", "read,write");
    assert_eq!(
        tier_of_mcp(project.path().to_path_buf()).await.0,
        Some(ToolAvailability::Deferred),
        "an explicit list still outranks it"
    );
    std::env::remove_var("LUXE_CORE_TOOLS");
    std::env::remove_var("XDG_CONFIG_HOME");
}

/// One word for a file across the whole surface.
///
/// The registry is the only place that sees every schema at once, which is exactly why the
/// surface drifted: `read`/`write`/`edit` said `path` while the discovery funnel said `file`,
/// so the funnel's own next step was where a model arrived holding the other word. The tools
/// still *accept* the old spelling; what must not come back is a second one being advertised.
/// The whole surface, with the capability-gated tools forced in.
///
/// `screenshot`, `ast_grep` and `debug` register only where their program is installed, so a
/// sweep over the plain registry silently stops covering them on a machine without chromium
/// — which is most CI runners, and one of those three is exactly where a stale argument name
/// last hid.
fn every_schema() -> Vec<luxe_core::provider::ToolSchema> {
    use luxe_core::Tool;
    let lsp = super::super::services::build_lsp_service();
    let registry =
        super::build_tool_registry(std::path::Path::new("."), std::path::Path::new("."), lsp, true);
    let mut schemas = registry.schemas();
    for gated in ["screenshot", "ast_grep"] {
        if !schemas.iter().any(|s| s.name == gated) {
            schemas.push(match gated {
                "screenshot" => luxe_tools::screenshot::ScreenshotTool::new().schema(),
                _ => luxe_tools::astgrep::AstGrepTool::new().schema(),
            });
        }
    }
    if !schemas.iter().any(|s| s.name == "debug") {
        schemas
            .push(luxe_dap::DebugTool::new(super::super::services::shared_dap_service()).schema());
    }
    schemas
}

/// Every property name in the surface, `tool.a.b` style, including nested `items` schemas.
fn advertised_names(schema: &luxe_core::provider::ToolSchema) -> Vec<String> {
    fn walk(node: &serde_json::Value, trail: &str, out: &mut Vec<String>) {
        if let Some(properties) = node.get("properties").and_then(|p| p.as_object()) {
            for (name, child) in properties {
                let path = if trail.is_empty() { name.clone() } else { format!("{trail}.{name}") };
                out.push(path.clone());
                walk(child, &path, out);
            }
        }
        if let Some(items) = node.get("items") {
            walk(items, trail, out);
        }
    }
    let mut out = Vec::new();
    walk(&schema.parameters, "", &mut out);
    out
}

/// No advertised argument carries an underscore, at any depth.
///
/// Grounded in measurement rather than taste: `edit`'s `old_text`/`new_text` corrupted two in
/// five calls, because the model emitted the separator after a key ending in `_text` as `">`
/// and the provider's JSON repair then stringified the whole array. Only that suffix is known to
/// be dangerous, but the underscore buys nothing anywhere — a name is a name — and one convention
/// across the surface keeps the next `wait_ms` from being written without a second thought.
/// Retired spellings stay *accepted* via `luxe_core::tool::arg`; what this refuses is
/// advertising one.
#[test]
fn no_tool_advertises_an_argument_with_an_underscore_in_its_name() {
    let schemas = every_schema();
    assert!(schemas.len() > 15, "the sweep found almost no tools: {}", schemas.len());

    let mut offenders = Vec::new();
    for schema in &schemas {
        for name in advertised_names(schema) {
            let leaf = name.rsplit('.').next().unwrap_or(&name);
            if leaf.contains('_') {
                offenders.push(format!("{}.{name}", schema.name));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "advertise these camelCase and accept the old spelling via `luxe_core::tool::arg`: \
             {offenders:?}"
    );
}

/// The same concept spelled two ways is the drift this convention exists to stop.
///
/// `screenshot` took `wait_ms` while `lsp_diagnostics` took `waitMs` — the same argument, the
/// same units, two names, and nothing that would ever have failed.
#[test]
fn a_shared_argument_is_spelled_the_same_way_by_every_tool_that_takes_it() {
    let mut spellings: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for schema in every_schema() {
        for name in advertised_names(&schema) {
            let leaf = name.rsplit('.').next().unwrap_or(&name).to_string();
            spellings
                .entry(leaf.to_lowercase().replace('_', ""))
                .or_default()
                .push(format!("{}.{leaf}", schema.name));
        }
    }
    for (canonical, uses) in spellings {
        let distinct: std::collections::BTreeSet<&str> =
            uses.iter().filter_map(|use_| use_.split('.').nth(1)).collect();
        assert!(distinct.len() <= 1, "`{canonical}` is advertised under {distinct:?} by {uses:?}");
    }
}

#[test]
fn every_tool_advertises_one_file_path_under_the_same_name() {
    let lsp = super::super::services::build_lsp_service();
    let registry = super::build_tool_registry(
        std::path::Path::new("."),
        std::path::Path::new("."),
        lsp,
        false,
    );

    // `target` is deliberately not a path: it takes a symbol name, a qualified name, or a file.
    const NOT_A_PATH_ARGUMENT: [&str; 1] = ["impact"];
    for schema in registry.schemas() {
        if NOT_A_PATH_ARGUMENT.contains(&schema.name.as_str()) {
            continue;
        }
        let Some(properties) = schema.parameters.get("properties").and_then(|p| p.as_object())
        else {
            continue;
        };
        for spelling in ["file", "file_path", "filePath"] {
            assert!(
                !properties.contains_key(spelling),
                "{} advertises `{spelling}`; a single file path is spelled `path`",
                schema.name
            );
        }
    }
}
