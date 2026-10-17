//! A refused flag combination ends the process the way a refused flag does: one line, exit 1.

use std::process::Command;

fn run(args: &[&str]) -> (Option<i32>, String) {
    let (code, stderr, home) = run_in_a_home(args);
    drop(home);
    (code, stderr)
}

/// [`run`], keeping the directory it ran in so a test can look at what it left.
fn run_in_a_home(args: &[&str]) -> (Option<i32>, String, tempfile::TempDir) {
    let home = tempfile::tempdir().expect("a home");
    let out = Command::new(env!("CARGO_BIN_EXE_luxe"))
        .args(args)
        .current_dir(home.path())
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join("config"))
        .env("XDG_STATE_HOME", home.path().join("state"))
        .output()
        .expect("the binary runs");
    (out.status.code(), String::from_utf8_lossy(&out.stderr).into_owned(), home)
}

/// The refusal comes back through `main`, so it is a sentence after `luxe:` and not `Error:` with a
/// debug dump, and nothing ran before it.
#[test]
fn a_refused_combination_is_one_line_and_exit_one() {
    for (args, says) in [
        (&["--on-ask", "deny"][..], "luxe: --on-ask settles what an `ask` becomes"),
        (&["--github-writes"][..], "luxe: --github-writes lets an --rpc client publish"),
        (&["--headless"][..], "luxe: --headless requires --prompt <text>"),
    ] {
        let (code, stderr) = run(args);
        assert_eq!(code, Some(1), "{args:?}: {stderr}");
        assert!(stderr.starts_with(says), "{args:?}: {stderr}");
        assert!(!stderr.contains("Error:"), "{args:?}: {stderr}");
    }
}

/// A refusal comes before anything is created: a headless run with no prompt used to get as far as
/// opening its log in `.luxe/` in the directory it was started in.
#[test]
fn a_refusal_leaves_nothing_behind() {
    let (code, _, home) = run_in_a_home(&["--headless"]);
    assert_eq!(code, Some(1));
    assert!(!home.path().join(".luxe").exists(), "a refused run made a .luxe/");
}
