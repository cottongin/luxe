use super::*;
use crate::guided_review::{FileId, ReviewBase, ReviewItemRef};

fn sample_record() -> ActiveReviewRecord {
    let revision_id = ReviewRevisionId::from_canonical_bytes(b"diff --git a/a b/a\n");
    ActiveReviewRecord {
        schema_version: ACTIVE_REVIEW_RECORD_SCHEMA_VERSION,
        review_id: ReviewId::new(),
        revision_id: revision_id.clone(),
        target: ReviewTarget::WorkingTree,
        source: ReviewSource {
            base: ReviewBase::EmptyTree,
            content_digest: revision_id,
            includes_staged: false,
            includes_unstaged: true,
            included_untracked_files: 0,
            captured_at_ms: 42,
            uncovered_paths: Vec::new(),
        },
        context: ReviewContext {
            title: "Review".into(),
            goal: None,
            phase_index: None,
            phase_title: None,
            planning_excerpt: None,
            acceptance_excerpt: None,
            source_intent: None,
            source_commits: 0,
            pull_request: None,
        },
        chapters: Vec::new(),
        checks: CheckSummary::default(),
        generated_at_ms: 42,
        brief: None,
    }
}

#[test]
fn record_round_trips_without_source_lines() {
    let record = sample_record();
    let serialized = serde_json::to_string(&record);
    assert!(serialized.is_ok());
    let serialized = serialized.unwrap_or_default();
    assert!(!serialized.contains("source.diff"));
    let restored: Result<ActiveReviewRecord, _> = serde_json::from_str(&serialized);
    assert!(restored.is_ok());
    let restored = restored.unwrap_or_else(|_| record.clone());
    assert_eq!(restored, record);
    assert_eq!(restored.validate(), Ok(()));
}

#[test]
fn store_round_trips_a_private_record() {
    let root = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
    let store = ActiveReviewRecordStore::new(root.path());
    let record = sample_record();

    assert_eq!(store.load().unwrap_or_default(), None);
    assert!(store.save(&record).is_ok());
    assert_eq!(store.load().unwrap_or_default(), Some(record));
}

#[test]
fn store_preserves_an_unsupported_schema() {
    let root = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
    let store = ActiveReviewRecordStore::new(root.path());
    assert!(store.save(&sample_record()).is_ok());
    let path = root.path().join(ACTIVE_REVIEW_RECORD_RELATIVE_PATH);
    let original = fs::read(&path).unwrap_or_default();
    let unsupported = String::from_utf8_lossy(&original)
        .replacen("\"schema_version\":1", "\"schema_version\":99", 1)
        .into_bytes();
    assert_ne!(unsupported, original);
    assert!(fs::write(&path, &unsupported).is_ok());

    let result = store.save(&sample_record());
    assert!(matches!(
        result,
        Err(ActiveReviewRecordStoreError::Record(ActiveReviewRecordError::UnsupportedSchema {
            schema_version: 99
        }))
    ));
    assert_eq!(fs::read(&path).unwrap_or_default(), unsupported);
}

#[test]
fn store_preserves_malformed_metadata_instead_of_replacing_it() {
    let root = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
    let store = ActiveReviewRecordStore::new(root.path());
    assert!(store.save(&sample_record()).is_ok());
    let path = root.path().join(ACTIVE_REVIEW_RECORD_RELATIVE_PATH);
    let malformed = b"{not valid review metadata";
    assert!(fs::write(&path, malformed).is_ok());

    assert!(matches!(
        store.save(&sample_record()),
        Err(ActiveReviewRecordStoreError::Deserialize { .. })
    ));
    assert_eq!(fs::read(&path).unwrap_or_default(), malformed);
}

#[test]
fn restore_fails_when_the_pinned_artifact_is_missing() {
    let root = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
    let record = sample_record();

    assert!(matches!(
        record.restore(root.path(), &crate::guided_review::ReviewExclusions::default()),
        Err(ActiveReviewRestoreError::SourceStore(SourceStoreError::Io {
            operation: "inspect",
            ..
        }))
    ));
}

#[test]
fn restore_uses_the_pinned_artifact_after_workspace_mutation_and_rejects_tampering() {
    let root = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
    let source_bytes = b"diff --git a/mutable.rs b/mutable.rs\nindex 1111111..2222222 100644\n--- a/mutable.rs\n+++ b/mutable.rs\n@@ -1 +1 @@\n-before\n+after\n".to_vec();
    let artifact = ReviewSourceArtifact::capture_at(source_bytes, Some(42));
    assert!(artifact.persist(root.path()).is_ok());
    let source = ReviewSource {
        base: ReviewBase::EmptyTree,
        content_digest: artifact.manifest.revision_id.clone(),
        includes_staged: false,
        includes_unstaged: true,
        included_untracked_files: 0,
        captured_at_ms: 42,
        uncovered_paths: Vec::new(),
    };
    let review = GuidedReview::from_pinned_artifact(
        ReviewId::new(),
        ReviewTarget::WorkingTree,
        source,
        ReviewContext {
            title: "Pinned review".into(),
            goal: None,
            phase_index: None,
            phase_title: None,
            planning_excerpt: None,
            acceptance_excerpt: None,
            source_intent: None,
            source_commits: 0,
            pull_request: None,
        },
        &artifact,
        &crate::guided_review::ReviewExclusions::default(),
        42,
    );
    assert!(review.is_ok());
    let review = review.unwrap_or_else(|error| panic!("review build failed: {error}"));
    let record = ActiveReviewRecord::from_review(&review);
    assert!(fs::write(root.path().join("mutable.rs"), "changed again\n").is_ok());

    let restored = record.restore(root.path(), &crate::guided_review::ReviewExclusions::default());
    assert!(restored.is_ok());
    let restored = restored.unwrap_or_else(|error| panic!("restore failed: {error}"));
    assert_eq!(restored.files, review.files);
    assert_eq!(restored.revision_id, review.revision_id);
    assert_eq!(
        fs::read_to_string(root.path().join("mutable.rs")).unwrap_or_default(),
        "changed again\n"
    );

    assert!(fs::write(
        artifact.cache_dir(root.path()).join(super::super::source_store::SOURCE_PAYLOAD_FILE),
        b"tampered pinned source",
    )
    .is_ok());
    assert!(matches!(
        record.restore(root.path(), &crate::guided_review::ReviewExclusions::default()),
        Err(ActiveReviewRestoreError::SourceStore(SourceStoreError::Integrity { .. }))
    ));
}

/// A record whose chapters name items its source no longer has reopens with structural chapters
/// rather than not at all.
///
/// The break this catches: a binary addition once read as a modification, under another file id;
/// re-read correctly, the saved chapters named an item the review did not have, and the open
/// review was dropped on restart.
#[test]
fn chapters_naming_items_the_source_no_longer_has_give_way_to_structural_ones() {
    let root = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
    let source_bytes =
        b"diff --git a/logo.bin b/logo.bin\nnew file mode 100644\nBinary files /dev/null and b/logo.bin differ\n".to_vec();
    let artifact = ReviewSourceArtifact::capture_at(source_bytes, Some(42));
    assert!(artifact.persist(root.path()).is_ok());
    let source = ReviewSource {
        base: ReviewBase::EmptyTree,
        content_digest: artifact.manifest.revision_id.clone(),
        includes_staged: false,
        includes_unstaged: true,
        included_untracked_files: 0,
        captured_at_ms: 42,
        uncovered_paths: Vec::new(),
    };
    let context = crate::guided_review::ReviewContextDraft::default().build();
    let exclusions = crate::guided_review::ReviewExclusions::default();
    let review = GuidedReview::from_pinned_artifact(
        ReviewId::new(),
        ReviewTarget::WorkingTree,
        source,
        context,
        &artifact,
        &exclusions,
        42,
    )
    .unwrap_or_else(|error| panic!("review build failed: {error}"));
    let mut record = ActiveReviewRecord::from_review(&review);
    let stale = FileId::from_path_lineage(Some(Path::new("logo.bin")), Some(Path::new("logo.bin")));
    record.chapters[0].items = vec![ReviewItemRef::File(stale)];
    record.brief = Some(crate::guided_review::ReviewBrief {
        intent: "Ship the logo".into(),
        account: vec!["Adds logo.bin".into()],
    });

    let restored = record
        .restore(root.path(), &exclusions)
        .unwrap_or_else(|error| panic!("restore failed: {error}"));
    assert_eq!(restored.chapters, review.chapters, "the structural chapters the source has");
    assert_eq!(restored.brief, record.brief, "the brief names no items, so it survives");
}
