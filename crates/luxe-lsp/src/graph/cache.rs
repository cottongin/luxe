//! Cache — content-addressed per-file graph shards under `.luxe/graph/` (design-code-graph.md §5).
//!
//! Warm-start without re-parsing: each file's *pre-resolution* [`FileGraph`] is written to a shard
//! validated by the content hash it came from. On rebuild, a file whose content hash still matches
//! its shard is loaded straight from disk (no tree-sitter parse); changed/new files re-extract and
//! rewrite their shard. Edge *resolution* is never cached — it's recomputed in memory on load, so a
//! dependency's shard never needs rewriting when a resolution target moves. `.luxe/` is gitignored
//! and every shard is regenerable, so a corrupt/missing shard just triggers a re-parse.
//!
//! A shard also records the [`EXTRACTOR_VERSION`] that wrote it, because a matching content hash
//! only says the file is unchanged, not that today's extractor would read it the same way. Writes
//! are atomic, so a second Luxe in the same project never reads a torn shard.

use super::extract::FileGraph;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// SHA-256 hex digest of `bytes`.
fn hash_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    base16ct::lower::encode_string(&hasher.finalize())
}

/// Content hash for a file's source — the shard's validity key.
pub fn content_hash(content: &str) -> String {
    hash_hex(content.as_bytes())
}

/// The version of what [`super::extract`] produces. Bump it whenever extraction changes — a new
/// symbol kind, an edge rule — so every shard written before is a miss and re-parses once.
const EXTRACTOR_VERSION: u32 = 1;

/// Borrowed shard for writing (avoids cloning the fragment).
#[derive(Serialize)]
struct ShardOut<'a> {
    extractor_version: u32,
    content_hash: &'a str,
    graph: &'a FileGraph,
}

/// Owned shard for reading. A shard from before versioning reads as version 0, a miss.
#[derive(Deserialize)]
struct ShardIn {
    #[serde(default)]
    extractor_version: u32,
    content_hash: String,
    graph: FileGraph,
}

/// The manifest as written: which extractor indexed which file at which content hash.
#[derive(Serialize, Deserialize)]
struct Manifest {
    extractor_version: u32,
    files: HashMap<String, String>,
}

/// On-disk graph cache rooted at `<project>/.luxe/graph/`.
pub struct GraphCache {
    dir: PathBuf,
}

impl GraphCache {
    /// A cache under `project_root/.luxe/graph`.
    pub fn new(project_root: &Path) -> Self {
        Self { dir: project_root.join(".luxe").join("graph") }
    }

    fn files_dir(&self) -> PathBuf {
        self.dir.join("files")
    }

    /// A shard's path — filename is a hash of the project-relative path (stable per file).
    fn shard_path(&self, rel_path: &str) -> PathBuf {
        self.files_dir().join(format!("{}.json", hash_hex(rel_path.as_bytes())))
    }

    fn manifest_path(&self) -> PathBuf {
        self.dir.join("manifest.json")
    }

    /// Load a file's cached fragment iff its shard's content hash matches `content_hash` and this
    /// extractor wrote it — a hit means the file needn't be re-parsed. Any read/parse failure is a
    /// miss.
    pub fn load(&self, rel_path: &str, content_hash: &str) -> Option<FileGraph> {
        let bytes = std::fs::read(self.shard_path(rel_path)).ok()?;
        let shard: ShardIn = serde_json::from_slice(&bytes).ok()?;
        (shard.extractor_version == EXTRACTOR_VERSION && shard.content_hash == content_hash)
            .then_some(shard.graph)
    }

    /// Write a file's fragment shard (best-effort; a cache write failure is non-fatal).
    pub fn store(&self, rel_path: &str, content_hash: &str, graph: &FileGraph) {
        let shard = ShardOut { extractor_version: EXTRACTOR_VERSION, content_hash, graph };
        write_best_effort(
            &self.files_dir(),
            &self.shard_path(rel_path),
            serde_json::to_vec(&shard),
        );
    }

    /// The persisted manifest: project-relative path → last-indexed content hash, empty when a
    /// different extractor wrote it.
    pub fn load_manifest(&self) -> HashMap<String, String> {
        std::fs::read(self.manifest_path())
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Manifest>(&bytes).ok())
            .filter(|manifest| manifest.extractor_version == EXTRACTOR_VERSION)
            .map(|manifest| manifest.files)
            .unwrap_or_default()
    }

    /// Persist the manifest (best-effort).
    pub fn store_manifest(&self, manifest: &HashMap<String, String>) {
        let manifest = Manifest { extractor_version: EXTRACTOR_VERSION, files: manifest.clone() };
        write_best_effort(&self.dir, &self.manifest_path(), serde_json::to_vec_pretty(&manifest));
    }

    /// Delete shards for files no longer in `manifest` (renamed/removed files leave orphans).
    pub fn prune(&self, manifest: &HashMap<String, String>) {
        let keep: HashSet<String> =
            manifest.keys().map(|rel| format!("{}.json", hash_hex(rel.as_bytes()))).collect();
        let Ok(entries) = std::fs::read_dir(self.files_dir()) else { return };
        for entry in entries.flatten() {
            let name = entry.file_name();
            if let Some(name) = name.to_str() {
                if !keep.contains(name) {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    }
}

/// Writes `json` to `path`, creating `dir` first: atomically, so a reader never sees half a shard,
/// and unsynced, since a cold build writes thousands and a lost one costs only a re-parse. A
/// failure is logged and dropped.
fn write_best_effort(dir: &Path, path: &Path, json: serde_json::Result<Vec<u8>>) {
    let written = json
        .map_err(std::io::Error::other)
        .and_then(|json| std::fs::create_dir_all(dir).map(|()| json))
        .and_then(|json| luxe_core::fs_atomic::replace_unsynced(path, &json));
    if let Err(error) = written {
        tracing::debug!(%error, path = %path.display(), "graph cache write failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::extract::extract_file;

    #[test]
    fn content_hash_keeps_the_hex_spelling_written_shards_are_keyed_by() {
        // Literal computed outside this crate. Shards on disk carry this string as their validity
        // key, so a re-spelled digest turns every warm cache into a silent full re-parse.
        assert_eq!(
            content_hash("fn a() { b(); }\nfn b() {}\n"),
            "f1bcafc4e7c84d96ad4dd5062fe4f5f6854b2edb4ced6ee98f972264ed565399"
        );
    }

    #[test]
    fn shard_round_trips_and_validates_content_hash() {
        let tmp_guard = tempfile::tempdir().expect("temp dir");
        let tmp = tmp_guard.path().to_path_buf();
        std::fs::create_dir_all(&tmp).unwrap();
        let cache = GraphCache::new(&tmp);

        let src = "fn a() { b(); }\nfn b() {}\n";
        let fg = extract_file("src/x.rs", src).unwrap();
        let hash = content_hash(src);
        cache.store("src/x.rs", &hash, &fg);

        // Same hash → hit with identical fragment.
        let loaded = cache.load("src/x.rs", &hash).expect("cache hit");
        assert_eq!(loaded.nodes, fg.nodes);
        assert_eq!(loaded.edges, fg.edges);

        // Different hash (edited file) → miss.
        assert!(cache.load("src/x.rs", &content_hash("fn a() {}\n")).is_none());
    }

    #[test]
    fn prune_removes_orphan_shards() {
        let tmp_guard = tempfile::tempdir().expect("temp dir");
        let tmp = tmp_guard.path().to_path_buf();
        std::fs::create_dir_all(&tmp).unwrap();
        let cache = GraphCache::new(&tmp);

        let fg = extract_file("a.rs", "fn a() {}\n").unwrap();
        cache.store("a.rs", &content_hash("fn a() {}\n"), &fg);
        cache.store("b.rs", &content_hash("fn b() {}\n"), &fg);

        // Keep only a.rs.
        let manifest: HashMap<String, String> =
            [("a.rs".to_string(), content_hash("fn a() {}\n"))].into_iter().collect();
        cache.prune(&manifest);

        assert!(cache.load("a.rs", &content_hash("fn a() {}\n")).is_some());
        assert!(cache.load("b.rs", &content_hash("fn b() {}\n")).is_none());
    }

    /// A shard another extractor wrote is a miss even when the file is unchanged, and so is one
    /// from before shards were versioned.
    ///
    /// The break this catches: validity was the content hash alone, so after an upgrade changed
    /// extraction every unchanged file kept serving its old fragment.
    #[test]
    fn a_shard_from_another_extractor_misses() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = GraphCache::new(tmp.path());
        let src = "fn a() {}\n";
        let (fg, hash) = (extract_file("a.rs", src).unwrap(), content_hash(src));
        cache.store("a.rs", &hash, &fg);
        assert!(cache.load("a.rs", &hash).is_some(), "this extractor's shard hits");

        let older = |version: Option<u32>| {
            let mut shard = serde_json::json!({ "content_hash": hash, "graph": fg });
            if let Some(version) = version {
                shard["extractor_version"] = version.into();
            }
            std::fs::write(cache.shard_path("a.rs"), shard.to_string()).unwrap();
        };
        older(Some(EXTRACTOR_VERSION - 1));
        assert!(cache.load("a.rs", &hash).is_none(), "an older extractor's shard misses");
        older(None);
        assert!(cache.load("a.rs", &hash).is_none(), "an unversioned shard misses");
        older(Some(EXTRACTOR_VERSION + 1));
        assert!(cache.load("a.rs", &hash).is_none(), "and so does a newer extractor's");
    }

    /// The manifest says which extractor wrote it, and one from another is no manifest at all.
    #[test]
    fn a_manifest_from_another_extractor_reads_as_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = GraphCache::new(tmp.path());
        let files: HashMap<String, String> = [("a.rs".into(), "h".into())].into_iter().collect();
        cache.store_manifest(&files);
        assert_eq!(cache.load_manifest(), files);
        std::fs::write(cache.manifest_path(), r#"{"a.rs":"h"}"#).unwrap();
        assert!(cache.load_manifest().is_empty(), "the unversioned shape");
    }
}
