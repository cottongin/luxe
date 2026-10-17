use super::*;
use luxe_core::tool::FileReadHook;
use std::path::Path;

type ObservedReads = Arc<Mutex<Vec<(String, PermissionAction)>>>;

struct ReadProbeBash {
    observed: ObservedReads,
}

#[async_trait::async_trait]
impl Tool for ReadProbeBash {
    fn name(&self) -> &str {
        "bash"
    }

    fn schema(&self) -> luxe_core::provider::ToolSchema {
        luxe_core::provider::ToolSchema {
            name: "bash".into(),
            description: "Probe discovered reads.".into(),
            parameters: serde_json::json!({"type": "object", "properties": {"command": {"type": "string"}}}),
        }
    }

    async fn run(&self, _: serde_json::Value, _: &ToolContext) -> ToolResultData {
        ToolResultData { content: vec![ContentBlock::Text("legacy-run".into())], is_error: false }
    }

    async fn run_with_read_hook(
        &self,
        _: serde_json::Value,
        _: &ToolContext,
        read_hook: &dyn FileReadHook,
    ) -> ToolResultData {
        for path in [".env", ".env.example", "review.md"] {
            self.observed.lock().await.push((path.into(), read_hook.decision(Path::new(path))));
        }
        ToolResultData {
            content: vec![ContentBlock::Text("read-hook-run".into())],
            is_error: false,
        }
    }
}

/// Bypassing the read-hook run entry would lose env denial; a discovered file that asks resolves
/// the way this run resolves `ask`, so headless auto-allow does not silently withhold it.
#[tokio::test]
async fn bounded_tool_dispatch_passes_the_current_read_policy_snapshot() {
    for (ask, review) in [
        (AskResolution::AutoAllow, PermissionAction::Allow),
        (AskResolution::AutoDeny, PermissionAction::Deny),
        (AskResolution::Prompt, PermissionAction::Ask),
    ] {
        assert_eq!(
            discovered_reads(ask).await,
            [
                (".env".into(), PermissionAction::Deny),
                (".env.example".into(), PermissionAction::Allow),
                ("review.md".into(), review),
            ],
            "{ask:?}"
        );
    }
}

async fn discovered_reads(ask: AskResolution) -> Vec<(String, PermissionAction)> {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/read-hook-test").unwrap();
    let provider = Arc::new(BashCaller::running("echo ok"));
    let global = PermissionConfig::Keyed(
        [(
            "read".into(),
            PermValue::Map([("review.md".into(), PermissionAction::Ask)].into_iter().collect()),
        )]
        .into_iter()
        .collect(),
    )
    .to_ruleset();
    let permission =
        PermissionSettings { engine: PermissionEngine::new(global, vec![], false), ask };
    let mut cfg = config(
        store,
        session_id,
        provider,
        Arc::new(AtomicUsize::new(0)),
        Arc::new(Mutex::new(Vec::new())),
        permission,
        HookRegistry::new(),
        None,
    );
    let observed = Arc::new(Mutex::new(Vec::new()));
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(ReadProbeBash { observed: observed.clone() }));
    cfg.tool_registry = registry;
    let (tx, mut rx) = Engine::spawn(cfg);
    let (_, results) = run_turn_collecting_results(&tx, &mut rx, "probe reads").await;
    assert!(results.iter().any(|result| result.contains("read-hook-run")), "{results:?}");
    let observed = observed.lock().await.clone();
    observed
}

/// Yolo opts out of default guards without silently discarding a user's read-specific denial.
#[test]
fn the_read_hook_honors_yolo_and_explicit_user_denials() {
    let yolo = PermissionEngine::assemble(
        luxe_core::permission::PermissionMode::Yolo,
        vec![],
        vec![],
        false,
    );
    assert_eq!(yolo.decision(Path::new(".env")), PermissionAction::Allow);
    let global = PermissionConfig::Keyed(
        [(
            "read".into(),
            PermValue::Map([("blocked.txt".into(), PermissionAction::Deny)].into_iter().collect()),
        )]
        .into_iter()
        .collect(),
    )
    .to_ruleset();
    let guarded = PermissionEngine::assemble(
        luxe_core::permission::PermissionMode::Yolo,
        global,
        vec![],
        false,
    );
    assert_eq!(guarded.decision(Path::new("blocked.txt")), PermissionAction::Deny);
}
