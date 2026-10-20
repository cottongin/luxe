//! Live `gdb --interpreter=dap` debugging — the first test of this crate against a real debugger.
//!
//! Everything else here drives a **stub** adapter: a shell script that speaks just enough DAP to
//! answer the choreography. That proves luxe's side of the protocol and nothing about a real one,
//! which is how twelve lines of GPL notice came to ride into the model's context on every gdb
//! launch — a stub has no banner to emit, so no test could see it.
//!
//! Skips unless `gdb` (14+, for DAP) and a C compiler are both present.
//!
//! ```bash
//! cargo nextest run -p luxe-dap --run-ignored all -E 'test(real_gdb)' --no-capture
//! ```

use luxe_core::tool::{Tool, ToolContext};
use luxe_dap::DapService;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, watch};

fn context(cwd: &std::path::Path) -> ToolContext {
    let (event_tx, _event_rx) = mpsc::channel(64);
    let (_cancel_tx, cancel_rx) = watch::channel(false);
    ToolContext {
        cwd: cwd.to_path_buf(),
        session_id: "real-gdb".to_string(),
        cancelled: Arc::new(cancel_rx),
        event_tx,
        hook_ctx: None,
        question_broker: None,
    }
}

fn on_path(program: &str) -> bool {
    luxe_core::program::resolve(program).is_some()
}

/// Whether this `gdb` speaks DAP at all (it arrived in GDB 14).
fn gdb_speaks_dap() -> bool {
    let Ok(output) = std::process::Command::new("gdb").arg("--version").output() else {
        return false;
    };
    let banner = String::from_utf8_lossy(&output.stdout);
    banner
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().last())
        .and_then(|version| version.split('.').next()?.parse::<u32>().ok())
        .is_some_and(|major| major >= 14)
}

/// A two-function C program, compiled with debug info.
fn build(dir: &std::path::Path) -> Option<String> {
    std::fs::write(
        dir.join("main.c"),
        "#include <stdio.h>\n\
         int add(int a, int b) { int sum = a + b; return sum; }\n\
         int main(void) { int total = add(2, 3); printf(\"%d\\n\", total); return 0; }\n",
    )
    .ok()?;
    let binary = dir.join("app");
    let built = std::process::Command::new("cc")
        .args(["-g", "-O0", "-o"])
        .arg(&binary)
        .arg(dir.join("main.c"))
        .status()
        .ok()?;
    built.success().then(|| binary.to_string_lossy().into_owned())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "drives a real gdb; run with --run-ignored"]
async fn real_gdb_stops_at_a_breakpoint_and_reports_the_frame() {
    if !on_path("gdb") || !gdb_speaks_dap() || !on_path("cc") {
        eprintln!("skipping: needs gdb 14+ (DAP) and a C compiler");
        return;
    }
    let dir_guard = tempfile::tempdir().expect("temp dir");
    let dir = dir_guard.path().to_path_buf();
    std::fs::create_dir_all(&dir).expect("temp dir");
    let Some(program) = build(&dir) else {
        eprintln!("skipping: the fixture would not compile");
        return;
    };

    let service = DapService::new();
    let outcome = tokio::time::timeout(
        Duration::from_secs(60),
        service.launch(
            program,
            vec![],
            dir.to_string_lossy().into_owned(),
            false,
            vec![luxe_dap::BreakpointRequest {
                path: dir.join("main.c").to_string_lossy().into_owned(),
                breakpoints: vec![luxe_dap::SourceBreakpoint { line: 2, condition: None }],
            }],
            Some("gdb".to_string()),
        ),
    )
    .await
    .expect("gdb answers inside a minute")
    .expect("gdb launches the program");

    eprintln!("STOP: {:?}", outcome.stop);
    eprintln!("OUTPUT: {:?}", outcome.output);

    let stop = outcome.stop.as_ref().expect("a breakpoint on line 2 must stop the program");
    assert_eq!(stop.reason, "breakpoint", "{stop:?}");

    // The adapter's own startup chatter is not the program's output. gdb opens with its version
    // and GPL notice tagged as `stdout`, which is indistinguishable from program output by
    // category alone — so it is dropped by *when* it arrived, before the program existed.
    assert!(
        !outcome.output.contains("GNU gdb"),
        "the adapter's banner is not program output: {:?}",
        outcome.output
    );
    assert!(!outcome.output.contains("GPL"), "nor is its license: {:?}", outcome.output);

    service.terminate_all().await;
}

/// Every remaining `debug` action, driven through the tool against real gdb.
///
/// `debug` carries the largest schema in the set and, until now, the least evidence: its whole
/// suite drove a shell-script stub that answers the choreography and nothing else. A stub cannot
/// disagree with a real debugger about a frame id, a variables reference, or what `evaluate`
/// accepts — so these actions were, in practice, untested.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "drives a real gdb; run with --run-ignored"]
async fn real_gdb_walks_the_whole_debug_surface() {
    if !on_path("gdb") || !gdb_speaks_dap() || !on_path("cc") {
        eprintln!("skipping: needs gdb 14+ (DAP) and a C compiler");
        return;
    }
    let dir_guard = tempfile::tempdir().expect("temp dir");
    let dir = dir_guard.path().to_path_buf();
    std::fs::create_dir_all(&dir).expect("temp dir");
    let Some(program) = build(&dir) else {
        eprintln!("skipping: the fixture would not compile");
        return;
    };
    let source = dir.join("main.c").to_string_lossy().into_owned();
    let cwd = dir.to_string_lossy().into_owned();

    let service = Arc::new(DapService::new());
    let tool = luxe_dap::DebugTool::new(service.clone());
    let cx = context(&dir);
    let call = |args: serde_json::Value| {
        let tool = &tool;
        let cx = &cx;
        async move {
            let result = tool.run(args, cx).await;
            let text = result
                .content
                .iter()
                .filter_map(|block| match block {
                    luxe_core::ContentBlock::Text(text) => Some(text.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            (text, result.is_error)
        }
    };

    // launch{program, cwd, stopOnEntry} — stop before any user code runs.
    let (launched, failed) = call(json!({
        "action": "launch", "program": program, "cwd": cwd, "adapter": "gdb", "stopOnEntry": true
    }))
    .await;
    assert!(!failed, "launch: {launched}");
    eprintln!("LAUNCH:\n{launched}");
    let session = launched
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("session "))
        .and_then(|rest| rest.split_whitespace().next())
        .expect("launch names its session")
        .to_string();

    // set_function_breakpoints{functions} — a breakpoint by name, not by line.
    let (set, failed) = call(
        json!({"action": "set_function_breakpoints", "session": session, "functions": ["add"]}),
    )
    .await;
    assert!(!failed, "set_function_breakpoints: {set}");
    assert!(set.contains("1 function breakpoint"), "{set}");

    // set_breakpoints{path, lines, condition} — the conditional form, which the stub never tested.
    let (set, failed) = call(json!({
        "action": "set_breakpoints", "session": session, "path": source,
        "lines": [3], "condition": "total == 5"
    }))
    .await;
    assert!(!failed, "set_breakpoints: {set}");
    eprintln!("BREAKPOINTS: {set}");

    // continue → the function breakpoint in add().
    let (stopped, failed) = call(json!({"action": "continue", "session": session})).await;
    assert!(!failed, "continue: {stopped}");
    eprintln!("CONTINUE:\n{stopped}");
    assert!(stopped.contains("add"), "the function breakpoint should stop in add: {stopped}");

    // threads → at least the main thread, with an id the other actions take.
    let (threads, failed) = call(json!({"action": "threads", "session": session})).await;
    assert!(!failed, "threads: {threads}");
    let thread_id: i64 = threads
        .lines()
        .next()
        .and_then(|line| line.split(':').next())
        .and_then(|id| id.trim().parse().ok())
        .unwrap_or_else(|| panic!("threads lists an id: {threads}"));

    // stack_trace{threadId} → frames, whose ids `scopes` takes.
    let (frames, failed) =
        call(json!({"action": "stack_trace", "session": session, "threadId": thread_id})).await;
    assert!(!failed, "stack_trace: {frames}");
    eprintln!("STACK:\n{frames}");
    assert!(frames.contains("add"), "the top frame is add: {frames}");
    let frame_id: i64 = frames
        .lines()
        .next()
        .and_then(|line| line.strip_prefix('#'))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|id| id.parse().ok())
        .unwrap_or_else(|| panic!("a frame carries an id: {frames}"));

    // scopes{frameId} → a variables reference, which is the only way to reach `variables`.
    let (scopes, failed) =
        call(json!({"action": "scopes", "session": session, "frameId": frame_id})).await;
    assert!(!failed, "scopes: {scopes}");
    eprintln!("SCOPES:\n{scopes}");
    let vref: i64 = scopes
        .lines()
        .find_map(|line| line.rsplit_once("ref ").map(|(_, r)| r.trim_end_matches(')').to_string()))
        .and_then(|r| r.parse().ok())
        .unwrap_or_else(|| panic!("a scope carries a variables reference: {scopes}"));

    // variables{variablesReference} → the arguments this frame was called with.
    let (vars, failed) =
        call(json!({"action": "variables", "session": session, "variablesReference": vref})).await;
    assert!(!failed, "variables: {vars}");
    eprintln!("VARIABLES:\n{vars}");
    assert!(vars.contains('a') && vars.contains('2'), "a = 2 is in scope: {vars}");

    // set_variable{variablesReference, name, value} → and evaluate proves it took.
    let (updated, failed) = call(json!({
        "action": "set_variable", "session": session, "variablesReference": vref,
        "name": "b", "value": "40"
    }))
    .await;
    assert!(!failed, "set_variable: {updated}");
    eprintln!("SET VARIABLE: {updated}");

    // evaluate{expression} → reads the frame we are stopped in, without being handed a frameId.
    let (evaluated, failed) =
        call(json!({"action": "evaluate", "session": session, "expression": "a + b"})).await;
    assert!(!failed, "evaluate: {evaluated}");
    eprintln!("EVALUATE a + b = {evaluated}");
    assert!(
        evaluated.contains("42"),
        "set_variable must have changed what evaluate sees: {evaluated}"
    );

    // next — a step that gdb reports, from a frame where there is somewhere to go.
    //
    // Only `next` is asserted here. gdb answers `stepIn` with `success: true` and then sends no
    // event at all when there is nothing to step into (confirmed against the raw protocol), and no
    // client can invent a stop the adapter never reported — what luxe owes there is a *reply that
    // says so*, which `a_resume_that_never_stops_says_the_program_is_running` covers.
    let (stepped, failed) =
        call(json!({"action": "next", "session": session, "threadId": thread_id})).await;
    assert!(!failed, "next: {stepped}");
    eprintln!("NEXT: {}", stepped.lines().next().unwrap_or(""));

    // sessions → this one, listed.
    let (sessions, failed) = call(json!({"action": "sessions"})).await;
    assert!(!failed, "sessions: {sessions}");
    assert!(sessions.contains(&session), "the live session is listed: {sessions}");

    // output → whatever the program has printed, without the adapter's banner.
    let (output, failed) = call(json!({"action": "output", "session": session})).await;
    assert!(!failed, "output: {output}");
    assert!(!output.contains("GNU gdb"), "the adapter's banner is not output: {output}");

    // terminate → and the session stops being listed.
    let (ended, failed) = call(json!({"action": "terminate", "session": session})).await;
    assert!(!failed, "terminate: {ended}");
    let (sessions, _) = call(json!({"action": "sessions"})).await;
    assert!(
        !sessions.contains(&session),
        "a terminated session must not still be listed: {sessions}"
    );
}

/// `launch{args, breakpoints}` and `attach{pid}` — the last of the schema, against real gdb.
///
/// `args` and the launch-time `breakpoints` array had never reached a debugger: the walk test sets
/// breakpoints as a follow-up action, and a stub cannot tell whether an adapter received argv at
/// all. `attach` was stub-only, which proves the choreography and nothing about a real process.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "drives a real gdb; run with --run-ignored"]
async fn real_gdb_takes_program_arguments() {
    if !on_path("gdb") || !gdb_speaks_dap() || !on_path("cc") {
        eprintln!("skipping: needs gdb 14+ (DAP) and a C compiler");
        return;
    }
    let dir_guard = tempfile::tempdir().expect("temp dir");
    let dir = dir_guard.path().to_path_buf();
    std::fs::create_dir_all(&dir).expect("temp dir");

    // Reads argv into a local, so `args` arriving is observable in a variable rather than inferred.
    std::fs::write(
        dir.join("main.c"),
        "#include <stdio.h>\n\
         int main(int argc, char **argv) {\n\
         \x20   int seen = argc;\n\
         \x20   printf(\"%s\\n\", argv[argc - 1]);\n\
         \x20   return seen;\n\
         }\n",
    )
    .expect("write");
    let binary = dir.join("app");
    assert!(
        std::process::Command::new("cc")
            .args(["-g", "-O0", "-o"])
            .arg(&binary)
            .arg(dir.join("main.c"))
            .status()
            .expect("cc runs")
            .success(),
        "the fixture must compile"
    );
    let program = binary.to_string_lossy().into_owned();
    let source = dir.join("main.c").to_string_lossy().into_owned();
    let cwd = dir.to_string_lossy().into_owned();

    let service = Arc::new(DapService::new());
    let tool = luxe_dap::DebugTool::new(service.clone());
    let cx = context(&dir);
    let call = |args: serde_json::Value| {
        let (tool, cx) = (&tool, &cx);
        async move {
            let result = tool.run(args, cx).await;
            let text = result
                .content
                .iter()
                .filter_map(|block| match block {
                    luxe_core::ContentBlock::Text(text) => Some(text.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            (text, result.is_error)
        }
    };

    // launch{args, breakpoints} — three arguments, and a breakpoint set by the launch itself.
    let (launched, failed) = call(json!({
        "action": "launch", "program": program, "cwd": cwd, "adapter": "gdb",
        "args": ["alpha", "beta", "gamma"],
        "breakpoints": [{ "path": source, "lines": [4] }]
    }))
    .await;
    assert!(!failed, "launch with args: {launched}");
    eprintln!("LAUNCH+ARGS:\n{launched}");
    assert!(
        launched.contains("stopped"),
        "the launch-time breakpoint must stop the program: {launched}"
    );
    let session = launched
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("session "))
        .and_then(|rest| rest.split_whitespace().next())
        .expect("launch names its session")
        .to_string();

    // argc is 4: the program plus the three arguments. Anything else and `args` did not arrive.
    let (argc, failed) =
        call(json!({"action": "evaluate", "session": session, "expression": "argc"})).await;
    assert!(!failed, "evaluate argc: {argc}");
    eprintln!("ARGC: {argc}");
    assert!(argc.contains('4'), "three `args` plus the program name is argc 4, got: {argc}");

    let (argv, _) =
        call(json!({"action": "evaluate", "session": session, "expression": "argv[2]"})).await;
    eprintln!("ARGV[2]: {argv}");
    assert!(argv.contains("beta"), "the arguments arrive in order: {argv}");

    call(json!({"action": "terminate", "session": session})).await;
    service.terminate_all().await;
}

/// Attaching to a process this host forbids ptracing is reported, not waited out silently.
///
/// `kernel.yama.ptrace_scope = 1` — the default on most desktop Linux — forbids attaching to a
/// process you did not fork. gdb accepts the `attach` request anyway and then simply never reports
/// a stop, so the only thing luxe can do is say which situation this is. It used to answer
/// `request 'event' timed out`, which names an internal wait and offers nothing; the check here is
/// that the reply names the permission and the sysctl that governs it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "drives a real gdb; run with --run-ignored"]
async fn real_gdb_explains_an_attach_the_host_forbids() {
    if !on_path("gdb") || !gdb_speaks_dap() || !on_path("cc") {
        eprintln!("skipping: needs gdb 14+ (DAP) and a C compiler");
        return;
    }
    let scope = std::fs::read_to_string("/proc/sys/kernel/yama/ptrace_scope").unwrap_or_default();
    if scope.trim() == "0" {
        eprintln!("skipping: this host permits ptrace, so there is no refusal to report");
        return;
    }
    let dir_guard = tempfile::tempdir().expect("temp dir");
    let dir = dir_guard.path().to_path_buf();
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(
        dir.join("main.c"),
        "#include <unistd.h>\nint main(void) { for (int i = 0; i < 600; i++) sleep(1); return 0; }\n",
    )
    .expect("write");
    let binary = dir.join("app");
    assert!(
        std::process::Command::new("cc")
            .args(["-g", "-O0", "-o"])
            .arg(&binary)
            .arg(dir.join("main.c"))
            .status()
            .expect("cc runs")
            .success(),
        "the fixture must compile"
    );

    let mut running = std::process::Command::new(&binary)
        .stdout(std::process::Stdio::null())
        .spawn()
        .expect("the fixture runs");
    tokio::time::sleep(Duration::from_millis(300)).await;

    let service = DapService::new();
    let outcome = service
        .attach(
            Some(i64::from(running.id())),
            None,
            dir.to_string_lossy().into_owned(),
            vec![],
            Some("gdb".to_string()),
        )
        .await;

    // Killed before asserting: a panic must not leave the fixture running.
    let _ = running.kill();
    let _ = running.wait();
    service.terminate_all().await;

    let refusal = outcome.expect_err("this host forbids the attach").to_string();
    eprintln!("ATTACH REFUSAL: {refusal}");
    assert!(
        refusal.contains("ptrace_scope"),
        "the reply must name the permission that governs this: {refusal}"
    );
    assert!(
        !refusal.contains("'event'"),
        "naming an internal wait tells the caller nothing: {refusal}"
    );
}
