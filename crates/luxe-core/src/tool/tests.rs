use super::*;

/// A no-op tool with a given name, for registry ordering tests.
struct NamedTool(&'static str);

#[async_trait]
impl Tool for NamedTool {
    fn name(&self) -> &str {
        self.0
    }
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: self.0.to_string(),
            description: String::new(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        }
    }
    async fn run(&self, _args: serde_json::Value, _cx: &ToolContext) -> ToolResultData {
        ToolResultData { content: vec![], is_error: false }
    }
}

/// A tool that opts out of a bound (`question`, `bash`, `test`) must not be handed one by the
/// blanket override: the point of opting out is that no harness-wide number applies to it.
fn schema_with(properties: &[&str]) -> crate::provider::ToolSchema {
    let properties: serde_json::Map<String, serde_json::Value> =
        properties.iter().map(|name| ((*name).to_string(), serde_json::json!({}))).collect();
    crate::provider::ToolSchema {
        name: "t".into(),
        description: String::new(),
        parameters: serde_json::json!({"type": "object", "properties": properties}),
    }
}

/// An argument the tool does not know is named, with what it probably meant; a declared one,
/// an alias, or a case or underscore variant is not.
#[test]
fn an_argument_the_tool_does_not_know_is_named() {
    let schema = schema_with(&["pattern", "lang", "paths", "activeForm"]);
    let args = serde_json::json!({
        "pattern": "x", "lang": "rust", "path": "src", "active_form": "y", "tint": 1
    });
    let stray = stray_arguments(&schema, &[], &args);
    assert_eq!(
        stray,
        vec![("path".to_string(), Some("paths".to_string())), ("tint".to_string(), None)]
    );
    assert!(stray_arguments(&schema, &["path", "tint"], &args).is_empty(), "declared aliases");
    let note = stray_note("ast_grep", &stray).expect("a note");
    assert_eq!(
        note,
        "Note: `path` is not an argument of `ast_grep`, so it was ignored. Did you mean `paths`?\n\
             Note: `tint` is not an argument of `ast_grep`, so it was ignored."
    );
    assert_eq!(stray_note("t", &[]), None);
    // a schema without properties declares nothing to hold a call to
    let open = crate::provider::ToolSchema {
        name: "t".into(),
        description: String::new(),
        parameters: serde_json::json!({"type": "object"}),
    };
    assert!(stray_arguments(&open, &[], &args).is_empty());
}

#[test]
fn the_nearest_name_is_close_or_nothing() {
    let declared = ["offset", "limit", "paths"];
    assert_eq!(nearest_name("ofset", declared.into_iter()).as_deref(), Some("offset"));
    assert_eq!(nearest_name("lmit", declared.into_iter()).as_deref(), Some("limit"));
    assert_eq!(nearest_name("path", declared.into_iter()).as_deref(), Some("paths"));
    assert_eq!(nearest_name("query", declared.into_iter()), None);
    assert_eq!(edit_distance("kitten", "sitting"), 3);
    assert_eq!(edit_distance("", "abc"), 3);
}

#[test]
fn the_timeout_override_replaces_declared_bounds_and_grants_none() {
    struct Unbounded;
    #[async_trait]
    impl Tool for Unbounded {
        fn name(&self) -> &str {
            "unbounded"
        }
        fn schema(&self) -> ToolSchema {
            ToolSchema {
                name: "unbounded".into(),
                description: String::new(),
                parameters: serde_json::json!({}),
            }
        }
        fn timeout(&self) -> Option<Duration> {
            None
        }
        async fn run(&self, _args: serde_json::Value, _cx: &ToolContext) -> ToolResultData {
            ToolResultData { content: vec![], is_error: false }
        }
    }

    struct Restore(Option<String>);
    impl Drop for Restore {
        fn drop(&mut self) {
            match self.0.take() {
                Some(value) => std::env::set_var(TIMEOUT_ENV, value),
                None => std::env::remove_var(TIMEOUT_ENV),
            }
        }
    }
    let _restore = Restore(std::env::var(TIMEOUT_ENV).ok());

    let default: &dyn Tool = &NamedTool("read");
    let unbounded: &dyn Tool = &Unbounded;

    std::env::remove_var(TIMEOUT_ENV);
    assert_eq!(effective_timeout(default), Some(DEFAULT_TOOL_TIMEOUT));
    assert_eq!(effective_timeout(unbounded), None);

    std::env::set_var(TIMEOUT_ENV, "7");
    assert_eq!(effective_timeout(default), Some(Duration::from_secs(7)));
    assert_eq!(effective_timeout(unbounded), None, "an opt-out stays opted out");

    // A typo or a zero must not quietly remove every bound in the process.
    for bad in ["nonsense", "0", ""] {
        std::env::set_var(TIMEOUT_ENV, bad);
        assert_eq!(effective_timeout(default), Some(DEFAULT_TOOL_TIMEOUT), "{bad:?}");
    }
}

/// Registration order must not leak into `names()`/`schemas()`: both are sorted so the
/// request prefix (tools array + `## Tools` prompt section) is byte-stable for prompt
/// caching, regardless of the backing `HashMap`'s per-process iteration order.
#[test]
fn registry_output_is_sorted_by_name() {
    let mut reg = ToolRegistry::new();
    for name in ["write", "bash", "read", "edit", "git"] {
        reg.register(Arc::new(NamedTool(name)));
    }
    assert_eq!(reg.names(), vec!["bash", "edit", "git", "read", "write"]);
    let schemas = reg.schemas();
    let schema_names: Vec<&str> = schemas.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(schema_names, vec!["bash", "edit", "git", "read", "write"]);
}

fn tiered_registry() -> ToolRegistry {
    let mut reg = ToolRegistry::new();
    for name in ["read", "bash", "web_fetch", "screenshot", ACTIVATE_TOOLS_NAME] {
        reg.register(Arc::new(NamedTool(name)));
    }
    reg.set_tiers(HashSet::from(["read".to_string(), "bash".to_string()]));
    reg
}

fn names(schemas: Vec<ToolSchema>) -> Vec<String> {
    schemas.into_iter().map(|schema| schema.name).collect()
}

/// Untiered registries expose everything; the tiered helpers are inert.
#[test]
fn untiered_registry_exposes_all_and_defers_nothing() {
    let mut reg = ToolRegistry::new();
    reg.register(Arc::new(NamedTool("read")));
    reg.register(Arc::new(NamedTool("web_fetch")));
    assert!(!reg.is_tiered());
    assert_eq!(reg.request_schemas(&BTreeSet::new()).len(), 2);
    assert_eq!(reg.prompt_names(), vec!["read", "web_fetch"]);
    assert!(reg.deferred_catalog().is_empty());
    assert!(reg.deferred_schemas().is_empty());
}

/// With tiering, only core + the activate meta-tool ship until a deferred tool is activated,
/// and the deferred list holds every other tool whether it's been activated or not.
#[test]
fn tiered_registry_defers_until_activated() {
    let reg = tiered_registry();
    let none = BTreeSet::new();
    assert_eq!(
        names(reg.request_schemas(&none)),
        vec![ACTIVATE_TOOLS_NAME.to_string(), "bash".into(), "read".into()]
    );
    assert_eq!(names(reg.deferred_schemas()), vec!["screenshot", "web_fetch"]);

    // An unregistered name and a core one change nothing
    let activated = BTreeSet::from(["web_fetch".into(), "nope".into(), "read".into()]);
    assert_eq!(
        names(reg.request_schemas(&activated)),
        vec![ACTIVATE_TOOLS_NAME.to_string(), "bash".into(), "read".into(), "web_fetch".into()]
    );
    assert_eq!(
        names(reg.deferred_schemas()),
        vec!["screenshot", "web_fetch"],
        "activation never moves a tool out of the deferred list"
    );
}

/// What the prompt is built from does not move when the model activates a tool.
///
/// The break this catches, caught by `luxe wire` on a loop run: the `## Tools` section and the
/// on-demand catalog were both derived from the *activated* set, so the system prompt was
/// rewritten mid-session — and a rewritten prefix is re-billed at the write rate for every
/// token after it. The request's schemas still change, which is the whole point of lazy tools;
/// the prompt above them must not.
#[test]
fn activating_a_tool_leaves_the_prompt_s_view_alone() {
    let reg = tiered_registry();
    let before_names = reg.prompt_names();
    let before_catalog: Vec<String> =
        reg.deferred_catalog().into_iter().map(|(name, _)| name).collect();
    assert_eq!(before_names, vec![ACTIVATE_TOOLS_NAME, "bash", "read"]);
    assert_eq!(before_catalog, vec!["screenshot".to_string(), "web_fetch".to_string()]);

    let activated = BTreeSet::from(["web_fetch".to_string()]);

    assert_eq!(reg.prompt_names(), before_names, "the prompt's tool set is the core tier");
    let after_catalog: Vec<String> =
        reg.deferred_catalog().into_iter().map(|(name, _)| name).collect();
    assert_eq!(after_catalog, before_catalog, "and the catalog it lists is every deferred tool");
    assert!(
        reg.request_schemas(&activated).iter().any(|schema| schema.name == "web_fetch"),
        "while the request itself does carry the newly activated schema"
    );
}

/// The two ways a tool answers in text: a result, or a failure said as written.
#[test]
fn a_text_result_and_a_text_error_differ_only_in_the_flag() {
    let ok = ToolResultData::text("done");
    assert!(!ok.is_error);
    assert!(matches!(ok.content.as_slice(), [ContentBlock::Text(text)] if text == "done"));
    let failed = ToolResultData::error(String::from("Error: no"));
    assert!(failed.is_error);
    assert!(matches!(failed.content.as_slice(), [ContentBlock::Text(text)] if text == "Error: no"));
}
