//! Test environment — isolation for the two things config resolution reads that a process shares.
//!
//! One owner, because more than one module's tests need it and two locks do not serialize against
//! each other — which is the failure mode this exists to prevent.

/// Env + real config are process-global; serialize the tests that touch them. `cargo test`
/// runs tests as threads in one process, so a test that skips this lock does not only risk
/// itself — it corrupts the environment under whichever locked test is mid-flight. Recover
/// from poisoning rather than `unwrap`, or one genuine failure cascades into a screenful of
/// fictional ones.
pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Point config discovery at an empty temp dir and clear the provider/model env so
/// resolution is deterministic and host-independent.
pub(crate) fn isolated_env() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("a temp dir");
    std::env::set_var("XDG_CONFIG_HOME", dir.path());
    // The *state* dir too: a remembered model outranks `default_model`, so a developer who has
    // ever used `/model` would have their own choice resolved here instead of the fixture's,
    // and the suite would pass or fail depending on whose machine ran it.
    std::env::set_var("XDG_STATE_HOME", dir.path().join("state"));
    for var in [
        "LUXE_PROVIDER",
        "LUXE_MODEL",
        "LUXE_API_KEY",
        "LUXE_BASE_URL",
        "LUXE_CONFIG_DIR",
        "LUXE_STATE_DIR",
        "OPENAI_API_KEY",
        "OPENAI_BASE_URL",
        "ANTHROPIC_API_KEY",
    ] {
        std::env::remove_var(var);
    }
    dir
}
