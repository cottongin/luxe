//! On-disk catalog cache.
//!
//! The cache is derived data, so it lives under `$XDG_CACHE_HOME` and any read failure simply
//! means "no cache". A sidecar carries the `ETag` and the last-checked time: freshness is
//! tracked there rather than by file mtime so a `304` can reset the clock without rewriting
//! several megabytes.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::schema::{parse, Catalog};

/// Revalidation state stored alongside the cached catalog.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Meta {
    /// `ETag` from the last successful fetch, replayed as `If-None-Match`
    #[serde(default)]
    pub etag: Option<String>,
    /// Unix seconds of the last check, whether it returned `200` or `304`
    #[serde(default)]
    pub checked_at: u64,
}

impl Meta {
    /// How long since the catalog was last revalidated.
    pub fn age(&self) -> Option<Duration> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
        Some(Duration::from_secs(now.saturating_sub(self.checked_at)))
    }

    /// Whether the last check is older than `ttl`.
    pub fn is_stale(&self, ttl: Duration) -> bool {
        self.age().is_none_or(|age| age >= ttl)
    }
}

/// The sidecar path for a cache file.
fn meta_path(path: &Path) -> PathBuf {
    path.with_extension("meta.json")
}

/// Read the cached catalog, or `None` when it's absent or unparseable.
pub fn load(path: &Path) -> Option<Catalog> {
    let body = std::fs::read_to_string(path).ok()?;
    match parse(&body) {
        Ok(catalog) => Some(catalog),
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "ignoring corrupt models.dev cache");
            None
        }
    }
}

/// Read the sidecar, defaulting to "never checked".
pub fn load_meta(path: &Path) -> Meta {
    std::fs::read_to_string(meta_path(path))
        .ok()
        .and_then(|body| serde_json::from_str(&body).ok())
        .unwrap_or_default()
}

/// Write the catalog body and sidecar, replacing any previous copy.
///
/// Atomically, so a concurrent reader (or a second `luxe` refreshing at the same time) always sees a
/// whole file.
pub fn store(path: &Path, body: &str, etag: Option<String>) -> std::io::Result<()> {
    luxe_core::fs_atomic::replace(path, body.as_bytes())?;
    touch(path, etag)
}

/// Record a successful revalidation without rewriting the body (the `304` path).
pub fn touch(path: &Path, etag: Option<String>) -> std::io::Result<()> {
    let checked_at =
        SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or_default();
    let meta = Meta { etag: etag.or_else(|| load_meta(path).etag), checked_at };
    let body = serde_json::to_string(&meta).unwrap_or_default();
    luxe_core::fs_atomic::replace(&meta_path(path), body.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &str = r#"{"anthropic":{"models":{"claude-opus-5":{"name":"Claude Opus 5"}}}}"#;

    #[test]
    fn store_then_load_round_trips_body_and_meta() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nested").join("models-dev.json");

        assert!(load(&path).is_none(), "no cache yet");
        assert!(load_meta(&path).is_stale(Duration::from_secs(1)));

        store(&path, BODY, Some("\"abc\"".into())).expect("store");
        let catalog = load(&path).expect("cache reads back");
        assert!(catalog.model("anthropic", "claude-opus-5").is_some());

        let meta = load_meta(&path);
        assert_eq!(meta.etag.as_deref(), Some("\"abc\""));
        assert!(!meta.is_stale(Duration::from_secs(3600)));
        // No temp files left behind.
        let leftovers: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains("tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temp file survived: {leftovers:?}");
    }

    #[test]
    fn touch_resets_the_clock_and_keeps_the_etag() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("models-dev.json");
        store(&path, BODY, Some("\"abc\"".into())).expect("store");

        // Simulate an old check, then revalidate with no new etag (a 304).
        std::fs::write(path.with_extension("meta.json"), r#"{"etag":"\"abc\"","checked_at":1}"#)
            .unwrap();
        assert!(load_meta(&path).is_stale(Duration::from_secs(3600)));

        touch(&path, None).expect("touch");
        let meta = load_meta(&path);
        assert!(!meta.is_stale(Duration::from_secs(3600)), "clock reset");
        assert_eq!(meta.etag.as_deref(), Some("\"abc\""), "etag survives a 304");
    }

    #[test]
    fn a_corrupt_cache_reads_as_absent_rather_than_panicking() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("models-dev.json");
        std::fs::write(&path, "{ not json").unwrap();
        assert!(load(&path).is_none());
    }
}
