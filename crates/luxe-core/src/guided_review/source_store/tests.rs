use super::*;
use tempfile::tempdir;

#[test]
fn captured_artifacts_are_content_addressed_and_project_local() {
    let artifact = ReviewSourceArtifact::capture(b"canonical source".to_vec());
    let root = Path::new("/workspace/project");

    assert_eq!(artifact.manifest.schema_version, SOURCE_ARTIFACT_SCHEMA_VERSION);
    assert_eq!(artifact.manifest.source_len, 16);
    assert_eq!(
        artifact.cache_dir(root),
        root.join(SOURCE_CACHE_RELATIVE_DIR).join(artifact.manifest.revision_id.cache_key())
    );
}

#[test]
fn retention_never_evicts_protected_or_untimestamped_artifacts() {
    let artifact = ReviewSourceArtifact::capture_at(b"canonical source".to_vec(), Some(100));
    let policy = SourceRetentionPolicy { max_age_ms: Some(10), max_total_bytes: 1 };
    let protected = HashSet::from([artifact.manifest.revision_id.clone()]);

    assert!(!policy.may_evict(&artifact.manifest, 200, &protected));
    assert!(policy.may_evict(&artifact.manifest, 200, &HashSet::new()));

    let legacy = ReviewSourceArtifact::capture_at(b"legacy source".to_vec(), None);
    assert!(!policy.may_evict(&legacy.manifest, 200, &HashSet::new()));
}

#[test]
fn prune_expired_keeps_protected_and_untrusted_artifacts() {
    let project = tempdir().unwrap();
    let expired = ReviewSourceArtifact::capture_at(b"expired".to_vec(), Some(100));
    let protected = ReviewSourceArtifact::capture_at(b"protected".to_vec(), Some(100));
    let corrupt = ReviewSourceArtifact::capture_at(b"corrupt".to_vec(), Some(100));
    let expired_path = expired.persist(project.path()).unwrap();
    let protected_path = protected.persist(project.path()).unwrap();
    let corrupt_path = corrupt.persist(project.path()).unwrap();
    let corrupt_manifest = corrupt_path.join(SOURCE_MANIFEST_FILE);
    let mut manifest: SourceArtifactManifest =
        serde_json::from_slice(&fs::read(&corrupt_manifest).unwrap()).unwrap();
    manifest.schema_version = SOURCE_ARTIFACT_SCHEMA_VERSION + 1;
    fs::write(&corrupt_manifest, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let policy = SourceRetentionPolicy { max_age_ms: Some(10), max_total_bytes: u64::MAX };
    let protected_revisions = HashSet::from([protected.manifest.revision_id.clone()]);

    let removed =
        ReviewSourceArtifact::prune_expired(project.path(), &policy, 200, &protected_revisions)
            .unwrap();

    assert_eq!(removed, vec![expired.manifest.revision_id]);
    assert!(!expired_path.exists());
    assert!(protected_path.exists());
    assert!(corrupt_path.exists());
}

#[test]
fn byte_budget_evicts_the_oldest_verified_unprotected_source_first() {
    let project = tempdir().unwrap();
    let oldest = ReviewSourceArtifact::capture_at(b"old-one".to_vec(), Some(10));
    let newest = ReviewSourceArtifact::capture_at(b"new-two".to_vec(), Some(20));
    let protected = ReviewSourceArtifact::capture_at(b"guard".to_vec(), Some(1));
    let legacy = ReviewSourceArtifact::capture_at(b"legacy".to_vec(), None);
    let corrupt = ReviewSourceArtifact::capture_at(b"corrupt".to_vec(), Some(2));
    let oldest_path = oldest.persist(project.path()).unwrap();
    let newest_path = newest.persist(project.path()).unwrap();
    let protected_path = protected.persist(project.path()).unwrap();
    let legacy_path = legacy.persist(project.path()).unwrap();
    let corrupt_path = corrupt.persist(project.path()).unwrap();
    let corrupt_manifest = corrupt_path.join(SOURCE_MANIFEST_FILE);
    let mut manifest: SourceArtifactManifest =
        serde_json::from_slice(&fs::read(&corrupt_manifest).unwrap()).unwrap();
    manifest.schema_version = SOURCE_ARTIFACT_SCHEMA_VERSION + 1;
    fs::write(&corrupt_manifest, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let policy = SourceRetentionPolicy { max_age_ms: None, max_total_bytes: 18 };
    let protected_revisions = HashSet::from([protected.manifest.revision_id.clone()]);

    let removed =
        ReviewSourceArtifact::prune_to_byte_budget(project.path(), &policy, &protected_revisions)
            .unwrap();

    assert_eq!(removed, vec![oldest.manifest.revision_id]);
    assert!(!oldest_path.exists());
    assert!(newest_path.exists());
    assert!(protected_path.exists());
    assert!(legacy_path.exists());
    assert!(corrupt_path.exists(), "cleanup must preserve unknown-schema artifacts");
}

#[test]
fn persist_rejects_a_manifest_that_does_not_describe_its_payload() {
    let project = tempdir().unwrap();
    let mut artifact = ReviewSourceArtifact::capture(b"canonical source".to_vec());
    artifact.manifest.source_len += 1;

    assert!(matches!(
        artifact.persist(project.path()),
        Err(SourceStoreError::Integrity {
            reason: "manifest byte length does not match payload",
            ..
        })
    ));
    assert!(!project.path().join(SOURCE_CACHE_RELATIVE_DIR).exists());
}

#[test]
fn load_rejects_deserialized_revision_ids_that_cannot_name_cache_directories() {
    let project = tempdir().unwrap();
    let revision_id: ReviewRevisionId = serde_json::from_str("\"revision:../../outside\"").unwrap();

    assert!(matches!(
        ReviewSourceArtifact::load(project.path(), &revision_id),
        Err(SourceStoreError::InvalidRevisionId)
    ));
}

#[test]
fn persist_writes_a_private_complete_artifact_once() {
    let project = tempdir().unwrap();
    let artifact = ReviewSourceArtifact::capture(b"canonical source".to_vec());
    let path = artifact.persist(project.path()).unwrap();

    assert_eq!(fs::read(path.join(SOURCE_PAYLOAD_FILE)).unwrap(), artifact.canonical_bytes);
    let manifest: SourceArtifactManifest =
        serde_json::from_slice(&fs::read(path.join(SOURCE_MANIFEST_FILE)).unwrap()).unwrap();
    assert_eq!(manifest, artifact.manifest);
    assert_eq!(artifact.persist(project.path()).unwrap(), path);
    assert_eq!(
        ReviewSourceArtifact::load(project.path(), &artifact.manifest.revision_id).unwrap(),
        artifact
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(
            fs::metadata(path.join(SOURCE_PAYLOAD_FILE)).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(path.join(SOURCE_MANIFEST_FILE)).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn reload_after_workspace_mutation_uses_the_pinned_source() {
    let project = tempdir().unwrap();
    let worktree_file = project.path().join("mutable.rs");
    std::fs::write(&worktree_file, "before\n").unwrap();
    let artifact = ReviewSourceArtifact::capture(
        b"diff --git a/mutable.rs b/mutable.rs\n-before\n+after\n".to_vec(),
    );
    artifact.persist(project.path()).unwrap();

    std::fs::write(&worktree_file, "changed again\n").unwrap();

    let reloaded =
        ReviewSourceArtifact::load(project.path(), &artifact.manifest.revision_id).unwrap();
    assert_eq!(reloaded.canonical_bytes, artifact.canonical_bytes);
    assert_eq!(std::fs::read_to_string(worktree_file).unwrap(), "changed again\n");
}

#[test]
fn load_rejects_unknown_schema_without_touching_the_artifact() {
    let project = tempdir().unwrap();
    let artifact = ReviewSourceArtifact::capture(b"canonical source".to_vec());
    let path = artifact.persist(project.path()).unwrap();
    let manifest_path = path.join(SOURCE_MANIFEST_FILE);
    let mut manifest: SourceArtifactManifest =
        serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    manifest.schema_version = SOURCE_ARTIFACT_SCHEMA_VERSION + 1;
    fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();

    assert!(matches!(
        ReviewSourceArtifact::load(project.path(), &artifact.manifest.revision_id),
        Err(SourceStoreError::UnsupportedSchema { .. })
    ));
    assert!(manifest_path.exists());
}

#[cfg(unix)]
#[test]
fn load_rejects_symlinked_artifact_payloads() {
    use std::os::unix::fs::symlink;

    let project = tempdir().unwrap();
    let artifact = ReviewSourceArtifact::capture(b"canonical source".to_vec());
    let path = artifact.persist(project.path()).unwrap();
    let payload_path = path.join(SOURCE_PAYLOAD_FILE);
    fs::remove_file(&payload_path).unwrap();
    symlink("/dev/null", &payload_path).unwrap();

    assert!(matches!(
        ReviewSourceArtifact::load(project.path(), &artifact.manifest.revision_id),
        Err(SourceStoreError::Integrity { reason: "artifact file is not a regular file", .. })
    ));
}

#[cfg(unix)]
#[test]
fn load_rejects_symlinked_artifact_directories() {
    use std::os::unix::fs::symlink;

    let project = tempdir().unwrap();
    let artifact = ReviewSourceArtifact::capture(b"canonical source".to_vec());
    let path = artifact.persist(project.path()).unwrap();
    let relocated = project.path().join("relocated-artifact");
    fs::rename(&path, &relocated).unwrap();
    symlink(&relocated, &path).unwrap();

    assert!(matches!(
        ReviewSourceArtifact::load(project.path(), &artifact.manifest.revision_id),
        Err(SourceStoreError::Integrity {
            reason: "artifact directory is not a real directory",
            ..
        })
    ));
}

#[test]
fn load_and_deduplication_reject_a_tampered_payload() {
    let project = tempdir().unwrap();
    let artifact = ReviewSourceArtifact::capture(b"canonical source".to_vec());
    let path = artifact.persist(project.path()).unwrap();
    let payload_path = path.join(SOURCE_PAYLOAD_FILE);
    let tampered = b"canonical sourCe";
    fs::write(&payload_path, tampered).unwrap();

    assert!(matches!(
        ReviewSourceArtifact::load(project.path(), &artifact.manifest.revision_id),
        Err(SourceStoreError::Integrity { .. })
    ));
    assert!(matches!(artifact.persist(project.path()), Err(SourceStoreError::Integrity { .. })));
    assert_eq!(fs::read(payload_path).unwrap(), tampered);
}
