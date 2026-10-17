//! Build-time models.dev snapshot.
//!
//! The floor under every other layer: a fresh install with no network, no cache, and no config
//! still resolves real context windows, output caps, reasoning levels, and pricing. Regenerate
//! with `scripts/update-models-snapshot.sh`; the runtime cache supersedes it for anything
//! released since the build.

use std::sync::OnceLock;

use super::schema::{parse, Catalog};

/// The gzipped projection produced by `scripts/update-models-snapshot.sh` (~130 KB).
const SNAPSHOT_GZ: &[u8] = include_bytes!("../../data/models-dev-snapshot.json.gz");

/// The embedded catalog, inflated and parsed once on first use.
///
/// A corrupt blob degrades to an empty catalog rather than killing startup — the cache and
/// config layers still work, which is exactly the pre-snapshot behavior.
pub fn snapshot() -> &'static Catalog {
    static SNAPSHOT: OnceLock<Catalog> = OnceLock::new();
    SNAPSHOT.get_or_init(|| match decode() {
        Ok(catalog) => catalog,
        Err(error) => {
            tracing::warn!(%error, "embedded models.dev snapshot unreadable");
            Catalog::default()
        }
    })
}

fn decode() -> Result<Catalog, String> {
    use std::io::Read;
    let mut json = String::new();
    flate2::read::GzDecoder::new(SNAPSHOT_GZ)
        .read_to_string(&mut json)
        .map_err(|error| error.to_string())?;
    parse(&json).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_snapshot_decodes_and_carries_real_metadata() {
        let catalog = snapshot();
        assert!(catalog.len() > 1_000, "snapshot looks truncated: {} models", catalog.len());
        assert!(catalog.providers.contains_key("anthropic"));
        assert!(catalog.providers.contains_key("openai"));

        // Spot-check a model that shipped after this code was written: the whole point is that a
        // recent release resolves properly with no network and no config.
        let opus = catalog.model("anthropic", "claude-opus-5").expect("claude-opus-5 in snapshot");
        assert_eq!(opus.limit.as_ref().and_then(|l| l.context), Some(1_000_000));
        assert_eq!(opus.limit.as_ref().and_then(|l| l.output), Some(128_000));
        assert_eq!(opus.reasoning, Some(true));
        assert!(opus.cost.as_ref().and_then(|c| c.cache_read).is_some());
        assert!(!opus.reasoning_options.is_empty());

        // The current Anthropic generation offers effort and no token budget, which is what routes
        // it to adaptive thinking; sending extended thinking to these models is a 400.
        for id in ["claude-opus-5", "claude-sonnet-5"] {
            let (info, _) = super::super::resolve(
                "anthropic",
                id,
                &super::super::Sources { snapshot: Some(catalog), ..<_>::default() },
            );
            assert!(info.thinking_controls.effort_only(), "{id} must resolve to effort-only");
        }
    }

    /// The snapshot must not be *pathologically* slow to inflate. It sits on the startup path, but
    /// only once — [`snapshot`] holds it in a `OnceLock`.
    ///
    /// The budget is deliberately loose, and that is the point. It was 500ms, which is within a
    /// factor of two of what a loaded runner actually takes (706ms on windows-latest against ~50ms
    /// on a developer machine), so it was measuring the runner rather than the code and went red on
    /// a commit that touched neither. What is worth catching is a *catastrophe* — a codec swap, or
    /// decoding per call instead of once — and an order of magnitude still catches that.
    #[test]
    fn decoding_is_not_pathologically_slow_on_the_startup_path() {
        let start = std::time::Instant::now();
        let catalog = decode().expect("snapshot decodes");
        let elapsed = start.elapsed();

        assert!(catalog.len() > 1_000, "a decode that returns nothing proves nothing");
        assert!(elapsed < std::time::Duration::from_secs(5), "snapshot decode took {elapsed:?}");
    }
}
