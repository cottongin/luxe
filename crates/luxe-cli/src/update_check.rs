//! Update check — the single outbound request behind the dashboard's update notice.
//!
//! The policy, the version comparison and the install-provenance detection all live in
//! [`luxe_core::update`], which has no network in it. This module is only the request and the
//! scheduling around it: when to ask, how to ask without saying anything about the user, and how
//! to fail quietly.
//!
//! Shaped after [`crate::model_catalog::spawn_refresh`], because it answers the same question —
//! *is my cached copy of a remote fact old enough to re-ask?* — and two answers would drift.
//!
//! Three rules hold the whole feature together:
//!
//! - **The notice shown this run comes from the cache, never from a live request.** The check runs
//!   in the background and lands in time for the *next* launch. Nothing waits on the network to
//!   draw a dashboard.
//! - **The request carries nothing.** No version, no platform, no identifier. Even the user agent
//!   is a bare `luxe` — deliberately unlike the models.dev fetch, which sends its version.
//! - **Every failure is silent.** Offline, rate-limited, a 404 before the repository is public: all
//!   of them mean "no notice", which is exactly what a user who did not ask for this wants.

use std::path::PathBuf;
use std::time::Duration;

use luxe_core::update::{self, InstallSource, UpdateNotice, UpdateState};

/// How long one check may take, start to finish. Short: nothing is waiting on it, and a request
/// that has not landed in ten seconds may as well land next launch.
const TIMEOUT: Duration = Duration::from_secs(10);

/// Hard ceiling on the response body. A release manifest is a few kilobytes; this is remote,
/// untrusted JSON, so a server that streams forever gets cut off rather than exhausting memory.
const MAX_BODY_BYTES: usize = 1024 * 1024;

/// Where the cached answer lives, with `LUXE_UPDATE_PATH` as a test seam.
fn state_path() -> PathBuf {
    match std::env::var("LUXE_UPDATE_PATH") {
        Ok(path) if !path.trim().is_empty() => PathBuf::from(path),
        _ => update::state_path(),
    }
}

/// Whether the user has turned the check off, by config or environment.
///
/// The environment wins because it is the one a script or a CI job can set without editing a file
/// that belongs to a human.
fn disabled(repo_root: &std::path::Path) -> bool {
    match std::env::var("LUXE_UPDATE_CHECK").ok().map(|v| v.trim().to_ascii_lowercase()) {
        Some(value) if matches!(value.as_str(), "0" | "false" | "no" | "off") => return true,
        Some(value) if matches!(value.as_str(), "1" | "true" | "yes" | "on") => return false,
        _ => {}
    }
    !luxe_core::config::Config::load(repo_root).update.enabled()
}

/// The notice to draw, or `None`. Reads the cache only — never the network.
pub(crate) fn cached_notice(repo_root: &std::path::Path) -> Option<UpdateNotice> {
    if disabled(repo_root) {
        return None;
    }
    let state = UpdateState::load(&state_path());
    update::notice(&state, luxe_core::version::VERSION, InstallSource::detect())
}

/// Refresh the cached answer in the background, if it is stale and the user allows it.
///
/// Returns immediately. A failure is logged at `debug` rather than `warn`: unlike a models.dev
/// refresh, a missed update check costs the user nothing they asked for, and warning about it on
/// every offline launch would train people to ignore warnings.
pub(crate) fn spawn_check(repo_root: &std::path::Path) {
    if disabled(repo_root) {
        return;
    }
    let path = state_path();
    if !UpdateState::load(&path).is_stale(update::CHECK_TTL) {
        return;
    }
    let url = manifest_url();

    tokio::spawn(async move {
        // The timestamp moves whether or not the fetch worked, so an unreachable endpoint is
        // retried once a day rather than on every launch.
        let latest = fetch_latest(&url).await.map_err(|error| {
            tracing::debug!(%error, "update check failed; no notice this run");
        });
        let previous = UpdateState::load(&path);
        let state = UpdateState { latest: latest.ok().or(previous.latest), checked_at: now_secs() };
        if let Err(error) = state.store(&path) {
            tracing::debug!(%error, path = %path.display(), "could not cache the update check");
        }
    });
}

/// Asks the manifest now, whatever the cache says, and records the answer: the reader asked, as
/// `luxe update` does, so neither the daily cadence nor the opt-out applies. The request is the
/// same anonymous one the daily check makes.
pub(crate) async fn check_now() -> Result<String, String> {
    let latest = fetch_latest(&manifest_url()).await?;
    let path = state_path();
    let state = UpdateState { latest: Some(latest.clone()), checked_at: now_secs() };
    if let Err(error) = state.store(&path) {
        tracing::debug!(%error, path = %path.display(), "could not cache the update check");
    }
    Ok(latest)
}

/// The release manifest, with `LUXE_UPDATE_URL` as a test seam.
fn manifest_url() -> String {
    std::env::var("LUXE_UPDATE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| update::RELEASE_MANIFEST_URL.to_string())
}

/// Seconds since the epoch, or 0 if the clock is unreadable (which reads as "never checked").
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or(0)
}

/// Ask the manifest for the newest released version.
///
/// Accepts either a GitHub release object (`tag_name`) or a hand-rolled manifest (`version`), so
/// the endpoint can move off the GitHub API later without a new code path.
async fn fetch_latest(url: &str) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(TIMEOUT)
        // Bare `luxe`, with no version. The GitHub API rejects a request with no user agent at
        // all, so this is the least that can be said; anything more would make the request a
        // report about this machine rather than a question about the release.
        .user_agent("luxe")
        .build()
        .map_err(|error| error.to_string())?;

    let response = client.get(url).send().await.map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!("release manifest returned {}", response.status()));
    }
    let body = bounded_body(response).await?;

    let parsed: serde_json::Value =
        serde_json::from_str(&body).map_err(|error| error.to_string())?;
    let raw = parsed
        .get("tag_name")
        .or_else(|| parsed.get("version"))
        .and_then(|value| value.as_str())
        .ok_or_else(|| "release manifest has no tag_name or version".to_string())?;

    // Parse before caching, so a manifest full of nonsense cannot make every later launch compare
    // against something that will never be a version.
    let normalized = update::normalize_version(raw);
    if semver::Version::parse(normalized).is_err() {
        return Err(format!("release manifest version is not semver: {raw}"));
    }
    Ok(normalized.to_string())
}

/// Read a response body, refusing one that outgrows [`MAX_BODY_BYTES`].
async fn bounded_body(response: reqwest::Response) -> Result<String, String> {
    use futures::StreamExt;

    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| error.to_string())?;
        if body.len() + chunk.len() > MAX_BODY_BYTES {
            return Err("release manifest is implausibly large".to_string());
        }
        body.extend_from_slice(&chunk);
    }
    String::from_utf8(body).map_err(|_| "release manifest was not UTF-8".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serve one canned HTTP response on a loopback port, then stop.
    ///
    /// A real socket rather than a mocked client, because the things worth catching here — a
    /// non-success status, a body that is not the shape we expect, a version that is not semver —
    /// all live in the seam between reqwest and our parsing.
    async fn serve_once(body: &'static str, status: &'static str) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("a loopback port");
        let port = listener.local_addr().expect("addr").port();
        tokio::spawn(async move {
            let Ok((mut socket, _)) = listener.accept().await else { return };
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut scratch = [0u8; 1024];
            let _ = socket.read(&mut scratch).await;
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: \
                 application/json\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.shutdown().await;
        });
        format!("http://127.0.0.1:{port}/latest")
    }

    #[tokio::test]
    async fn a_github_release_object_yields_its_tag() {
        let url = serve_once(r#"{"tag_name":"v0.9.9","name":"0.9.9"}"#, "200 OK").await;
        assert_eq!(fetch_latest(&url).await.as_deref(), Ok("0.9.9"));
    }

    #[tokio::test]
    async fn a_plain_manifest_yields_its_version() {
        let url = serve_once(r#"{"version":"1.2.3"}"#, "200 OK").await;
        assert_eq!(fetch_latest(&url).await.as_deref(), Ok("1.2.3"));
    }

    /// Before the repository is public this endpoint 404s, which must be an ordinary quiet failure
    /// rather than anything a user sees.
    #[tokio::test]
    async fn a_missing_manifest_is_an_error_not_a_version() {
        let url = serve_once(r#"{"message":"Not Found"}"#, "404 Not Found").await;
        assert!(fetch_latest(&url).await.is_err());
    }

    /// Nonsense is rejected at fetch time, so it can never be cached and compared against on every
    /// later launch.
    #[tokio::test]
    async fn a_manifest_whose_version_is_not_semver_is_refused() {
        let url = serve_once(r#"{"tag_name":"nightly"}"#, "200 OK").await;
        assert!(fetch_latest(&url).await.is_err());
        let url = serve_once(r#"{"released":true}"#, "200 OK").await;
        assert!(fetch_latest(&url).await.is_err());
    }

    /// The whole path, as the dashboard walks it: fetch, cache, then read a notice back out.
    #[tokio::test]
    async fn a_fetched_version_reaches_the_notice_through_the_cache() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cache = dir.path().join("update.json");
        let url = serve_once(r#"{"tag_name":"v99.0.0"}"#, "200 OK").await;

        let latest = fetch_latest(&url).await.expect("a version");
        UpdateState { latest: Some(latest), checked_at: now_secs() }.store(&cache).expect("store");

        std::env::set_var("LUXE_UPDATE_PATH", &cache);
        std::env::set_var("LUXE_UPDATE_CHECK", "1");
        let notice = cached_notice(dir.path()).expect("99.0.0 is ahead of anything we ship");
        assert!(notice.line().contains("99.0.0"), "{}", notice.line());
        std::env::remove_var("LUXE_UPDATE_PATH");
        std::env::remove_var("LUXE_UPDATE_CHECK");
    }

    /// `luxe update` asks whatever the cadence and the opt-out say, since the reader asked, and
    /// what it learns is cached, so the start screen agrees with it.
    #[tokio::test]
    async fn an_asked_for_check_ignores_the_opt_out_and_caches_the_answer() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cache = dir.path().join("update.json");
        std::env::set_var("LUXE_UPDATE_PATH", &cache);
        std::env::set_var("LUXE_UPDATE_CHECK", "0");
        std::env::set_var(
            "LUXE_UPDATE_URL",
            serve_once(r#"{"tag_name":"v99.0.0"}"#, "200 OK").await,
        );
        assert_eq!(check_now().await.as_deref(), Ok("99.0.0"));
        assert_eq!(UpdateState::load(&cache).latest.as_deref(), Some("99.0.0"));
        for name in ["LUXE_UPDATE_PATH", "LUXE_UPDATE_CHECK", "LUXE_UPDATE_URL"] {
            std::env::remove_var(name);
        }
    }

    /// Both manifest shapes resolve, so moving off the GitHub API later is a URL change rather
    /// than a code change.
    #[test]
    fn either_manifest_shape_yields_a_version() {
        for (body, expected) in [
            (r#"{"tag_name": "v0.4.0", "name": "0.4.0"}"#, "0.4.0"),
            (r#"{"version": "0.4.0"}"#, "0.4.0"),
        ] {
            let parsed: serde_json::Value = serde_json::from_str(body).expect("valid json");
            let raw = parsed
                .get("tag_name")
                .or_else(|| parsed.get("version"))
                .and_then(|value| value.as_str())
                .expect("a version field");
            assert_eq!(update::normalize_version(raw), expected);
        }
    }

    /// `LUXE_UPDATE_CHECK=0` must win over a config that says yes, because it is the switch a
    /// script or a CI job can reach.
    ///
    /// Mutates the environment, which nextest makes safe by giving each test its own process.
    #[test]
    fn the_environment_can_turn_the_check_off() {
        let project = tempfile::tempdir().expect("tempdir");
        std::env::set_var("LUXE_UPDATE_CHECK", "0");
        assert!(disabled(project.path()));
        std::env::set_var("LUXE_UPDATE_CHECK", "1");
        assert!(!disabled(project.path()));
        std::env::remove_var("LUXE_UPDATE_CHECK");
    }

    /// A disabled check draws nothing even with a cache full of newer versions sitting right
    /// there. Turning it off has to mean off, not "off until something interesting happens".
    #[test]
    fn a_disabled_check_shows_no_notice_even_with_a_populated_cache() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cache = dir.path().join("update.json");
        UpdateState { latest: Some("99.0.0".into()), checked_at: now_secs() }
            .store(&cache)
            .expect("store");
        std::env::set_var("LUXE_UPDATE_PATH", &cache);

        std::env::set_var("LUXE_UPDATE_CHECK", "1");
        assert!(cached_notice(dir.path()).is_some(), "the cache really does hold a newer version");
        std::env::set_var("LUXE_UPDATE_CHECK", "0");
        assert!(cached_notice(dir.path()).is_none(), "off means off");

        std::env::remove_var("LUXE_UPDATE_CHECK");
        std::env::remove_var("LUXE_UPDATE_PATH");
    }
}
