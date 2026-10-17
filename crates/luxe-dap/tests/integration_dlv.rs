//! Live Delve (Go) debugging test — exercises the TCP dial-in transport end-to-end.
//!
//! Skips unless both `dlv` and `go` are on PATH (CI won't have them). Builds a tiny Go module in a
//! temp dir and drives `DapService` through the real dlv DAP dial-in choreography.

use luxe_dap::DapService;
use std::time::Duration;

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
        .unwrap_or(false)
}

// Opt-in: this drives a real dlv + Go build, which is slow and environment-sensitive, so it is
// skipped unless `LUXE_TEST_DLV=1` (keeps the default suite fast and unpoisonable).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dlv_launches_go_and_hits_a_breakpoint() {
    if std::env::var("LUXE_TEST_DLV").as_deref() != Ok("1") {
        eprintln!("skipping dlv test (set LUXE_TEST_DLV=1 to run)");
        return;
    }
    if !on_path("dlv") || !on_path("go") {
        eprintln!("skipping: dlv/go not installed");
        return;
    }

    let dir_guard = tempfile::tempdir().expect("temp dir");

    let dir = dir_guard.path().to_path_buf();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("go.mod"), "module dbggo\ngo 1.21\n").unwrap();
    std::fs::write(
        dir.join("main.go"),
        "package main\n\nimport \"fmt\"\n\nfunc compute(n int) int {\n\ttotal := 0\n\tfor i := 0; i < n; i++ {\n\t\ttotal += i\n\t}\n\treturn total\n}\n\nfunc main() {\n\tfmt.Printf(\"result=%d\\n\", compute(5))\n}\n",
    )
    .unwrap();
    let cwd = dir.to_string_lossy().to_string();

    // Results go to a file so they survive even when the harness drops the test's stdout for
    // debugger-process-tree commands (read /tmp/dlv-result.txt afterward).
    let result_path = "/tmp/dlv-result.txt";
    std::fs::write(result_path, "=== dlv (Go) e2e ===\n").ok();
    let mark = |line: String| {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(result_path) {
            let _ = writeln!(f, "{line}");
            let _ = f.flush();
        }
    };

    let service = DapService::new();
    mark("launching (adapter=dlv, stopOnEntry)…".to_string());
    let outcome = tokio::time::timeout(
        Duration::from_secs(40),
        service.launch(
            cwd.clone(),
            Vec::new(),
            cwd.clone(),
            true, // stopOnEntry
            Vec::new(),
            Some("dlv".to_string()),
        ),
    )
    .await
    .expect("[dlv-test] launch hung >40s")
    .expect("launch should succeed");
    mark(format!(
        "launch OK: adapter={} terminated={} stop_reason={:?}",
        outcome.adapter_id,
        outcome.terminated,
        outcome.stop.as_ref().map(|s| s.reason.clone())
    ));
    assert_eq!(outcome.adapter_id, "dlv");
    assert!(outcome.stop.is_some(), "stopOnEntry should have stopped the program");

    // Set a breakpoint on the `total += i` line and continue to it.
    let session = service.get(&outcome.session_id).await.expect("session exists");
    let main_go = dir.join("main.go");
    let bps = tokio::time::timeout(
        Duration::from_secs(15),
        luxe_dap::service::set_breakpoints(
            &session.client,
            &main_go.to_string_lossy(),
            &[luxe_dap::SourceBreakpoint { line: 8, condition: None }],
        ),
    )
    .await
    .expect("set_breakpoints hung")
    .expect("set_breakpoints");
    mark(format!("breakpoint line 8: {} verified", bps.iter().filter(|b| b.verified).count()));
    assert!(bps.iter().any(|b| b.verified), "breakpoint should verify");

    let stop = tokio::time::timeout(Duration::from_secs(20), session.continue_(None))
        .await
        .expect("continue hung")
        .expect("continue");
    mark(format!("continue → stop_reason={:?}", stop.as_ref().map(|s| s.reason.clone())));
    assert!(stop.is_some(), "should stop at the breakpoint");

    // Capture the locals at the stop (the money shot: top frame → scopes → variables).
    if let Ok(frames) = session.stack_trace(None).await {
        if let Some(top) = frames.first() {
            mark(format!("top frame: {} @ line {}", top.name, top.line));
            if let Ok(scopes) = session.scopes(top.id).await {
                if let Some(scope) = scopes.iter().find(|s| !s.expensive) {
                    if let Ok(vars) = session.variables(scope.variables_reference).await {
                        for v in vars.iter().take(6) {
                            mark(format!("  local {} = {}", v.name, v.value));
                        }
                    }
                }
            }
        }
    }

    service.terminate(&outcome.session_id).await.ok();
    mark("terminated OK — dlv e2e PASSED".to_string());
}
