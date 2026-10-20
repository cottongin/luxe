use super::*;
use std::io::Write;

/// luxe's own crates are visible at `info`, and dependencies are not.
///
/// The break this catches is the one that shipped: with a bare `warn` default, nothing luxe
/// logged at `info` was ever seen, so routine loop behavior drifted up to `warn!` just to be
/// visible — an ordinary steer arrived labeled as a warning. Widening it to `info` for
/// everything instead would swap that for a firehose, because `hyper` and `rustls` narrate every
/// socket at `debug` and `reqwest` is not far behind.
///
/// Asserted by running events through the filter rather than by reading its directives back: the
/// question is what a subscriber lets through, and `to_string()` only repeats what we typed.
#[test]
fn the_default_filter_shows_luxe_at_info_and_nothing_else() {
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::Layer;

    /// Records the target of every event that survives the filter.
    struct Capture(Arc<Mutex<Vec<String>>>);

    impl<S: tracing::Subscriber> Layer<S> for Capture {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            if let Ok(mut seen) = self.0.lock() {
                seen.push(format!("{}@{}", event.metadata().target(), event.metadata().level()));
            }
        }
    }

    let seen = Arc::new(Mutex::new(Vec::new()));
    let subscriber =
        tracing_subscriber::registry().with(Capture(seen.clone()).with_filter(super::env_filter()));

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(target: "luxe_core", "routine luxe behavior");
        tracing::debug!(target: "luxe_core", "luxe detail");
        tracing::info!(target: "hyper", "a socket happened");
        tracing::warn!(target: "hyper", "a socket misbehaved");
    });

    let seen = seen.lock().expect("not poisoned").clone();
    assert!(
        seen.contains(&"luxe_core@INFO".to_string()),
        "luxe's own routine logging must be visible: {seen:?}"
    );
    assert!(!seen.contains(&"luxe_core@DEBUG".to_string()), "debug stays opt-in: {seen:?}");
    assert!(
        !seen.contains(&"hyper@INFO".to_string()),
        "a dependency's routine logging is noise: {seen:?}"
    );
    assert!(
        seen.contains(&"hyper@WARN".to_string()),
        "but a dependency's warning is worth keeping: {seen:?}"
    );
}

/// An explicit `LUXE_LOG` overrides the default wholesale.
#[test]
fn luxe_log_overrides_the_default() {
    // Safe here because nextest runs each test in its own process.
    std::env::set_var("LUXE_LOG", "luxe_core=debug");
    let rendered = super::env_filter().to_string();
    std::env::remove_var("LUXE_LOG");
    assert!(rendered.contains("luxe_core=debug"), "{rendered}");
    assert!(!rendered.contains("hyper"), "an override replaces the default: {rendered}");
}

#[test]
fn redacting_writer_scrubs_secret_shaped_tokens() {
    let tmp_guard = tempfile::tempdir().expect("temp dir");
    let tmp = tmp_guard.path().to_path_buf();
    std::fs::create_dir_all(&tmp).unwrap();
    let path = tmp.join("luxe.log");
    let file = OpenOptions::new().create(true).append(true).open(&path).unwrap();
    let mut writer = RedactingFile(Arc::new(Mutex::new(file)));

    writer.write_all(b"connecting with key sk-abcdefghijklmnopqrstuvwxyz0123\n").unwrap();
    writer.flush().unwrap();

    // and a key the process resolved, whatever its shape
    luxe_core::redact::process_secrets().add("log-shapeless-key-5150");
    writer.write_all(b"resolved log-shapeless-key-5150 for openai\n").unwrap();
    writer.flush().unwrap();

    let contents = std::fs::read_to_string(&path).unwrap();
    assert!(!contents.contains("sk-abcdefghijklmnopqrstuvwxyz0123"), "raw key leaked");
    assert!(contents.contains("[REDACTED_API_KEY]"));
    assert!(!contents.contains("log-shapeless-key-5150"), "a resolved key leaked: {contents}");
}

/// A reporting command leaves no `.luxe/` behind, and its log still lands somewhere.
///
/// The defect: `luxe auth list` in `/tmp` made `/tmp/.luxe`, and every repository under `/tmp`
/// then rooted itself there.
#[test]
fn only_a_project_run_writes_into_the_project() {
    let _env = crate::engine_setup::test_env::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let scratch = tempfile::tempdir().expect("temp tree");
    let state = tempfile::tempdir().expect("temp state");
    std::env::set_var("XDG_STATE_HOME", state.path());
    std::env::remove_var("LUXE_STATE_DIR");

    let user = LogHome::User.prepare().expect("a user log directory");
    assert!(user.starts_with(state.path()), "{user:?}");
    // the state override moves them with the rest of the state
    let moved = state.path().join("moved");
    std::env::set_var("LUXE_STATE_DIR", &moved);
    let user = LogHome::User.prepare();
    std::env::remove_var("LUXE_STATE_DIR");
    assert_eq!(user, Some(moved.join("logs")));
    assert!(!scratch.path().join(".luxe").exists(), "a reporting command re-roots nothing");

    let project =
        LogHome::Project(scratch.path().to_path_buf()).prepare().expect("a project log directory");
    assert_eq!(project, scratch.path().join(".luxe").join("logs"));
    assert!(scratch.path().join(".luxe").join(".gitignore").exists(), "and it ignores itself");

    std::env::remove_var("XDG_STATE_HOME");
}

#[test]
fn self_ignore_hides_the_whole_directory_and_never_clobbers() {
    let dir_guard = tempfile::tempdir().expect("temp dir");
    let dir = dir_guard.path().to_path_buf();
    std::fs::create_dir_all(&dir).unwrap();
    let ignore = dir.join(".gitignore");

    ensure_self_ignored(&dir);
    let first = std::fs::read_to_string(&ignore).unwrap();
    assert!(first.lines().any(|l| l == "*"), "should ignore everything by default");
    assert!(first.lines().any(|l| l == "!permissions.json"), "shareable config carved out");
    assert!(
        !first.lines().any(|l| l == "!.gitignore"),
        "the rules ignore themselves: one un-ignored file is all Git needs to report the \
             whole directory as untracked, in every status and every review"
    );

    // An older luxe-authored file is upgraded in place.
    std::fs::write(&ignore, "# Created automatically by luxe — do not delete.\n*\n!.gitignore\n")
        .unwrap();
    ensure_self_ignored(&dir);
    assert!(!std::fs::read_to_string(&ignore).unwrap().contains("!.gitignore"));

    // A user-curated file must survive a later run untouched.
    std::fs::write(&ignore, "*\n!custom.json\n").unwrap();
    ensure_self_ignored(&dir);
    assert_eq!(std::fs::read_to_string(&ignore).unwrap(), "*\n!custom.json\n");
}

/// A log holds what the agent read and ran, so a new one is readable by its owner only, and one an
/// older luxe left at the umask's default is tightened when it is next opened.
#[cfg(unix)]
#[test]
fn a_log_file_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let mode =
        |path: &std::path::Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    open_log(dir.path(), ROTATE_AT_BYTES).unwrap();
    assert_eq!(mode(&dir.path().join("luxe.log")), 0o600);

    let old = tempfile::tempdir().unwrap();
    std::fs::write(old.path().join("luxe.log"), "earlier\n").unwrap();
    std::fs::set_permissions(old.path().join("luxe.log"), std::fs::Permissions::from_mode(0o644))
        .unwrap();
    open_log(old.path(), ROTATE_AT_BYTES).unwrap();
    assert_eq!(mode(&old.path().join("luxe.log")), 0o600);
}

/// A log past the threshold is moved aside when opened, so a long-lived project's log stops
/// growing; the previous one is kept, the one before it is not.
#[test]
fn a_log_past_the_threshold_rotates_on_open() {
    use std::io::Write as _;
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("luxe.log");
    std::fs::write(&log, "0123456789").unwrap();
    open_log(dir.path(), 100).unwrap();
    assert_eq!(std::fs::read_to_string(&log).unwrap(), "0123456789", "under the threshold");

    std::fs::write(dir.path().join("luxe.log.1"), "the one before").unwrap();
    let (mut file, failed) = open_log(dir.path(), 5).unwrap();
    assert!(failed.is_none(), "{failed:?}");
    file.write_all(b"fresh").unwrap();
    assert_eq!(std::fs::read_to_string(&log).unwrap(), "fresh");
    assert_eq!(std::fs::read_to_string(dir.path().join("luxe.log.1")).unwrap(), "0123456789");
}
