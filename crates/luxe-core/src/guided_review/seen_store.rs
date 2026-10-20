//! What a reviewer has already looked at, per project and per target.
//!
//! A review is worth doing when it shows you what you have *not* seen. Without a durable record of
//! the last one, every `/diff` re-presents the whole accumulated change and the reviewer pays for
//! their own diligence, until a working-tree review reaches a hundred-odd items and stops being a
//! review at all.
//!
//! Only the sections actually walked are recorded: opening a review, glancing at the first section
//! and leaving must not mark three hundred items as seen.

use super::{ChapterId, GuidedReview, ReviewItemRef, ReviewRevisionId, ReviewTarget};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Where the record lives, relative to the project root.
pub const SEEN_REVIEW_RELATIVE_PATH: &str = ".luxe/reviews/seen.json";
/// Schema version for the durable record.
pub const SEEN_REVIEW_SCHEMA_VERSION: u32 = 1;
/// Most items retained per target; a review larger than this needs the file-level view anyway.
const MAX_SEEN_ITEMS: usize = 4_000;
/// Most files retained per target.
const MAX_SEEN_FILES: usize = 2_000;

/// What one finished review left behind.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SeenReview {
    /// The pinned revision the reviewer read.
    pub revision_id: Option<ReviewRevisionId>,
    /// When it was recorded, milliseconds since the Unix epoch.
    pub at_ms: u64,
    /// Item ids (hunks and non-text files) inside the sections the reviewer walked.
    pub item_ids: Vec<String>,
    /// Per-file `(additions, deletions)` as they stood, so a file that moved again is spotted
    /// cheaply — the dock cannot afford to re-parse a diff to answer "how much is new".
    pub files: BTreeMap<String, (u64, u64)>,
}

impl SeenReview {
    /// Whether this item was inside a section the reviewer walked.
    pub fn covers_item(&self, item_id: &str) -> bool {
        self.item_ids.iter().any(|seen| seen == item_id)
    }

    /// Whether a file is unchanged since it was last seen (same \u00b1 counts).
    pub fn covers_file(&self, path: &str, additions: u64, deletions: u64) -> bool {
        self.files.get(path).is_some_and(|seen| *seen == (additions, deletions))
    }

    /// Whether anything was recorded at all.
    pub fn is_empty(&self) -> bool {
        self.item_ids.is_empty() && self.files.is_empty()
    }

    /// What reading `walked` of `review` covered, or `None` when no section was walked.
    ///
    /// Only walked sections count: opening a review, glancing at the first section and leaving
    /// must not mark three hundred items as seen. Every file is recorded with its ± counts, so the
    /// dock can spot one that moved again without re-parsing a diff.
    pub fn from_walked(review: &GuidedReview, walked: &[ChapterId], at_ms: u64) -> Option<Self> {
        let walked: Vec<_> =
            review.chapters.iter().filter(|chapter| walked.contains(&chapter.id)).collect();
        if walked.is_empty() {
            return None;
        }
        let item_ids = walked
            .iter()
            .flat_map(|chapter| &chapter.items)
            .map(|item| match item {
                ReviewItemRef::Hunk(id) => id.as_str().to_owned(),
                ReviewItemRef::File(id) => id.as_str().to_owned(),
            })
            .collect();
        let files = review
            .files
            .iter()
            .filter_map(|file| {
                let path = file.new_path.as_deref().or(file.old_path.as_deref())?;
                Some((
                    path.to_string_lossy().into_owned(),
                    (file.additions.unwrap_or_default(), file.deletions.unwrap_or_default()),
                ))
            })
            .collect();
        Some(Self { revision_id: Some(review.revision_id.clone()), at_ms, item_ids, files })
    }
}

/// The durable file: one record per review target.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SeenReviewFile {
    schema_version: u32,
    #[serde(default)]
    targets: BTreeMap<String, SeenReview>,
}

impl Default for SeenReviewFile {
    fn default() -> Self {
        Self { schema_version: SEEN_REVIEW_SCHEMA_VERSION, targets: BTreeMap::new() }
    }
}

/// Reads and writes the per-project record of what has been reviewed.
#[derive(Debug, Clone)]
pub struct SeenReviewStore {
    path: PathBuf,
}

impl SeenReviewStore {
    /// Opens the store for one project root.
    pub fn new(project_root: impl AsRef<Path>) -> Self {
        Self { path: project_root.as_ref().join(SEEN_REVIEW_RELATIVE_PATH) }
    }

    /// What was last reviewed for `target`, or `None` when nothing was.
    ///
    /// A malformed or future-schema file reads as "nothing reviewed": the record is an optimisation
    /// for the reviewer, never a gate, so it must never fail a review open.
    pub fn load(&self, target: &ReviewTarget) -> Option<SeenReview> {
        let text = std::fs::read_to_string(&self.path).ok()?;
        let file: SeenReviewFile = serde_json::from_str(&text).ok()?;
        if file.schema_version != SEEN_REVIEW_SCHEMA_VERSION {
            return None;
        }
        file.targets.get(&target_key(target)).cloned().filter(|seen| !seen.is_empty())
    }

    /// Records what this review left behind, replacing the previous record for the same target.
    pub fn record(&self, target: &ReviewTarget, mut seen: SeenReview) -> std::io::Result<()> {
        seen.item_ids.sort();
        seen.item_ids.dedup();
        seen.item_ids.truncate(MAX_SEEN_ITEMS);
        while seen.files.len() > MAX_SEEN_FILES {
            let Some(last) = seen.files.keys().next_back().cloned() else { break };
            seen.files.remove(&last);
        }
        let mut file: SeenReviewFile = std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .filter(|file: &SeenReviewFile| file.schema_version == SEEN_REVIEW_SCHEMA_VERSION)
            .unwrap_or_default();
        file.targets.insert(target_key(target), seen);
        let body = serde_json::to_string_pretty(&file)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        crate::fs_atomic::replace_private(&self.path, body.as_bytes())
    }
}

/// The stable key one target records under.
fn target_key(target: &ReviewTarget) -> String {
    match target {
        ReviewTarget::WorkingTree => "working_tree".into(),
        // A turn is reviewed once and never returned to, so every turn shares one slot.
        ReviewTarget::LastTurn { .. } => "last_turn".into(),
        ReviewTarget::Base { reference } => format!("base:{reference}"),
        ReviewTarget::Commit { oid } => format!("commit:{oid}"),
        // By number, never by revision: the second look after the author pushes has to find the
        // first. GitHub ignores case in a repository's name, so the key does too.
        ReviewTarget::PullRequest { number, repository } => format!(
            "pull_request:{}#{number}",
            repository.as_deref().unwrap_or_default().to_ascii_lowercase()
        ),
        // Its own slot: this one runs to the working tree, so what was seen of the head alone is
        // not what was seen here
        ReviewTarget::OwnPullRequest { number } => format!("own_pull_request:#{number}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(items: &[&str]) -> SeenReview {
        SeenReview {
            revision_id: Some(ReviewRevisionId::from_canonical_bytes(b"revision")),
            at_ms: 42,
            item_ids: items.iter().map(|item| (*item).to_owned()).collect(),
            files: [("src/lib.rs".to_string(), (3, 1))].into_iter().collect(),
        }
    }

    #[test]
    fn a_record_round_trips_per_target() {
        let project = tempfile::tempdir().unwrap();
        let store = SeenReviewStore::new(project.path());
        assert!(store.load(&ReviewTarget::WorkingTree).is_none(), "nothing reviewed yet");

        store.record(&ReviewTarget::WorkingTree, seen(&["hunk:a", "hunk:b"])).unwrap();
        store.record(&ReviewTarget::Base { reference: "main".into() }, seen(&["hunk:c"])).unwrap();

        let working = store.load(&ReviewTarget::WorkingTree).expect("a record");
        assert!(working.covers_item("hunk:a") && working.covers_item("hunk:b"));
        assert!(!working.covers_item("hunk:c"), "targets keep their own record");
        assert!(working.covers_file("src/lib.rs", 3, 1));
        assert!(!working.covers_file("src/lib.rs", 4, 1), "a file that moved again is not covered");
    }

    /// The record is an optimisation, never a gate: a corrupt file must not fail a review.
    /// A pull request is remembered by number, so the look after a push finds the one before it;
    /// the repository's case is GitHub's business, not a second record's.
    #[test]
    fn a_pull_request_is_remembered_by_number_and_repository() {
        let dir = tempfile::tempdir().unwrap();
        let store = SeenReviewStore::new(dir.path());
        let pr = |number, repository: Option<&str>| ReviewTarget::PullRequest {
            number,
            repository: repository.map(str::to_owned),
        };
        store.record(&pr(7, Some("Upstream/luxe")), seen(&["a"])).unwrap();
        store.record(&pr(7, None), seen(&["b"])).unwrap();

        assert_eq!(store.load(&pr(7, Some("upstream/luxe"))).unwrap().item_ids, vec!["a"]);
        assert_eq!(store.load(&pr(7, None)).unwrap().item_ids, vec!["b"], "a different repository");
        assert!(store.load(&pr(8, None)).is_none(), "a different pull request");
    }

    #[test]
    fn a_corrupt_record_reads_as_nothing_reviewed() {
        let project = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(project.path().join(".luxe/reviews")).unwrap();
        std::fs::write(project.path().join(SEEN_REVIEW_RELATIVE_PATH), "{ not json").unwrap();

        let store = SeenReviewStore::new(project.path());
        assert!(store.load(&ReviewTarget::WorkingTree).is_none());
        // …and it can still be written over.
        store.record(&ReviewTarget::WorkingTree, seen(&["hunk:a"])).unwrap();
        assert!(store.load(&ReviewTarget::WorkingTree).is_some());
    }

    #[cfg(unix)]
    #[test]
    fn the_record_is_owner_readable_only() {
        use std::os::unix::fs::PermissionsExt;
        let project = tempfile::tempdir().unwrap();
        let store = SeenReviewStore::new(project.path());
        store.record(&ReviewTarget::WorkingTree, seen(&["hunk:a"])).unwrap();

        let mode = std::fs::metadata(project.path().join(SEEN_REVIEW_RELATIVE_PATH))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    /// A record covers exactly the sections walked, and every file as it stood.
    ///
    /// The break this catches is the one the record exists to prevent: counting a section nobody
    /// read as seen, which hides it from the next review's "what is new".
    #[test]
    fn a_record_covers_the_walked_sections_and_no_more() {
        let review = crate::guided_review::test_support::review_from(
            crate::guided_review::test_support::TWO_FILES,
        );
        assert!(review.chapters.len() >= 2, "the fixture needs a section left unwalked");
        assert_eq!(SeenReview::from_walked(&review, &[], 7), None, "nothing walked, nothing seen");

        let walked = &review.chapters[0];
        let record = SeenReview::from_walked(&review, std::slice::from_ref(&walked.id), 7)
            .expect("a record");
        let expected: Vec<String> = walked
            .items
            .iter()
            .map(|item| match item {
                ReviewItemRef::Hunk(id) => id.as_str().to_owned(),
                ReviewItemRef::File(id) => id.as_str().to_owned(),
            })
            .collect();
        assert_eq!(record.item_ids, expected);
        assert_eq!(record.revision_id.as_ref(), Some(&review.revision_id));
        assert_eq!(record.at_ms, 7);
        assert_eq!(record.files.len(), review.files.len(), "every file, walked or not");
        assert!(record.covers_file("src/a.rs", 1, 1));
        assert!(record.covers_file("tests/b.rs", 1, 0));
    }
}
