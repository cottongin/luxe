//! Headless-browser capture for `mode=url` — renders a page (e.g. a local dev server) to PNG.
//!
//! Shells out to a headless Chrome/Chromium/Edge rather than embedding a CDP client, matching the
//! native-tools philosophy of the rest of the screenshot backend.
//!
//! Model-supplied URLs are untrusted, so `mode=url` reuses the same [`crate::ssrf`] guard as
//! `web_fetch`: private-network and cloud-metadata hosts are refused, `file://` is refused, and
//! Chrome's own DNS is pinned to the address we validated so a rebind can't slip a different one past
//! the check. **Loopback is allowed**, because capturing a dev server on this machine is the reason
//! URL mode exists — see [`crate::ssrf`] for why that costs nothing next to an unrestricted `bash`.
//! `LUXE_WEB_ALLOW_PRIVATE=1` remains the opt-out for a LAN host or a `file://` capture.

use anyhow::{anyhow, bail, Context, Result};
use std::path::Path;
use tokio::process::Command;

use super::capture::which;

/// Renders `url` at `width`x`height`, waiting `wait_ms` for it to settle, into the PNG at `out`.
///
/// Chrome runs on a throwaway profile under `<cwd>/.luxe/browser/`, removed afterwards, so a page
/// never sees the user's cookies or a previous capture's.
pub(super) async fn capture_url(
    url: &str,
    size: (u32, u32),
    wait_ms: u32,
    out: &Path,
    cwd: &Path,
) -> Result<()> {
    let resolver_rule = validate_capture_url(url).await?;
    let browser = find_browser()
        .ok_or_else(|| anyhow!("no headless browser found (install Chrome, Chromium, or Edge)"))?;
    let out = out.to_str().ok_or_else(|| anyhow!("output path is not valid UTF-8"))?;
    let profile = Profile::create(cwd)?;
    let capture = ChromeCapture {
        url,
        width: size.0,
        height: size.1,
        wait_ms,
        out,
        resolver_rule,
        sandboxed: !sandbox_unavailable(
            luxe_core::isolation::running_as_root(),
            luxe_core::isolation::in_container(),
        ),
        profile: &profile.0,
    };
    // `kill_on_drop` is what makes the engine's deadline and the user's interrupt actually reach
    // Chrome. Dropping this future is how a stopped tool call unwinds, and a browser aimed at a host
    // that accepts the connection and then answers nothing will otherwise sit there for the rest of
    // the session: `--virtual-time-budget` bounds the page's clock, not its network.
    let output = Command::new(&browser)
        .args(chrome_args(&capture))
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .with_context(|| format!("failed to run headless browser `{browser}`"))?;
    // Chrome exits 0 on a successful screenshot; some builds print to stderr even on success, so
    // fall back to checking the file was actually written.
    if !output.status.success() && !out_written(out) {
        bail!("headless `{browser}` failed: {}", String::from_utf8_lossy(&output.stderr).trim());
    }
    if !out_written(out) {
        bail!("headless `{browser}` produced no screenshot for {url}");
    }
    Ok(())
}

/// Everything Chrome is told for one capture, apart so the flags are testable without a browser.
struct ChromeCapture<'a> {
    url: &'a str,
    width: u32,
    height: u32,
    wait_ms: u32,
    out: &'a str,
    resolver_rule: Option<String>,
    sandboxed: bool,
    profile: &'a Path,
}

fn chrome_args(capture: &ChromeCapture<'_>) -> Vec<String> {
    let mut args = vec![
        "--headless=new".to_string(),
        "--disable-gpu".to_string(),
        "--hide-scrollbars".to_string(),
        "--force-device-scale-factor=1".to_string(),
        format!("--user-data-dir={}", capture.profile.display()),
        format!("--window-size={},{}", capture.width, capture.height),
        format!("--virtual-time-budget={}", capture.wait_ms),
        format!("--screenshot={}", capture.out),
    ];
    // a container workaround, not a default: the sandbox is what stands between a hostile page and
    // the user's desktop
    if !capture.sandboxed {
        args.push("--no-sandbox".to_string());
    }
    if let Some(rule) = &capture.resolver_rule {
        args.push(format!("--host-resolver-rules={rule}"));
    }
    args.push(capture.url.to_string());
    args
}

/// A fresh browser profile directory, removed when the capture is done.
struct Profile(std::path::PathBuf);

impl Profile {
    fn create(cwd: &Path) -> Result<Self> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        let next = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let name = format!("profile-{}-{stamp}-{next}", std::process::id());
        let dir = cwd.join(".luxe").join("browser").join(name);
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("could not create {}", dir.display()))?;
        Ok(Self(dir))
    }
}

impl Drop for Profile {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.0) {
            tracing::warn!(%error, profile = %self.0.display(), "could not remove a browser profile");
        }
    }
}

/// Whether Chrome's sandbox cannot start here: as root, or inside a container.
fn sandbox_unavailable(root: bool, in_container: bool) -> bool {
    root || in_container
}

fn out_written(out: &str) -> bool {
    std::fs::metadata(out).map(|meta| meta.len() > 0).unwrap_or(false)
}

/// Validates a `mode=url` target against the SSRF guard and returns the Chrome
/// `--host-resolver-rules` value ([`resolver_rule`]) that keeps the capture on that host.
///
/// Public and loopback origins pass; private-network and metadata hosts are refused, as is `file://`.
/// When the guard is disabled wholesale we return `None` (no pinning) so an intentional LAN or file
/// capture still works.
async fn validate_capture_url(url: &str) -> Result<Option<String>> {
    if url.starts_with("file://") {
        if crate::ssrf::guard_disabled() {
            return Ok(None);
        }
        // Not a network fetch at all: this would hand a model-supplied path to a browser with the
        // whole filesystem in reach, which `read` already does properly, scoped to the project.
        bail!(
            "file:// captures are blocked: a browser given a local path can read anything this user \
             can. Use `read` for file contents, or serve the file over http:// and capture that. \
             LUXE_WEB_ALLOW_PRIVATE=1 overrides this."
        );
    }
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        bail!("url must start with http:// or https:// (got '{url}')");
    }
    if crate::ssrf::guard_disabled() {
        return Ok(None);
    }
    let parsed = url::Url::parse(url).map_err(|e| anyhow!("invalid URL: {e}"))?;
    // Loopback allowed: a local dev server is what this mode is for.
    let addrs = crate::ssrf::resolve_validated(&parsed, crate::ssrf::LocalAccess::Allowed)
        .await
        .map_err(|e| anyhow!("{e}"))?;
    Ok(resolver_rule(&parsed, &addrs))
}

/// The Chrome resolver rule for a capture of `url`: only its host resolves, so a page cannot
/// redirect to or embed another one. The rule covers IP-literal hosts too: checked by hand against
/// Chromium, an image at `http://127.0.0.1:<port>/` on a `localhost` page did not load under
/// `MAP * ~NOTFOUND , EXCLUDE localhost`, and did without it.
///
/// A public name is pinned to the address that was checked, closing the rebinding gap. A loopback
/// name is not pinned: Chrome's MAP takes one address, and choosing between `::1` and `127.0.0.1`
/// on the user's behalf is how a dev server listening on the other one becomes unreachable.
fn resolver_rule(url: &url::Url, addrs: &[std::net::SocketAddr]) -> Option<String> {
    let host = match url.host()? {
        url::Host::Domain(name) if !addrs.iter().all(|addr| addr.ip().is_loopback()) => {
            let addr = addrs.first()?;
            // bracketed, so an IPv6 replacement can never be read as host:port
            let ip = match addr.ip() {
                std::net::IpAddr::V6(ip) => format!("[{ip}]"),
                ip => ip.to_string(),
            };
            return Some(format!("MAP {name} {ip} , MAP * ~NOTFOUND"));
        }
        url::Host::Domain(name) => name.to_string(),
        url::Host::Ipv4(ip) => ip.to_string(),
        // bare, not bracketed: Chrome's EXCLUDE does not match `[::1]`
        url::Host::Ipv6(ip) => ip.to_string(),
    };
    Some(format!("MAP * ~NOTFOUND , EXCLUDE {host}"))
}

/// Whether a headless browser is installed, for URL captures without a display.
pub(super) fn is_available() -> bool {
    find_browser().is_some()
}

/// Finds a headless-capable browser: PATH candidates first, then well-known OS install locations.
fn find_browser() -> Option<String> {
    const CANDIDATES: &[&str] = &[
        "chromium",
        "chromium-browser",
        "google-chrome",
        "google-chrome-stable",
        "microsoft-edge",
        "brave",
        "brave-browser",
    ];
    for candidate in CANDIDATES {
        if which(candidate) {
            return Some((*candidate).to_string());
        }
    }
    for path in well_known_paths() {
        if Path::new(path).is_file() {
            return Some(path.to_string());
        }
    }
    None
}

fn well_known_paths() -> &'static [&'static str] {
    if cfg!(target_os = "macos") {
        &[
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        ]
    } else if cfg!(target_os = "windows") {
        &[
            r"C:\Program Files\Google\Chrome\Application\chrome.exe",
            r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
            r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
        ]
    } else {
        &["/usr/bin/chromium", "/usr/bin/google-chrome", "/snap/bin/chromium"]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // These assume LUXE_WEB_ALLOW_PRIVATE is unset (the default, and the CI environment).
    #[tokio::test]
    async fn blocks_file_scheme_by_default() {
        let err = validate_capture_url("file:///etc/passwd").await.unwrap_err().to_string();
        assert!(err.contains("file://"), "file:// must be refused: {err}");
        assert!(err.contains("`read`"), "and point at the tool that does this properly: {err}");
    }

    #[tokio::test]
    async fn rejects_non_web_scheme() {
        assert!(validate_capture_url("ftp://example.com/x").await.is_err());
        assert!(validate_capture_url("data:text/html,hi").await.is_err());
    }

    /// The capture this tool advertises: a dev server on this machine, with no env var set first and
    /// no session already running that could have inherited one. Every spelling a dev server prints,
    /// including the bracketed IPv6 literal Vite and Astro report.
    #[tokio::test]
    async fn captures_a_local_dev_server() {
        for url in ["http://127.0.0.1:4321/", "http://localhost:4321/", "http://[::1]:4321/"] {
            validate_capture_url(url)
                .await
                .unwrap_or_else(|error| panic!("{url} must be capturable: {error:#}"));
        }
        // Nothing local is pinned: Chrome's MAP takes one address, and choosing between `::1` and
        // `127.0.0.1` on the user's behalf is how a dev server on the other one becomes unreachable.
        // Every other name is unresolvable, so the page cannot pull in another host.
        for (url, host) in [
            ("http://localhost:4321/", "localhost"),
            ("http://127.0.0.1:4321/", "127.0.0.1"),
            ("http://[::1]:4321/", "::1"),
        ] {
            let rule = validate_capture_url(url).await.unwrap();
            assert_eq!(
                rule.as_deref(),
                Some(&*format!("MAP * ~NOTFOUND , EXCLUDE {host}")),
                "{url}"
            );
        }
    }

    #[tokio::test]
    async fn blocks_private_and_metadata_hosts() {
        assert!(validate_capture_url("http://192.168.0.5/admin").await.is_err());
        assert!(validate_capture_url("http://10.0.0.1/").await.is_err());
        assert!(validate_capture_url("http://169.254.169.254/latest/meta-data").await.is_err());
    }

    /// A public *IP literal* needs no pin — there is no lookup between us and Chrome to rebind. A
    /// `MAP 1.1.1.1 1.1.1.1` rule would map a host Chrome never resolves.
    #[tokio::test]
    async fn a_public_ip_literal_needs_no_resolver_pin() {
        let rule = validate_capture_url("http://1.1.1.1/").await.unwrap();
        assert_eq!(rule.as_deref(), Some("MAP * ~NOTFOUND , EXCLUDE 1.1.1.1"));
    }

    /// A pinned public name resolves to the address that was checked, and nothing else resolves:
    /// a page that redirects to or embeds the metadata endpoint or a LAN host gets no answer.
    #[test]
    fn a_pinned_name_is_the_only_one_that_resolves() {
        let addr = "93.184.216.34:443".parse().unwrap();
        let rule = resolver_rule(&url::Url::parse("https://example.com/").unwrap(), &[addr]);
        assert_eq!(rule.as_deref(), Some("MAP example.com 93.184.216.34 , MAP * ~NOTFOUND"));
        // an IPv6 replacement is bracketed, or Chrome cannot tell its colons from a port
        let addr = "[2606:2800:220:1::1]:443".parse().unwrap();
        let rule = resolver_rule(&url::Url::parse("https://example.com/").unwrap(), &[addr]);
        assert_eq!(rule.as_deref(), Some("MAP example.com [2606:2800:220:1::1] , MAP * ~NOTFOUND"));
    }

    /// The flags Chrome gets: a throwaway profile, the resolver rule, and `--no-sandbox` only where
    /// the sandbox cannot run.
    #[test]
    fn chrome_args_carry_the_profile_the_rule_and_the_sandbox_choice() {
        let profile = std::path::Path::new("/project/.luxe/browser/profile-1");
        let capture = ChromeCapture {
            url: "http://localhost:4321/",
            width: 800,
            height: 600,
            wait_ms: 500,
            out: "/project/.luxe/screenshots/shot.png",
            resolver_rule: Some("MAP * ~NOTFOUND , EXCLUDE localhost".into()),
            sandboxed: true,
            profile,
        };
        let args = chrome_args(&capture);
        assert!(args.contains(&"--user-data-dir=/project/.luxe/browser/profile-1".to_string()));
        assert!(args.contains(&"--host-resolver-rules=MAP * ~NOTFOUND , EXCLUDE localhost".into()));
        assert!(!args.contains(&"--no-sandbox".to_string()), "{args:?}");
        assert_eq!(args.last().map(String::as_str), Some("http://localhost:4321/"));
        let unsandboxed = chrome_args(&ChromeCapture { sandboxed: false, ..capture });
        assert!(unsandboxed.contains(&"--no-sandbox".to_string()), "{unsandboxed:?}");
    }

    /// Root and containers are where Chrome's sandbox cannot start; everywhere else it stays on.
    #[test]
    fn the_sandbox_is_dropped_only_where_it_cannot_run() {
        assert!(sandbox_unavailable(true, false));
        assert!(sandbox_unavailable(false, true));
        assert!(!sandbox_unavailable(false, false));
    }
}
