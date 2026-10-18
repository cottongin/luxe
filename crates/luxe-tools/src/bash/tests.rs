#![allow(clippy::unwrap_used)]

use super::*;
use std::sync::Arc;
use tokio::sync::{mpsc, watch};

fn test_context(tmp_dir: &std::path::Path) -> ToolContext {
    let (event_tx, _event_rx) = mpsc::channel(64);
    let (_cancel_tx, cancel_rx) = watch::channel(false);
    ToolContext {
        cwd: tmp_dir.to_path_buf(),
        session_id: "test-session".to_string(),
        cancelled: Arc::new(cancel_rx),
        event_tx,
        hook_ctx: None,
        question_broker: None,
    }
}

/// Whether the assertions below can mean anything on this machine.
///
/// They are written in POSIX — `sleep 10`, `echo 'x'`, a pipeline — because that is the dialect
/// the model writes and the one the tool promises. A Windows box with no Git for Windows has no
/// shell that speaks it, and `cmd`'s complaint about a quote would be a fixture failure dressed
/// as a product failure. The behavior *there* has its own assertion below.
fn posix_shell_available() -> bool {
    luxe_core::program::has_posix_shell()
}

/// A directory of this test's own, deleted when it is dropped; it reads as the path it holds.
struct Scratch(tempfile::TempDir);

impl std::ops::Deref for Scratch {
    type Target = std::path::Path;

    fn deref(&self) -> &Self::Target {
        self.0.path()
    }
}

fn scratch_dir(name: &str) -> Scratch {
    Scratch(
        tempfile::Builder::new()
            .prefix(&format!("luxe-test-{name}-"))
            .tempdir()
            .expect("scratch dir"),
    )
}

/// The last command is the one whose status the shell reports, and a pipe inside a quoted
/// pattern does not end it.
#[test]
fn a_search_is_found_at_the_end_of_the_command() {
    assert!(ends_in_search("grep -n foo src/lib.rs"));
    assert!(ends_in_search(r#"npm test 2>&1 | grep -E "^not ok|^# (pass|fail)""#));
    assert!(ends_in_search("cd x && LC_ALL=C /usr/bin/rg needle"));
    assert!(ends_in_search("ls; egrep 'a|b' notes"));
    assert!(!ends_in_search("grep foo src | wc -l"), "wc ends it");
    assert!(!ends_in_search("grep foo src 2>&1"), "a redirection after it is unclear");
    assert!(!ends_in_search("echo 'grep x'"));
    assert!(!ends_in_search("false"));
}

/// Nothing matched is an answer: no red card, and the model is told what exit 1 meant. A search
/// that complained, or any other failing command, is still a failure.
#[tokio::test]
async fn a_search_that_matches_nothing_is_not_a_failure() {
    if !posix_shell_available() {
        return;
    }
    let tmp_dir = scratch_dir("no-match");
    let tool = BashTool::new();
    let cx = test_context(&tmp_dir);
    let text = |result: &ToolResultData| match &result.content[0] {
        ContentBlock::Text(text) => text.clone(),
        _ => String::new(),
    };

    let quiet = tool.run(json!({"command": "printf 'ok\\n' | grep zzz"}), &cx).await;
    assert!(!quiet.is_error, "{}", text(&quiet));
    assert!(text(&quiet).contains("matched nothing"), "{}", text(&quiet));

    let missing = tool.run(json!({"command": "grep zzz no-such-file"}), &cx).await;
    assert!(missing.is_error, "a search that complained failed: {}", text(&missing));

    let failed = tool.run(json!({"command": "false"}), &cx).await;
    assert!(failed.is_error);
    assert!(!text(&failed).contains("matched nothing"));
}

#[tokio::test]
async fn bash_tool_basic() {
    if !posix_shell_available() {
        return;
    }
    let tmp_dir = scratch_dir("bash");

    let tool = BashTool::new();
    let cx = test_context(&tmp_dir);

    let args = json!({
        "command": "echo 'Hello, World!'"
    });
    let result = tool.run(args, &cx).await;

    assert!(!result.is_error);
    if let ContentBlock::Text(text) = &result.content[0] {
        assert!(text.contains("Hello, World!"));
        assert!(text.contains("Exit code: 0"));
    } else {
        panic!("expected Text block");
    }
}

#[tokio::test]
async fn bash_tool_timeout() {
    if !posix_shell_available() {
        return;
    }
    let tmp_dir = scratch_dir("bash-timeout");

    let tool = BashTool::new();
    let cx = test_context(&tmp_dir);

    let args = json!({
        "command": "sleep 10",
        "timeout": 1
    });
    let result = tool.run(args, &cx).await;

    assert!(result.is_error);
    if let ContentBlock::Text(text) = &result.content[0] {
        assert!(text.contains("timed out"));
    } else {
        panic!("expected Text block");
    }
}

/// A command that leaves something running returns when *it* is done.
///
/// The incident: `cd /tmp && nohup setsid app >log 2>&1 </dev/null &` followed by `sleep 8`.
/// Everything about it is right except that `&` backgrounds the whole `&&` list, so the
/// subshell — which never got the redirections — sat holding this call's pipe for as long as
/// the app was open. Waiting for the pipe to close meant waiting for the app to be closed;
/// the call reported `8s` and ran for a minute, and the reader interrupted it.
///
/// The reported command is kept verbatim, and what it is asserted to do is only the part that
/// does not depend on the shell: **whether the pipe is still held is bash's call, not ours.**
/// The long-lived command here redirects away from the pipe, so the only thing that can hold it
/// is the intermediate subshell — and whether one survives is an optimization. bash 5.3 forks
/// and keeps it; the 5.2 on CI execs the tail in place and the pipe closes with the shell. The
/// note that says output was cut short is therefore pinned by the test below, on a command that
/// holds the pipe on any shell.
#[tokio::test]
async fn a_process_left_running_does_not_hold_the_call_open() {
    if !posix_shell_available() {
        return;
    }
    let tmp_dir = scratch_dir("bash-detached");
    let tool = BashTool::new();
    let cx = test_context(&tmp_dir);

    let started = std::time::Instant::now();
    let result = tool
        .run(
            json!({
                "command": "true && sleep 30 >/dev/null 2>&1 </dev/null & \nsleep 1; echo ran",
                "timeout": 20,
            }),
            &cx,
        )
        .await;

    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the call ends with the command, not with what the command left running"
    );
    assert!(!result.is_error);
    let ContentBlock::Text(text) = &result.content[0] else { panic!("expected Text block") };
    assert!(text.contains("ran"), "the output it did produce is still there: {text}");
}

/// Output cut short by something still holding the pipe says so.
///
/// `sleep 30 &` with no redirection is the unambiguous shape: the child inherits this call's
/// stdout and keeps it for thirty seconds, which no shell can optimize away, so the drain grace
/// always expires. The reader is about to compare this output against what the command prints
/// in a terminal, and the difference is a detail of shell syntax rather than anything they got
/// wrong.
#[tokio::test]
async fn output_held_open_by_a_detached_writer_says_so() {
    if !posix_shell_available() {
        return;
    }
    let tmp_dir = scratch_dir("bash-detached-note");
    let tool = BashTool::new();
    let cx = test_context(&tmp_dir);

    let started = std::time::Instant::now();
    let result = tool.run(json!({ "command": "sleep 30 & echo ran", "timeout": 20 }), &cx).await;

    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the call ends with the command: {:?}",
        started.elapsed()
    );
    assert!(!result.is_error);
    let ContentBlock::Text(text) = &result.content[0] else { panic!("expected Text block") };
    assert!(text.contains("ran"), "the output it did produce is still there: {text}");
    assert!(
        text.contains("still held its output"),
        "and the reader is told why it might be short: {text}"
    );
}

#[tokio::test]
async fn bash_tool_nonzero_exit() {
    let tmp_dir = scratch_dir("bash-exit");

    let tool = BashTool::new();
    let cx = test_context(&tmp_dir);

    let args = json!({
        "command": "exit 1"
    });
    let result = tool.run(args, &cx).await;

    assert!(result.is_error);
    if let ContentBlock::Text(text) = &result.content[0] {
        assert!(text.contains("Exit code: 1"));
    } else {
        panic!("expected Text block");
    }
}

#[tokio::test]
async fn bash_tool_compresses_noisy_output_but_keeps_errors() {
    if !posix_shell_available() {
        return;
    }
    let tmp_dir = scratch_dir("bash-compress");
    let tool = BashTool::new();
    let cx = test_context(&tmp_dir);

    // 600 lines of build noise with one real error buried inside.
    let args = json!({
        "command": "for i in $(seq 1 600); do echo \"   Compiling crate_$i v1.0\"; done; \
                        echo 'error: boom at src/x.rs:9:1'"
    });
    let result = tool.run(args, &cx).await;
    let ContentBlock::Text(text) = &result.content[0] else {
        panic!("expected Text block");
    };
    assert!(text.contains("error: boom"), "buried error must survive: {text}");
    assert!(text.contains("src/x.rs:9:1"), "file:line must survive");
    assert!(text.contains("compressed"), "savings footer expected: {text}");
    assert!(text.contains("Full output:"), "artifact reference expected");
    let artifacts = tmp_dir.join(".luxe").join("artifacts");
    assert!(artifacts.is_dir() && artifacts.read_dir().unwrap().count() >= 1);
}

/// A suite run through the shell names what broke since its last run, as the `test` tool does.
#[cfg(unix)]
#[tokio::test]
async fn a_test_run_through_bash_names_its_regressions() {
    use std::os::unix::fs::PermissionsExt;
    if !posix_shell_available() {
        return;
    }
    let tmp_dir = scratch_dir("bash-regression");
    let script = tmp_dir.join("tests").join("runtests.py");
    std::fs::create_dir_all(script.parent().unwrap()).unwrap();
    std::fs::write(
        &script,
        "#!/bin/sh\n\
             if [ -f ../broken ]; then\n\
             printf 'test_a (app.tests.T) ... ok\\ntest_b (app.tests.T) ... FAIL\\n\\n'\n\
             printf '======================================================================\\n'\n\
             printf 'FAIL: test_b (app.tests.T)\\n'\n\
             printf -- '----------------------------------------------------------------------\\n'\n\
             printf 'AssertionError\\n\\n'\n\
             printf -- '----------------------------------------------------------------------\\n'\n\
             printf 'Ran 2 tests in 0.001s\\n\\nFAILED (failures=1)\\n' >&2; exit 1\n\
             fi\n\
             printf 'Ran 2 tests in 0.001s\\n\\nOK\\n' >&2\n",
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let tool = BashTool::new();
    let cx = test_context(&tmp_dir);
    let run = || async {
        let result =
            tool.run(json!({"command": "cd tests && ./runtests.py app 2>&1 | tail -20"}), &cx);
        let result = result.await;
        let ContentBlock::Text(text) = &result.content[0] else { panic!("text") };
        text.clone()
    };

    let green = run().await;
    assert!(!green.contains("REGRESSION"), "{green}");
    std::fs::write(tmp_dir.join("broken"), "").unwrap();
    let red = run().await;
    assert!(red.starts_with("REGRESSION — 1 test(s)"), "{red}");
    assert!(red.contains("test_b"), "{red}");
}

/// A Windows box without Git for Windows runs through `cmd`, which refuses most of what a
/// model writes. The tool must say *why* once, on failure, instead of handing back cmd's
/// complaint about a quote and letting the model debug its own perfectly good command.
///
/// `exit 1` is understood by both shells, so the assertion is about the annotation, not the
/// dialect: present when there is no POSIX shell, absent when there is.
#[tokio::test]
async fn a_failure_without_a_posix_shell_says_that_is_why() {
    let tmp_dir = scratch_dir("bash-noshell");
    let result = BashTool::new().run(json!({"command": "exit 1"}), &test_context(&tmp_dir)).await;
    let ContentBlock::Text(text) = &result.content[0] else { panic!("expected text") };

    assert_eq!(
        text.contains("no POSIX shell was found"),
        !posix_shell_available(),
        "the note appears exactly when cmd.exe is what ran it: {text}"
    );
}

#[tokio::test]
async fn bash_tool_missing_command() {
    let tmp_dir = scratch_dir("bash-mc");

    let tool = BashTool::new();
    let cx = test_context(&tmp_dir);

    let args = json!({});
    let result = tool.run(args, &cx).await;

    assert!(result.is_error);
}

/// The compressor works per line, so a single enormous line escaped it entirely and 500 KB
/// went straight into the context window. The clamp is the backstop.
#[tokio::test]
async fn one_enormous_line_is_clamped() {
    if !posix_shell_available() {
        return;
    }
    let tmp_dir = tempfile::tempdir().unwrap();
    let ctx = test_context(tmp_dir.path());
    let tool = BashTool::new();

    let result =
        tool.run(serde_json::json!({"command": "python3 -c \"print('x'*500000)\""}), &ctx).await;

    let ContentBlock::Text(text) = &result.content[0] else { panic!("expected text") };
    assert!(
        text.chars().count() < MAX_OUTPUT_BYTES + 2_000,
        "clamped, got {} chars",
        text.chars().count()
    );
    assert!(text.contains("clamped"), "and says it clamped: {}", &text[..200.min(text.len())]);
    assert!(text.contains("Full output: "), "with the path back to all of it");
    let artifacts = tmp_dir.path().join(".luxe").join("artifacts");
    assert_eq!(artifacts.read_dir().map(Iterator::count).unwrap_or(0), 1, "saved there");
}

/// A long line of multi-byte characters is cut between characters. The cut used to land on a
/// byte offset and panic whenever that offset was inside one.
#[test]
fn the_clamp_never_cuts_inside_a_character() {
    for unit in ["€", "é", "\u{1F600}", "x"] {
        for extra in 0..4 {
            let text = format!("{}{}", "x".repeat(extra), unit.repeat(MAX_OUTPUT_BYTES));
            let clamped = clamp_bytes(&text, MAX_OUTPUT_BYTES);
            if text.len() <= MAX_OUTPUT_BYTES {
                assert_eq!(clamped, text, "at the cap, untouched");
                continue;
            }
            assert!(clamped.len() <= MAX_OUTPUT_BYTES + 60, "{unit} {extra}: {}", clamped.len());
            let dropped: usize = clamped
                .split_once("-- clamped ")
                .and_then(|(_, rest)| rest.split_once(' '))
                .and_then(|(count, _)| count.parse().ok())
                .expect("says how much it dropped");
            let kept = clamped.len() - format!("\n-- clamped {dropped} bytes --\n").len();
            assert_eq!(kept + dropped, text.len(), "{unit} {extra}: the count is exact");
        }
    }
    assert_eq!(clamp_bytes("short", MAX_OUTPUT_BYTES), "short");
}

/// A command that runs out of time still says what it had printed, and how to give it longer.
#[tokio::test]
async fn a_timeout_keeps_what_the_command_printed() {
    if !posix_shell_available() {
        return;
    }
    let tmp_dir = tempfile::tempdir().unwrap();
    let ctx = test_context(tmp_dir.path());
    let result = BashTool::new()
        .run(serde_json::json!({"command": "echo started; sleep 5", "timeout": 1}), &ctx)
        .await;
    assert!(result.is_error);
    let ContentBlock::Text(text) = &result.content[0] else { panic!("expected text") };
    assert!(text.contains("timed out after 1 seconds"), "{text}");
    assert!(text.contains("Output before it was stopped:\nstarted"), "{text}");
    assert!(text.contains("pass `timeout`"), "{text}");

    let silent =
        BashTool::new().run(serde_json::json!({"command": "sleep 5", "timeout": 1}), &ctx).await;
    let ContentBlock::Text(text) = &silent.content[0] else { panic!("expected text") };
    assert!(text.contains("printed nothing"), "{text}");
}

/// A timeout names what was still running, so "slow" and "stuck" stop looking the same.
#[cfg(unix)]
#[tokio::test]
async fn a_timeout_lists_what_was_still_running() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let ctx = test_context(tmp_dir.path());
    let result = BashTool::new()
        .run(serde_json::json!({"command": "sleep 7 & sleep 8", "timeout": 1}), &ctx)
        .await;
    let ContentBlock::Text(text) = &result.content[0] else { panic!("expected text") };
    assert!(text.contains("Still running when it was stopped (pid, state, command):"), "{text}");
    assert!(text.contains("sleep 7"), "{text}");
    assert!(text.contains("pass `timeout`"), "nothing suspended, so more time may help: {text}");
    assert!(!text.contains("State T"), "{text}");
}

/// Marks the run of [`under_a_terminal`] that [`a_command_that_touches_the_terminal_still_finishes`]
/// starts, so a plain `--ignored` run of the suite skips it.
#[cfg(unix)]
const UNDER_A_TERMINAL: &str = "LUXE_TEST_UNDER_A_TERMINAL";

/// The omacvm incident, end to end: a background job that changes the terminal's settings, as a
/// spinner does with `stty -echo < /dev/tty`, then a foreground `echo`.
///
/// Before commands left Luxe's terminal, the kernel suspended the whole process group for that,
/// foreground shell included, and the call timed out having "printed nothing". Reproducing it
/// takes a real controlling terminal and a process not run by nextest (which runs tests where
/// none of this happens), so this re-runs its own test binary under `script(1)`.
#[cfg(unix)]
#[test]
fn a_command_that_touches_the_terminal_still_finishes() {
    let Some(script) = luxe_core::program::resolve("script") else {
        eprintln!("skipped: script(1) is not installed, so there is no terminal to test under");
        return;
    };
    let exe = std::env::current_exe().expect("the test binary");
    let inner = ["bash::tests::under_a_terminal", "--exact", "--ignored", "--nocapture"];
    let mut command = std::process::Command::new(script);
    if cfg!(target_os = "macos") {
        command.args(["-q", "/dev/null"]).arg(&exe).args(inner);
    } else {
        let quote = |text: &str| format!("'{}'", text.replace('\'', "'\\''"));
        let line = std::iter::once(quote(&exe.to_string_lossy()))
            .chain(inner.iter().map(|arg| quote(arg)))
            .collect::<Vec<_>>()
            .join(" ");
        command.args(["-qec", &line, "/dev/null"]);
    }
    let output = command
        .env(UNDER_A_TERMINAL, "1")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("script runs");
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if said.contains("NO_TERMINAL") {
        eprintln!("skipped: script(1) gave the test no controlling terminal");
        return;
    }
    assert!(said.contains("FINISHED_PAST_THE_TERMINAL"), "{said}");
}

/// The half of the test above that runs under the terminal. Reports by marker, because its
/// output reaches the outer test through `script`, not through the test harness.
#[cfg(unix)]
#[tokio::test]
#[ignore = "run under a real terminal by a_command_that_touches_the_terminal_still_finishes"]
async fn under_a_terminal() {
    if std::env::var_os(UNDER_A_TERMINAL).is_none() {
        return;
    }
    if std::fs::File::open("/dev/tty").is_err() {
        println!("NO_TERMINAL");
        return;
    }
    let tmp_dir = tempfile::tempdir().unwrap();
    let ctx = test_context(tmp_dir.path());
    let result = BashTool::new()
        .run(
            serde_json::json!({
                "command": "(stty -echo </dev/tty; sleep 20) & sleep 1; echo reached",
                "timeout": 8
            }),
            &ctx,
        )
        .await;
    let ContentBlock::Text(text) = &result.content[0] else { panic!("expected text") };
    if !result.is_error && text.contains("reached") {
        println!("FINISHED_PAST_THE_TERMINAL");
    } else {
        println!("STUCK: {text}");
    }
}

/// A suspended command could never finish, and the old advice was to give it longer. A terminal
/// can no longer suspend one (see `a_command_that_touches_the_terminal_still_finishes`), so this
/// stops one by hand.
///
/// The stop is sent a few times over the first quarter second: on macOS a `SIGSTOP` sent the instant
/// a child is spawned can miss it, and the report then rightly lists a sleeper, not a stopped one.
#[cfg(unix)]
#[tokio::test]
async fn a_suspended_command_is_called_stuck_not_slow() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let ctx = test_context(tmp_dir.path());
    let command = "sleep 9 & for _ in 1 2 3 4 5; do kill -STOP $!; sleep 0.05; done; sleep 8";
    let result =
        BashTool::new().run(serde_json::json!({"command": command, "timeout": 1}), &ctx).await;
    let ContentBlock::Text(text) = &result.content[0] else { panic!("expected text") };
    assert!(text.contains("State T means suspended, not slow"), "{text}");
    assert!(!text.contains("pass `timeout`"), "more time cannot help a suspended command: {text}");
}

#[test]
fn a_timeout_with_nothing_left_running_lists_nothing_and_a_long_list_is_counted() {
    use luxe_core::program::GroupMember;
    assert!(!timeout_advice(&[]).contains("Still running"));
    let member = |pid: u32| GroupMember {
        pid,
        state: "S".into(),
        command: format!("worker {pid} {}", "x".repeat(200)),
    };
    let crowd: Vec<GroupMember> = (1..=12).map(member).collect();
    let text = timeout_advice(&crowd);
    assert!(text.contains("  10 S worker 10"), "{text}");
    assert!(!text.contains("  11 S"), "{text}");
    assert!(text.contains("… +2 more"), "{text}");
    assert!(text.contains("x…"), "a long command line is cut: {text}");

    let exactly = |pid: u32| GroupMember {
        pid,
        state: "S".into(),
        command: "y".repeat(SURVIVOR_COMMAND_CHARS),
    };
    let full: Vec<GroupMember> = (1..=MAX_SURVIVORS_LISTED as u32).map(exactly).collect();
    let text = timeout_advice(&full);
    assert!(!text.contains('…'), "a list and a line exactly at their caps are whole: {text}");
}

/// Volume was never the problem — proving the compressor still does its job.
#[tokio::test]
async fn many_lines_still_compress_far_below_the_clamp() {
    if !posix_shell_available() {
        return;
    }
    let tmp_dir = tempfile::tempdir().unwrap();
    let ctx = test_context(tmp_dir.path());
    let result = BashTool::new().run(serde_json::json!({"command": "seq 1 200000"}), &ctx).await;

    let ContentBlock::Text(text) = &result.content[0] else { panic!("expected text") };
    assert!(text.chars().count() < 5_000, "compressed, got {} chars", text.chars().count());
}
