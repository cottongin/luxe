//! Review identifiers — stable keys for pinned review artifacts.
//!
//! IDs are independent of a review revision where callers need refresh-stable selection and
//! feedback anchors. Revision IDs deliberately use SHA-256: cache identity must be
//! content-addressed, not merely process-local.

use sha2::{Digest, Sha256};
use std::path::Path;
use uuid::Uuid;

/// Identifies one logical review session across refreshes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct ReviewId(String);

impl ReviewId {
    /// Creates a distinct logical review identifier.
    pub fn new() -> Self {
        Self(format!("review:{}", Uuid::new_v4()))
    }

    /// Returns the stable serialized identifier.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for ReviewId {
    fn default() -> Self {
        Self::new()
    }
}

/// Identifies one durable review finding across revisions and submissions.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct FindingId(String);

impl FindingId {
    /// Creates a distinct opaque finding identifier.
    pub fn new() -> Self {
        Self(format!("finding:{}", Uuid::new_v4()))
    }

    /// Returns the stable serialized identifier.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for FindingId {
    fn default() -> Self {
        Self::new()
    }
}

/// Identifies one immutable feedback submission attempt.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct SubmissionId(String);

impl SubmissionId {
    /// Creates a distinct opaque submission identifier.
    pub fn new() -> Self {
        Self(format!("submission:{}", Uuid::new_v4()))
    }

    /// Returns the stable serialized identifier.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for SubmissionId {
    fn default() -> Self {
        Self::new()
    }
}

/// Identifies one immutable, content-addressed revision of a review source.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct ReviewRevisionId(String);

impl ReviewRevisionId {
    /// Derives an identifier from canonical source bytes.
    pub fn from_canonical_bytes(bytes: &[u8]) -> Self {
        Self(format!("revision:{}", digest("luxe.review.revision.v1", &[bytes])))
    }

    /// Returns the stable serialized identifier.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the filesystem-safe content digest used for source artifact directories.
    pub fn cache_key(&self) -> &str {
        self.0.strip_prefix("revision:").unwrap_or(&self.0)
    }
}

/// Identifies one engine-owned agent turn.
///
/// A turn ID exists before any pre-turn source capture, so review baselines never depend on
/// mutable transcript ordering.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct AgentTurnId(String);

impl AgentTurnId {
    /// Creates a distinct opaque turn identifier.
    pub fn new() -> Self {
        Self(format!("turn:{}", Uuid::new_v4()))
    }

    /// Returns the stable serialized identifier.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for AgentTurnId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl Default for AgentTurnId {
    fn default() -> Self {
        Self::new()
    }
}

/// Identifies one retained turn snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct TurnBaselineId(String);

impl TurnBaselineId {
    /// Derives a stable pre-turn snapshot key without exposing it as a filesystem path.
    pub fn from_session_turn(session_id: &str, turn_id: &str) -> Self {
        Self(format!(
            "turn-baseline:{}",
            digest("luxe.review.turn-baseline.v1", &[session_id.as_bytes(), turn_id.as_bytes()])
        ))
    }

    /// Derives a stable post-turn snapshot key distinct from the matching pre-turn snapshot.
    pub fn post_turn_from_session_turn(session_id: &str, turn_id: &str) -> Self {
        Self(format!(
            "turn-baseline:{}",
            digest(
                "luxe.review.turn-post-snapshot.v1",
                &[session_id.as_bytes(), turn_id.as_bytes()]
            )
        ))
    }

    /// Returns the stable serialized identifier.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the filesystem-safe digest used for baseline artifact directories.
    pub fn cache_key(&self) -> &str {
        self.0.strip_prefix("turn-baseline:").unwrap_or(&self.0)
    }
}

/// Identifies a file lineage independently of review ordering.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct FileId(String);

impl FileId {
    /// Derives an identifier from the old and new paths of a file lineage.
    pub fn from_path_lineage(old_path: Option<&Path>, new_path: Option<&Path>) -> Self {
        let old = old_path
            .map_or_else(|| b"/dev/null".as_slice(), |path| path.as_os_str().as_encoded_bytes());
        let new = new_path
            .map_or_else(|| b"/dev/null".as_slice(), |path| path.as_os_str().as_encoded_bytes());
        Self(format!("file:{}", digest("luxe.review.file.v1", &[old, new])))
    }

    /// Returns the stable serialized identifier.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Identifies a changed hunk independently of review ordering.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct HunkId(String);

impl HunkId {
    /// Derives an identifier from file lineage, normalized context, and changed content.
    pub fn from_content(file_id: &FileId, normalized_context: &str, changed_content: &str) -> Self {
        Self(format!(
            "hunk:{}",
            digest(
                "luxe.review.hunk.v1",
                &[
                    file_id.as_str().as_bytes(),
                    normalized_context.as_bytes(),
                    changed_content.as_bytes()
                ],
            )
        ))
    }

    /// Returns the stable serialized identifier.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Identifies a Guide chapter within a review revision.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct ChapterId(String);

impl ChapterId {
    /// Derives an identifier from the chapter kind and ordered immutable item keys.
    pub fn from_item_keys(kind: &str, item_keys: &[&str]) -> Self {
        let mut parts = Vec::with_capacity(item_keys.len() + 1);
        parts.push(kind.as_bytes());
        parts.extend(item_keys.iter().map(|item| item.as_bytes()));
        Self(format!("chapter:{}", digest("luxe.review.chapter.v1", &parts)))
    }

    /// Derives an identifier from the chapter kind and ordered text-hunk IDs.
    pub fn from_items(kind: &str, item_ids: &[HunkId]) -> Self {
        let keys = item_ids.iter().map(HunkId::as_str).collect::<Vec<_>>();
        Self::from_item_keys(kind, &keys)
    }

    /// Returns the stable serialized identifier.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Identifies one asynchronous review-load request.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct ReviewLoadRequestId(String);

impl ReviewLoadRequestId {
    /// Creates a distinct request identifier for stale-load rejection.
    pub fn new() -> Self {
        Self(format!("review-load:{}", Uuid::new_v4()))
    }

    /// Returns the stable serialized identifier.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for ReviewLoadRequestId {
    fn default() -> Self {
        Self::new()
    }
}

/// Identifies one attempt to prepare or post a pull request's review.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct PullRequestPostId(String);

impl PullRequestPostId {
    /// Creates a distinct identifier, so a late answer to an abandoned attempt is recognized.
    pub fn new() -> Self {
        Self(format!("pr-post:{}", Uuid::new_v4()))
    }

    /// Returns the stable serialized identifier.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for PullRequestPostId {
    fn default() -> Self {
        Self::new()
    }
}

/// Identifies one attempt to draft or open a branch's pull request.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct PullRequestOpenId(String);

impl PullRequestOpenId {
    /// Creates a distinct identifier, so a late answer to an abandoned attempt is recognized.
    pub fn new() -> Self {
        Self(format!("pr-open:{}", Uuid::new_v4()))
    }

    /// Returns the stable serialized identifier.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for PullRequestOpenId {
    fn default() -> Self {
        Self::new()
    }
}

/// Identifies one attempt to prepare or send answers to a pull request's review threads.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct ThreadAnswersId(String);

impl ThreadAnswersId {
    /// Creates a distinct identifier, so a late answer to an abandoned attempt is recognized.
    pub fn new() -> Self {
        Self(format!("thread-answers:{}", Uuid::new_v4()))
    }

    /// Returns the stable serialized identifier.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for ThreadAnswersId {
    fn default() -> Self {
        Self::new()
    }
}

/// Identifies one asynchronous feedback-session persistence request.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct ReviewSessionSaveRequestId(String);

impl ReviewSessionSaveRequestId {
    /// Creates a distinct request identifier for stale-save rejection.
    pub fn new() -> Self {
        Self(format!("review-save:{}", Uuid::new_v4()))
    }

    /// Returns the stable serialized identifier.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for ReviewSessionSaveRequestId {
    fn default() -> Self {
        Self::new()
    }
}

fn digest(domain: &str, parts: &[&[u8]]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(domain.as_bytes());
    hasher.update([0]);
    for part in parts {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part);
    }
    base16ct::lower::encode_string(&hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn content_addressed_ids_keep_the_hex_spelling_they_were_persisted_with() {
        // Literals computed outside this crate. A digest that round-trips against itself proves
        // nothing: these ids name directories under `.luxe/` and are recorded in saved reviews, so
        // the encoder has to keep producing lowercase, unpadded, full-width hex forever.
        assert_eq!(
            ReviewRevisionId::from_canonical_bytes(b"canonical diff").as_str(),
            "revision:bf1adce2be5ddf851bd1a875aee91e7d09fe1d089c191a12d3b4edddb2ac13e5"
        );
        assert_eq!(
            TurnBaselineId::from_session_turn("session-a", "turn-1").as_str(),
            "turn-baseline:13dde931da3faf8b6538712e57c941aa11fec4f8ae922acf9d6cc6f3a0c0c23f"
        );
    }

    #[test]
    fn revision_id_is_deterministic_and_content_addressed() {
        let first = ReviewRevisionId::from_canonical_bytes(b"canonical diff");
        assert_eq!(first, ReviewRevisionId::from_canonical_bytes(b"canonical diff"));
        assert_ne!(first, ReviewRevisionId::from_canonical_bytes(b"changed diff"));
    }

    #[test]
    fn agent_turn_id_is_opaque_and_distinct() {
        let first = AgentTurnId::new();
        let second = AgentTurnId::new();
        assert!(first.as_str().starts_with("turn:"));
        assert_ne!(first, second);
    }

    #[test]
    fn feedback_ids_are_opaque_and_distinct() {
        let first_finding = FindingId::new();
        let second_finding = FindingId::new();
        let first_submission = SubmissionId::new();
        let second_submission = SubmissionId::new();

        assert!(first_finding.as_str().starts_with("finding:"));
        assert_ne!(first_finding, second_finding);
        assert!(first_submission.as_str().starts_with("submission:"));
        assert_ne!(first_submission, second_submission);
    }

    #[test]
    fn turn_snapshot_ids_are_stable_and_distinguish_turn_boundaries() {
        let before = TurnBaselineId::from_session_turn("session-a", "turn-1");
        let after = TurnBaselineId::post_turn_from_session_turn("session-a", "turn-1");
        assert_eq!(before, TurnBaselineId::from_session_turn("session-a", "turn-1"));
        assert_eq!(after, TurnBaselineId::post_turn_from_session_turn("session-a", "turn-1"));
        assert_ne!(before, after);
        assert_ne!(before, TurnBaselineId::from_session_turn("session-b", "turn-1"));
        assert_eq!(before.cache_key().len(), 64);
        assert_eq!(after.cache_key().len(), 64);
    }

    #[test]
    fn hunk_id_changes_only_when_its_own_semantic_input_changes() {
        let file = FileId::from_path_lineage(None, Some(Path::new("src/lib.rs")));
        let first = HunkId::from_content(&file, "fn run", "+    render();");
        assert_eq!(first, HunkId::from_content(&file, "fn run", "+    render();"));
        assert_ne!(first, HunkId::from_content(&file, "fn run", "+    review();"));
    }

    #[test]
    fn file_lineage_is_not_derived_from_vector_order() {
        let file =
            FileId::from_path_lineage(Some(Path::new("src/old.rs")), Some(Path::new("src/new.rs")));
        assert_eq!(
            file,
            FileId::from_path_lineage(Some(Path::new("src/old.rs")), Some(Path::new("src/new.rs")))
        );
        assert_ne!(file, FileId::from_path_lineage(None, Some(Path::new("src/new.rs"))));
    }
}
