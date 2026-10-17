//! Guide cache — a model-written guide, kept for the next review of the same change.
//!
//! Reviewing your work and then `/pr` on it, or opening the same pull request twice, should not
//! pay for a second guide of a change the model has already explained. A guide is keyed on the
//! change's items — its hunks and non-text files, whose ids are derived from their content — so it
//! is reused exactly when the code is the same, whatever the target and however the diff was
//! captured, and any edit to the code writes a new one. What it cannot see is context that moved
//! without the code: a pull request description rewritten since.
//!
//! Only a guide the model wrote in full is kept. A structural fallback costs nothing to rebuild,
//! and a mixed one is exactly the guide worth asking for again.

use super::{
    source_items, validate_chapter_coverage, GuideProvenance, ReviewBrief, ReviewChapter,
    ReviewFile, ReviewItemRef,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Where guides live, relative to the project root.
pub const GUIDE_CACHE_RELATIVE_PATH: &str = ".luxe/reviews/guides";
/// Schema version of one kept guide.
pub const GUIDE_CACHE_SCHEMA_VERSION: u32 = 1;
/// Guides kept before the least recently written go.
const MAX_GUIDES: usize = 64;

/// A guide as kept.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachedGuide {
    /// Schema this was written with
    pub schema_version: u32,
    /// The chapters, notes included
    pub chapters: Vec<ReviewChapter>,
    /// The brief, when the pass wrote one
    pub brief: Option<ReviewBrief>,
    /// How the chapters were written
    pub provenance: GuideProvenance,
}

/// Guides kept under one project.
#[derive(Debug, Clone)]
pub struct GuideCache {
    directory: PathBuf,
}

impl GuideCache {
    /// The cache of the project at `project_root`.
    pub fn new(project_root: &Path) -> Self {
        Self { directory: project_root.join(GUIDE_CACHE_RELATIVE_PATH) }
    }

    /// The guide kept for exactly these files, when there is one and it still covers them.
    ///
    /// A file that does not read, or a guide that no longer covers every item, is treated as no
    /// guide at all: the worst a bad cache entry may do is cost a model pass.
    pub fn load(&self, files: &[ReviewFile]) -> Option<CachedGuide> {
        let key = key(files)?;
        let text = std::fs::read_to_string(self.directory.join(format!("{key}.json"))).ok()?;
        let guide: CachedGuide = serde_json::from_str(&text).ok()?;
        (guide.schema_version == GUIDE_CACHE_SCHEMA_VERSION
            && validate_chapter_coverage(files, &guide.chapters).is_ok())
        .then_some(guide)
    }

    /// Keeps a guide the model wrote for these files, and drops the oldest past the limit.
    pub fn save(
        &self,
        files: &[ReviewFile],
        chapters: &[ReviewChapter],
        brief: Option<&ReviewBrief>,
        provenance: GuideProvenance,
    ) -> std::io::Result<()> {
        if !matches!(provenance, GuideProvenance::Model { .. }) {
            return Ok(());
        }
        let Some(key) = key(files) else {
            return Ok(());
        };
        let guide = CachedGuide {
            schema_version: GUIDE_CACHE_SCHEMA_VERSION,
            chapters: chapters.to_vec(),
            brief: brief.cloned(),
            provenance,
        };
        std::fs::create_dir_all(&self.directory)?;
        let json = serde_json::to_vec(&guide).map_err(std::io::Error::other)?;
        crate::fs_atomic::write_private(&self.directory.join(format!("{key}.json")), &json)?;
        self.prune();
        Ok(())
    }

    /// Drops the least recently written guides past [`MAX_GUIDES`].
    fn prune(&self) {
        let Ok(entries) = std::fs::read_dir(&self.directory) else {
            return;
        };
        let mut guides: Vec<(std::time::SystemTime, PathBuf)> = entries
            .flatten()
            .filter(|entry| entry.path().extension().is_some_and(|extension| extension == "json"))
            .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
            .collect();
        if guides.len() <= MAX_GUIDES {
            return;
        }
        guides.sort();
        for (_, path) in &guides[..guides.len() - MAX_GUIDES] {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// The change's identity: a digest of its items in order, or `None` for a change with none.
fn key(files: &[ReviewFile]) -> Option<String> {
    let items = source_items(files);
    if items.is_empty() {
        return None;
    }
    let mut hasher = Sha256::new();
    hasher.update(b"luxe.review.guide.v1\0");
    for item in &items {
        let id = match item {
            ReviewItemRef::Hunk(hunk) => hunk.as_str(),
            ReviewItemRef::File(file) => file.as_str(),
        };
        hasher.update((id.len() as u64).to_be_bytes());
        hasher.update(id.as_bytes());
    }
    Some(base16ct::lower::encode_string(&hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guided_review::{fallback_chapters, parse_unified_diff};

    const DIFF: &str = "diff --git a/total.py b/total.py\n--- a/total.py\n+++ b/total.py\n\
                        @@ -1,2 +1,2 @@\n def total(numbers):\n-    return 0\n+    return sum(numbers)\n";

    fn files(diff: &str) -> Vec<ReviewFile> {
        parse_unified_diff(diff).unwrap().files
    }

    fn written(files: &[ReviewFile]) -> Vec<ReviewChapter> {
        let mut chapters = fallback_chapters(files).unwrap();
        chapters[0].title = "Sum every number".into();
        chapters
    }

    /// A guide the model wrote comes back for the same change, from another capture of it, and
    /// never for a changed one.
    #[test]
    fn the_same_change_gets_its_guide_back_and_a_changed_one_does_not() {
        let root = tempfile::tempdir().unwrap();
        let cache = GuideCache::new(root.path());
        let files = files(DIFF);
        let chapters = written(&files);
        let brief = ReviewBrief { intent: "Sum it.".into(), account: vec!["Sums.".into()] };
        let model = GuideProvenance::Model { batches: 1 };
        cache.save(&files, &chapters, Some(&brief), model).unwrap();

        // Captured again, here with a different index line: the same hunks
        let again = DIFF.replace("--- a/total.py", "index 1111111..2222222 100644\n--- a/total.py");
        let kept = cache.load(&self::files(&again)).expect("the same change");
        assert_eq!(kept.chapters, chapters);
        assert_eq!(kept.brief, Some(brief));
        assert_eq!(kept.provenance, model);

        let changed = DIFF.replace("sum(numbers)", "sum(numbers) + 0");
        assert!(cache.load(&self::files(&changed)).is_none());
    }

    /// A file reviewed while untracked, then committed and reviewed as a pull request, is the same
    /// change: reviewing your work and then `/pr` on it is the case this exists for.
    #[test]
    fn an_untracked_file_and_its_commit_are_the_same_change() {
        let untracked = crate::guided_review::render_untracked_addition(
            std::path::Path::new("test-note.md"),
            "Hello, world.\n",
        );
        // What `git diff` gives for the same file once committed, copied from a real capture
        let committed = "diff --git a/test-note.md b/test-note.md\nnew file mode 100644\n\
                         index 0000000..f75ba05\n--- /dev/null\n+++ b/test-note.md\n\
                         @@ -0,0 +1 @@\n+Hello, world.\n";
        assert_eq!(key(&files(&untracked)), key(&files(committed)));
    }

    /// Only a guide the model wrote in full is kept, and a kept one that no longer covers the
    /// change, or does not read, is no guide at all.
    #[test]
    fn only_a_whole_model_guide_is_kept_and_a_bad_one_is_ignored() {
        let root = tempfile::tempdir().unwrap();
        let cache = GuideCache::new(root.path());
        let files = files(DIFF);
        let chapters = written(&files);
        for provenance in [
            GuideProvenance::Structural { reason: None },
            GuideProvenance::Mixed { model_batches: 1, structural_batches: 1, reason: None },
        ] {
            cache.save(&files, &chapters, None, provenance).unwrap();
            assert!(cache.load(&files).is_none(), "{provenance:?}");
        }

        cache.save(&files, &[], None, GuideProvenance::Model { batches: 1 }).unwrap();
        assert!(cache.load(&files).is_none(), "covers nothing, so it is not this change's guide");

        let path = root
            .path()
            .join(GUIDE_CACHE_RELATIVE_PATH)
            .join(format!("{}.json", key(&files).unwrap()));
        std::fs::write(&path, "not json").unwrap();
        assert!(cache.load(&files).is_none());
        assert!(key(&[]).is_none(), "an empty change has nothing to key on");
    }

    /// The cache keeps the newest guides and drops the oldest past its limit.
    #[test]
    fn the_oldest_guides_go_past_the_limit() {
        let root = tempfile::tempdir().unwrap();
        let cache = GuideCache::new(root.path());
        let directory = root.path().join(GUIDE_CACHE_RELATIVE_PATH);
        std::fs::create_dir_all(&directory).unwrap();
        let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000);
        // Two over once the new one lands, so the count dropped is the difference
        for n in 0..=MAX_GUIDES {
            let path = directory.join(format!("{n:02}.json"));
            std::fs::write(&path, "{}").unwrap();
            let file = std::fs::File::options().write(true).open(&path).unwrap();
            file.set_modified(old + std::time::Duration::from_secs(n as u64)).unwrap();
        }
        let files = files(DIFF);
        cache.save(&files, &written(&files), None, GuideProvenance::Model { batches: 1 }).unwrap();
        let left = std::fs::read_dir(&directory).unwrap().count();
        assert_eq!(left, MAX_GUIDES);
        assert!(!directory.join("00.json").exists(), "the oldest went");
        assert!(!directory.join("01.json").exists(), "and the next oldest");
        assert!(directory.join("02.json").exists());
        assert!(cache.load(&files).is_some(), "the newest stayed");
    }
}
