//! The session's working directory, the preview's files, and what an applied rewrite announces.

use super::*;
use luxe_core::hook::{Hook, HookCtx, HookEvent, HookOutcome, HookPoint, HookRegistry};
use std::sync::{Arc, Mutex};

fn context(cwd: &std::path::Path) -> ToolContext {
    let (_, cancelled) = tokio::sync::watch::channel(false);
    let (event_tx, _) = tokio::sync::mpsc::channel(8);
    ToolContext {
        cwd: cwd.into(),
        session_id: "ast-cwd".into(),
        cancelled: Arc::new(cancelled),
        event_tx,
        hook_ctx: None,
        question_broker: None,
    }
}

/// A stub that logs each invocation's directory and arguments, then reports one match in `a.rs`.
fn stub(dir: &std::path::Path) -> std::path::PathBuf {
    let log = dir.join("calls.log");
    fake_binary(
        dir,
        "ast-cwd-cli",
        &format!(
            "#!/bin/sh\nprintf '%s | %s\\n' \"$PWD\" \"$*\" >> {log}\n\
             echo '{{\"text\":\"foo(1)\",\"replacement\":\"bar(1)\",\"range\":{{\"byteOffset\":\
             {{\"start\":0,\"end\":6}},\"start\":{{\"line\":0,\"column\":0}}}},\"file\":\"a.rs\"}}'\n",
            log = log.display()
        ),
    );
    BINARY.set(dir.join("ast-cwd-cli").to_string_lossy().into_owned()).unwrap();
    log
}

/// A session in a worktree must search that tree, not wherever the process happened to start.
#[tokio::test]
async fn the_matcher_runs_in_the_sessions_working_directory() {
    let bin = tempfile::tempdir().unwrap();
    let log = stub(bin.path());
    let project = tempfile::tempdir().unwrap();
    let args = serde_json::json!({"action": "search", "pattern": "foo($A)", "lang": "rust"});
    AstGrepTool::new().run(args, &context(project.path())).await;
    let calls = std::fs::read_to_string(log).unwrap();
    let project = std::fs::canonicalize(project.path()).unwrap();
    assert!(calls.starts_with(&format!("{} |", project.display())), "{calls}");
}

struct Announced(Arc<Mutex<Vec<String>>>);

#[async_trait]
impl Hook for Announced {
    fn name(&self) -> &'static str {
        "announced"
    }
    fn interest(&self) -> &[HookPoint] {
        &[HookPoint::FileChanged]
    }
    async fn on(&self, _: HookPoint, event: &HookEvent, _: &HookCtx) -> HookOutcome {
        if let HookEvent::FileChanged { path, .. } = event {
            self.0.lock().unwrap().push(path.clone());
        }
        HookOutcome::InjectContext("diagnostics: clean".into())
    }
}

/// The write pass rewrites what the preview showed, not a fresh scan of the whole scope, and each
/// rewritten file goes through `FileChanged` and the read tracker like an `edit` would.
#[tokio::test]
async fn an_applied_rewrite_writes_the_previewed_files_and_announces_them() {
    let bin = tempfile::tempdir().unwrap();
    let log = stub(bin.path());
    let project = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("a.rs"), "foo(1)").unwrap();
    let announced = Arc::new(Mutex::new(Vec::new()));
    let mut registry = HookRegistry::new();
    registry.register(Arc::new(Announced(announced.clone())));
    let mut cx = context(project.path());
    cx.hook_ctx = Some(HookCtx {
        session_id: cx.session_id.clone(),
        event_tx: cx.event_tx.clone(),
        hook_registry: registry,
    });
    let tracker = Arc::new(luxe_core::read_tracker::ReadTracker::new());
    let tool = AstGrepTool::new().with_tracker(tracker.clone());
    let args = serde_json::json!({"action": "replace", "pattern": "foo($A)", "rewrite": "bar($A)",
        "lang": "rust", "paths": ["src", "."], "apply": true});
    let result = tool.run(args, &cx).await;
    assert!(!result.is_error, "{result:?}");
    let calls = std::fs::read_to_string(log).unwrap();
    let update = calls.lines().find(|line| line.contains("--update-all")).expect(&calls);
    assert!(update.ends_with(" a.rs"), "{update}");
    assert!(!update.contains(" src") && !update.contains(" ."), "{update}");
    let file = project.path().join("a.rs");
    assert_eq!(*announced.lock().unwrap(), [luxe_core::paths::display(&file)]);
    assert_eq!(tracker.status("ast-cwd", &file), luxe_core::read_tracker::ReadStatus::Current);
    assert!(format!("{result:?}").contains("diagnostics: clean"), "{result:?}");
}

/// A batch that fails after earlier ones were rewritten must not read as nothing having happened.
#[tokio::test]
async fn a_failed_batch_reports_the_rewrites_already_on_disk() {
    let bin = tempfile::tempdir().unwrap();
    fake_binary(
        bin.path(),
        "ast-batch-cli",
        "#!/bin/sh\ncase \"$*\" in *--update-all*b.rs*) echo 'disk full' >&2; exit 1;; esac\n\
         case \"$*\" in *--update-all*) exit 0;; esac\n\
         for f in \"$@\"; do case \"$f\" in *.rs) printf '{\"text\":\"foo(1)\",\"replacement\":\"bar(1)\",\
         \"range\":{\"byteOffset\":{\"start\":0,\"end\":6},\"start\":{\"line\":0,\"column\":0}},\
         \"file\":\"%s\"}\\n' \"$f\";; esac; done\n",
    );
    BINARY.set(bin.path().join("ast-batch-cli").to_string_lossy().into_owned()).unwrap();
    let project = tempfile::tempdir().unwrap();
    for file in ["a.rs", "b.rs"] {
        std::fs::write(project.path().join(file), "foo(1)").unwrap();
    }
    let args = serde_json::json!({"action": "replace", "pattern": "foo($A)", "rewrite": "bar($A)",
        "lang": "rust", "apply": true});
    let call = super::super::guarded::Call::parse(args).unwrap();
    let batches = vec![vec!["a.rs".to_string()], vec!["b.rs".to_string()]];
    let result = call.over(&AstGrepTool::new(), batches, &context(project.path())).await;
    let text = format!("{result:?}");
    assert!(result.is_error, "{text}");
    assert!(text.contains("rewrote 1 match") && text.contains("a.rs"), "{text}");
    assert!(text.contains("disk full") && text.contains("on disk"), "{text}");
}

/// A file named like a flag reaches the matcher as a file: a repository cannot turn a search into a
/// rewrite with a file called `-U`.
#[tokio::test]
async fn paths_follow_an_end_of_options_marker() {
    let bin = tempfile::tempdir().unwrap();
    let log = stub(bin.path());
    let project = tempfile::tempdir().unwrap();
    let args = serde_json::json!({"action": "replace", "pattern": "foo($A)", "rewrite": "bar($A)",
        "lang": "rust", "paths": ["-U"], "apply": true});
    AstGrepTool::new().run(args, &context(project.path())).await;
    let calls = std::fs::read_to_string(log).unwrap();
    let mut lines = calls.lines();
    assert!(lines.next().unwrap().ends_with(" -- -U"), "{calls}");
    assert!(lines.next().unwrap().ends_with("--update-all -- a.rs"), "{calls}");
}
