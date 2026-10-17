use super::*;
use std::path::PathBuf;
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

/// Nothing is installed, so a detection test says what the *project* implies and nothing about
/// the machine it runs on.
fn nothing_installed(_: &str) -> bool {
    false
}

/// A virtualenv laid out the way the platform lays one out.
fn write_venv(root: &Path, name: &str, programs: &[&str]) -> PathBuf {
    let bin = root.join(name).join(if cfg!(windows) { "Scripts" } else { "bin" });
    std::fs::create_dir_all(&bin).expect("mkdir");
    for program in programs {
        let path =
            bin.join(if cfg!(windows) { format!("{program}.exe") } else { (*program).to_string() });
        std::fs::write(&path, "#!/bin/sh\nexit 0\n").expect("write");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&path).expect("metadata").permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).expect("chmod");
        }
    }
    root.join(name)
}

#[test]
fn detects_cargo_project() {
    let tmp = tempfile::tempdir().expect("temp dir");
    std::fs::write(tmp.path().join("Cargo.toml"), "[package]").unwrap();
    let detected = detect_command(tmp.path()).expect("a cargo project");
    assert!(detected.command.starts_with("cargo"), "{}", detected.command);
    assert_eq!(detected.framework, Framework::Cargo);
}

#[test]
fn no_markers_no_detection() {
    let tmp = tempfile::tempdir().expect("temp dir");
    assert!(detect_command(tmp.path()).is_err());
}

/// The defect this ladder exists for: a project with its own environment had its tests run by
/// the interpreter on `PATH`, which is a different installation with none of its dependencies.
#[test]
fn a_python_project_is_tested_by_its_own_environment() {
    let project = tempfile::tempdir().expect("temp dir");
    std::fs::write(project.path().join("pyproject.toml"), "[project]").expect("write");
    write_venv(project.path(), ".venv", &["python", "pytest"]);

    // `pytest` is on this PATH as far as detection can tell, and must still lose.
    let detected = detect_with(project.path(), &|_| true).expect("a python project");
    assert_eq!(detected.framework, Framework::Pytest);
    assert!(
        detected.command.starts_with(".venv/") && detected.command.ends_with("-m pytest"),
        "the project's interpreter, not the one on PATH: {}",
        detected.command
    );
}

/// An environment with nothing in it yet: the manager holding the lock file can fix that.
#[test]
fn a_locked_project_runs_through_the_manager_that_owns_its_environment() {
    let project = tempfile::tempdir().expect("temp dir");
    std::fs::write(project.path().join("pyproject.toml"), "[project]").expect("write");
    std::fs::write(project.path().join("uv.lock"), "version = 1").expect("write");

    assert_eq!(
        detect_with(project.path(), &|program| program == "uv").expect("detected").command,
        "uv run pytest"
    );
    // Not installed is not a suggestion: fall through rather than name a program that is absent.
    let without = detect_with(project.path(), &nothing_installed).expect("detected").command;
    assert!(!without.contains("uv"), "{without}");
}

/// A missing pytest is reported by the interpreter that would have imported it, not by a
/// stranger.
#[test]
fn an_environment_without_pytest_still_beats_a_global_one() {
    let project = tempfile::tempdir().expect("temp dir");
    std::fs::write(project.path().join("pyproject.toml"), "[project]").expect("write");
    write_venv(project.path(), ".venv", &["python"]);

    let detected = detect_with(project.path(), &|_| true).expect("a python project");
    assert!(detected.command.starts_with(".venv/"), "{}", detected.command);
}

/// With no environment anywhere, the fallbacks are still ordered by what exists.
#[test]
fn a_bare_python_project_falls_back_through_path() {
    let project = tempfile::tempdir().expect("temp dir");
    std::fs::write(project.path().join("pytest.ini"), "[pytest]").expect("write");
    assert_eq!(python_command(project.path(), &|p| p == "pytest"), "pytest");
    assert_eq!(python_command(project.path(), &|p| p == "python3"), "python3 -m pytest");
    assert_eq!(python_command(project.path(), &nothing_installed), "python -m pytest");
}

/// A lock file is the project saying which manager linked its dependencies.
#[test]
fn a_javascript_project_runs_its_script_through_its_own_package_manager() {
    let project = tempfile::tempdir().expect("temp dir");
    std::fs::write(project.path().join("package.json"), r#"{"scripts": {"test": "vitest run"}}"#)
        .expect("write");
    std::fs::write(project.path().join("pnpm-lock.yaml"), "lockfileVersion: 9").expect("write");

    let detected = detect_with(project.path(), &|_| true).expect("a node project");
    assert_eq!(detected.command, "pnpm test");
    assert_eq!(detected.framework, Framework::Node);
    // A manager the project pins but the machine does not have cannot run anything.
    assert_eq!(
        detect_with(project.path(), &|program| program == "npm").expect("detected").command,
        "npm test"
    );
}

/// `npm test` against a manifest with no test script fails on the manifest, which reads as a
/// broken tool rather than a project that never declared one.
#[test]
fn a_manifest_with_no_test_script_says_so_instead_of_running() {
    let project = tempfile::tempdir().expect("temp dir");
    std::fs::write(project.path().join("package.json"), r#"{"devDependencies": {"vitest": "^2"}}"#)
        .expect("write");

    let refusal = detect_with(project.path(), &|_| true).expect_err("no test script");
    assert!(refusal.contains("no `test` script"), "{refusal}");
    assert!(refusal.contains("npx vitest run"), "and names what to run instead: {refusal}");

    // A manifest nobody can parse is npm's error to report, not ours.
    std::fs::write(project.path().join("package.json"), "{ not json").expect("write");
    assert!(detect_with(project.path(), &|_| true).is_ok());
}

/// The note is for the run the model launched itself, and it must not fire on a healthy failure.
#[test]
fn a_run_outside_the_projects_environment_is_named_as_the_cause() {
    let project = tempfile::tempdir().expect("temp dir");
    let venv = write_venv(project.path(), ".venv", &["python"]);
    let own = venv_pytest(project.path(), &venv).expect("the environment's interpreter");
    let import_error = |success: bool| ShellResult {
        stdout: String::new(),
        stderr: "ModuleNotFoundError: No module named 'pytest_asyncio'".to_string(),
        exit_code: Some(if success { 0 } else { 1 }),
        success,
        detached_writer: false,
    };

    let note = foreign_interpreter_note(
        project.path(),
        "pytest -k hubspot",
        Framework::Pytest,
        &import_error(false),
    )
    .expect("a diagnosis");
    assert!(note.contains(".venv"), "{note}");

    for (command, output, why) in [
        (own.as_str(), import_error(false), "already the project's interpreter"),
        ("uv run pytest", import_error(false), "the manager owns the environment"),
        ("pytest", import_error(true), "a passing run has nothing to diagnose"),
    ] {
        assert_eq!(
            foreign_interpreter_note(project.path(), command, Framework::Pytest, &output),
            None,
            "{why}"
        );
    }
    assert_eq!(
        foreign_interpreter_note(
            project.path(),
            "cargo test",
            Framework::Cargo,
            &import_error(false)
        ),
        None,
        "not a python question"
    );
}

/// The zero-test guard, without needing a runner on the host to produce the lines.
///
/// Its whole job is telling an empty selection from a green one, and a runner reports the first
/// quietly and exits 0 for it — so every case here is one a reader would otherwise skim past.
#[test]
fn an_empty_selection_is_told_from_a_real_run() {
    for empty in [
        "no tests ran in 0.01s",
        "collected 0 items",
        "No test files found",
        "1 deselected in 0.02s",
    ] {
        assert!(executed_nothing(empty, ""), "{empty}");
        assert!(executed_nothing("", empty), "on stderr too: {empty}");
    }
    for real in [
        "19 passed, 1 deselected in 0.30s",
        "1 failed, 41 deselected in 0.40s",
        "test result: ok. 3457 passed; 0 failed",
    ] {
        assert!(!executed_nothing(real, ""), "{real}");
    }
}

#[test]
fn filter_uses_framework_flag() {
    assert_eq!(apply_filter("pytest", Framework::Pytest, Some("foo"), true), "pytest -k foo");
    assert_eq!(
        apply_filter("go test ./...", Framework::Go, Some("TestX"), true),
        "go test ./... -run TestX"
    );
    assert_eq!(apply_filter("cargo test", Framework::Cargo, Some("bar"), true), "cargo test bar");
    assert_eq!(apply_filter("npm test", Framework::Node, Some("baz"), true), "npm test -- baz");
    assert_eq!(apply_filter("cargo test", Framework::Cargo, None, true), "cargo test");
}

/// A filter is one argument, however it is spelled.
///
/// The break this catches: the filter was pasted into the shell line raw, so pytest's ordinary
/// `-k "a or b"` arrived as three words and `x;y` ran `y`.
#[test]
fn a_filter_is_one_argument() {
    let pytest = apply_filter("pytest", Framework::Pytest, Some("a or b"), true);
    assert_eq!(shlex::split(&pytest).unwrap(), ["pytest", "-k", "a or b"]);
    let hostile = apply_filter("cargo test", Framework::Cargo, Some("x; touch pwned"), true);
    assert_eq!(shlex::split(&hostile).unwrap(), ["cargo", "test", "x; touch pwned"]);
    let quoted = apply_filter("pytest", Framework::Pytest, Some("it's"), true);
    assert_eq!(shlex::split(&quoted).unwrap(), ["pytest", "-k", "it's"]);
    let cmd = apply_filter("pytest", Framework::Pytest, Some("a or \"b\""), false);
    assert_eq!(cmd, "pytest -k \"a or \"\"b\"\"\"");
}

/// A filtered pytest run gets its expression as one argument, end to end through the shell.
#[cfg(unix)]
#[tokio::test]
async fn a_spaced_filter_reaches_the_runner_whole() {
    let dir = tempfile::tempdir().unwrap();
    let runner = dir.path().join("pytest");
    std::fs::write(&runner, "#!/bin/sh\nfor a in \"$@\"; do echo \"ARG[$a]\"; done\n").unwrap();
    std::fs::set_permissions(&runner, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let command = format!("{} -q", runner.display());
    let args = json!({ "command": command, "filter": "test_a or test_b" });
    let result = TestTool::new().run(args, &test_context(dir.path())).await;
    let ContentBlock::Text(text) = &result.content[0] else { panic!("expected text") };
    assert!(text.contains("ARG[test_a or test_b]"), "{text}");
}

/// `filter: ""` is a strict-schema model saying "no filter", and must not draw the warning a
/// real filter earns — a real session got it on every full run.
#[tokio::test]
async fn an_empty_filter_is_no_filter() {
    let dir = tempfile::tempdir().expect("temp dir");
    let result = TestTool::new()
        .run(serde_json::json!({"command": "true", "filter": ""}), &test_context(dir.path()))
        .await;
    let ContentBlock::Text(text) = &result.content[0] else { panic!("expected text") };
    assert!(!text.contains("filtered"), "{text}");
    assert!(text.starts_with("$ true\n"), "and the command is run as written: {text}");
}

/// A filtered run that passes is the easiest way to believe work is done when it isn't.
#[tokio::test]
async fn a_filtered_run_says_it_proves_nothing_about_what_it_skipped() {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("test_x.py"), "def test_x():\n    assert True\n")
        .expect("write");
    let result = TestTool::new()
        .run(
            serde_json::json!({"command": "python3 -m pytest -q", "filter": "test_x"}),
            &test_context(dir.path()),
        )
        .await;

    let ContentBlock::Text(text) = &result.content[0] else { panic!("expected text") };
    assert!(text.contains("filtered"), "{text}");
    assert!(text.contains("touches"), "and says what to do about it: {text}");
}

/// Whether this machine can actually *collect and run* a pytest suite.
///
/// Probes with a real run rather than `--version`, because those are different questions: an
/// installed pytest that autoloads a broken third-party plugin answers `--version` happily and
/// then dies during collection, which failed this test on a machine where nothing was wrong with
/// Luxe. Probe the capability you need, not a proxy for it.
async fn pytest_can_run(dir: &Path) -> bool {
    let Some(python) = luxe_core::program::resolve("python3") else {
        return false;
    };
    tokio::process::Command::new(python)
        .args(["-m", "pytest", "-q"])
        .current_dir(dir)
        .kill_on_drop(true)
        .output()
        .await
        .is_ok_and(|output| output.status.success())
}

/// Every runner reports an empty selection quietly, and a zero-test run reads as a pass.
#[tokio::test]
async fn a_run_that_executed_nothing_does_not_read_as_green() {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("test_x.py"), "def test_x():\n    assert True\n")
        .expect("write");
    // The assertion is about how a zero-test run is *reported*, which needs a runner that can
    // report one. A missing or broken pytest is a different message entirely.
    if !pytest_can_run(dir.path()).await {
        return;
    }
    let result = TestTool::new()
        .run(
            serde_json::json!({
                "command": "python3 -m pytest -q", "filter": "no_such_test_zzz"
            }),
            &test_context(dir.path()),
        )
        .await;

    let ContentBlock::Text(text) = &result.content[0] else { panic!("expected text") };
    assert!(text.contains("No tests actually ran"), "{text}");
    assert!(text.contains("not a green suite"), "{text}");
}

/// A cancelled test run stops, and so does everything it started.
///
/// The break this catches: the suite ran under `wait_with_output` with no tree guard, so a
/// dropped call reaped only the shell and left `cargo test` running, holding its locks.
#[cfg(unix)]
#[tokio::test]
async fn a_dropped_test_run_leaves_no_child() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("alive");
    let script =
        format!("(for _ in $(seq 1 40); do touch {}; sleep 0.25; done) & wait", marker.display());
    let cx = test_context(dir.path());
    {
        let tool = TestTool::new();
        let run = tool.run(json!({ "command": script }), &cx);
        tokio::pin!(run);
        // far enough to spawn the tree, then dropped, exactly as a cancel does
        let _ = tokio::time::timeout(Duration::from_millis(900), &mut run).await;
    }
    let _ = std::fs::remove_file(&marker);
    tokio::time::sleep(Duration::from_millis(1_200)).await;
    assert!(!marker.exists(), "a cancelled test run left its grandchild running");
}

/// A suite that leaves a process holding its output says so, as `bash` does, rather than
/// dropping what came after the shell exited without a word.
#[cfg(unix)]
#[tokio::test]
async fn a_run_that_left_a_writer_behind_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let args = json!({ "command": "echo ok; (sleep 3; echo late) &" });
    let result = TestTool::new().run(args, &test_context(dir.path())).await;
    let ContentBlock::Text(text) = &result.content[0] else { panic!("expected text") };
    assert!(text.contains("still held its output"), "{text}");
}
