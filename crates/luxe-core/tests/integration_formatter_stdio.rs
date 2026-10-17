//! Format-on-save must never write to the terminal the TUI is drawing on.
//!
//! The regression this guards: the availability probe used to run `<formatter> --version` with
//! inherited stdio, so `rustfmt 1.9.0` (and, mid-escape-sequence, fragments of the frame's own
//! SGR codes) appeared inside the alternate screen — in the sidebar, and in the prompt box.
//!
//! Asserting "nothing was written to fd 1" only works from outside the process, so the test
//! re-executes its own binary as a child with piped stdio and inspects what the child emitted.

// The stub formatter is an executable shell script and the test re-execs itself, so the fixture is
// unix-only even though `run_formatter` is not. Gated at the file level so a Windows build has no
// half-used helpers left over.
#![cfg(unix)]

use luxe_core::formatter::{run_formatter, FormatterRegistry, FormatterSpec};
use std::path::Path;

/// Env var that switches the test binary into its child role.
const CHILD_MARKER: &str = "LUXE_FORMATTER_STDIO_CHILD";

/// Writes an executable formatter stub that is as loud as a real tool can be.
fn write_noisy_formatter(dir: &Path, name: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(
        &path,
        "#!/bin/sh\n\
         echo 'noisy-fmt 1.9.0 (deadbeef 2026-07-28)'\n\
         echo 'warning: something on stderr' >&2\n\
         printf '\\033[48;2;65;74;76m'\n\
         exit 0\n",
    )
    .expect("write formatter stub");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("chmod formatter stub");
    }
    path
}

/// The child role: run every formatter path against a deliberately loud stub.
///
/// Any byte the stub leaks reaches the real stdout/stderr of this process, which the parent reads.
async fn run_child() {
    let dir = tempfile::tempdir().expect("temp dir");
    let in_place = write_noisy_formatter(dir.path(), "noisy-fmt");
    let via_stdin = write_noisy_formatter(dir.path(), "noisy-filter");

    let mut registry = FormatterRegistry::default();
    registry.register(
        "noisy",
        FormatterSpec {
            command: in_place.display().to_string(),
            args: Vec::new(),
            via_stdin: false,
        },
    );
    registry.register(
        "quiet",
        FormatterSpec {
            command: via_stdin.display().to_string(),
            args: Vec::new(),
            via_stdin: true,
        },
    );

    let target = dir.path().join("sample.noisy");
    std::fs::write(&target, "unformatted\n").expect("write sample");
    let _ = run_formatter(&target, &registry).await;

    let filtered = dir.path().join("sample.quiet");
    std::fs::write(&filtered, "unformatted\n").expect("write sample");
    let _ = run_formatter(&filtered, &registry).await;
}

#[test]
#[cfg(unix)]
fn a_formatter_never_writes_to_the_parent_terminal() {
    if std::env::var(CHILD_MARKER).is_ok() {
        // Re-entered as the child: do the work, then exit before the harness prints a summary.
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("child runtime")
            .block_on(run_child());
        std::process::exit(0);
    }

    let output = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .args(["--exact", "a_formatter_never_writes_to_the_parent_terminal", "--nocapture"])
        .env(CHILD_MARKER, "1")
        .output()
        .expect("re-run the test binary as a child");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.contains("noisy-fmt 1.9.0"),
        "a formatter's stdout leaked onto the parent's terminal:\n{stdout}"
    );
    assert!(
        !stdout.contains('\u{1b}'),
        "a formatter leaked an escape sequence onto the parent's terminal:\n{stdout:?}"
    );
    assert!(
        !stderr.contains("something on stderr"),
        "a formatter's stderr leaked onto the parent's terminal:\n{stderr}"
    );
}
