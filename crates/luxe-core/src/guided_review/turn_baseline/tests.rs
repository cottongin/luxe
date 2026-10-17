use super::*;

fn digest() -> String {
    "a".repeat(64)
}

#[test]
fn payload_digest_keeps_the_hex_spelling_stored_manifests_carry() {
    // Literal computed outside this crate: a snapshot on disk is validated against the digest
    // its manifest recorded, so a re-spelled digest invalidates every retained baseline.
    assert_eq!(
        payload_digest(b"baseline payload"),
        "4b2d97ea65a085b392c984fb76e79c1f746743df45c66354e110a1d2bd77d4c7"
    );
}

#[test]
fn manifest_rejects_duplicate_or_unsafe_paths() {
    let id = TurnBaselineId::from_session_turn("session", "turn");
    let base = ReviewBase::EmptyTree;
    let file = TurnBaselineFile {
        path: PathBuf::from("src/main.rs"),
        content_digest: digest(),
        byte_len: 12,
    };
    assert!(TurnBaselineManifest::new(
        id.clone(),
        base.clone(),
        1,
        false,
        false,
        0,
        vec![file.clone(), file]
    )
    .is_err());
    let unsafe_file = TurnBaselineFile {
        path: PathBuf::from("../secret"),
        content_digest: digest(),
        byte_len: 1,
    };
    assert!(TurnBaselineManifest::new(id, base, 1, false, false, 0, vec![unsafe_file]).is_err());
}

#[test]
fn load_rejects_deserialized_baseline_ids_that_cannot_name_cache_directories() {
    let Ok(root) = tempfile::tempdir() else {
        panic!("temporary project directory must be available");
    };
    let Ok(id) = serde_json::from_str("\"turn-baseline:../../outside\"") else {
        panic!("test fixture must deserialize into a transparent baseline ID");
    };

    assert!(matches!(
        TurnBaselineArtifact::load(root.path(), &id),
        Err(TurnBaselineStoreError::InvalidBaselineId)
    ));
}

#[test]
fn persist_rejects_manifest_payload_mismatches_before_creating_a_cache() {
    let Ok(root) = tempfile::tempdir() else {
        panic!("temporary project directory must be available");
    };
    let Ok(mut artifact) = TurnBaselineArtifact::capture(
        TurnBaselineId::from_session_turn("session", "turn"),
        ReviewBase::EmptyTree,
        1,
        false,
        false,
        0,
        vec![(PathBuf::from("src/main.rs"), b"fn main() {}\n".to_vec())],
    ) else {
        panic!("normal baseline fixture must be valid");
    };
    artifact.manifest.files[0].byte_len = 0;

    assert!(matches!(
        artifact.persist(root.path()),
        Err(TurnBaselineStoreError::Integrity { reason: "payload metadata differs", .. })
    ));
    assert!(!root.path().join(TURN_BASELINE_CACHE_RELATIVE_DIR).exists());
}

#[test]
fn artifact_persists_and_reloads_private_payloads() {
    let Ok(root) = tempfile::tempdir() else {
        panic!("temporary project directory must be available");
    };
    let Ok(artifact) = TurnBaselineArtifact::capture(
        TurnBaselineId::from_session_turn("session", "turn"),
        ReviewBase::EmptyTree,
        1,
        false,
        false,
        0,
        vec![(PathBuf::from("src/main.rs"), b"fn main() {}\n".to_vec())],
    ) else {
        panic!("normal baseline fixture must be valid");
    };
    let Ok(target) = artifact.persist(root.path()) else {
        panic!("complete baseline fixture must persist");
    };
    // Only the permission assertion below reads it, and that is unix-only.
    let _ = &target;
    let Ok(loaded) = TurnBaselineArtifact::load(root.path(), &artifact.manifest.id) else {
        panic!("persisted complete baseline fixture must reload");
    };
    assert_eq!(loaded, artifact);
    assert_eq!(loaded.file(Path::new("src/main.rs")), Some(b"fn main() {}\n".as_slice()));
    assert_eq!(loaded.files().count(), 1);
    assert_eq!(loaded.revision_id(), artifact.revision_id());
    let materialized = root.path().join("comparison-snapshot");
    assert!(loaded.materialize_into(&materialized).is_ok());
    assert!(matches!(
        fs::read(materialized.join("src/main.rs")).as_deref(),
        Ok(bytes) if bytes == b"fn main() {}\n"
    ));
    assert!(matches!(
        loaded.materialize_into(&materialized),
        Err(TurnBaselineStoreError::Integrity { .. })
    ));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let Ok(metadata) = fs::metadata(target) else {
            panic!("persisted baseline directory must have metadata");
        };
        assert_eq!(metadata.permissions().mode() & 0o777, 0o700);
    }
}

#[test]
fn manifest_accepts_complete_normal_entries() {
    let manifest = TurnBaselineManifest::new(
        TurnBaselineId::from_session_turn("session", "turn"),
        ReviewBase::EmptyTree,
        1,
        false,
        false,
        0,
        vec![TurnBaselineFile {
            path: PathBuf::from("src/main.rs"),
            content_digest: digest(),
            byte_len: 12,
        }],
    )
    .unwrap();
    assert_eq!(manifest.files.len(), 1);
}

/// The exclusion shape a retained manifest carries, pinned: a rename would load as no exclusions,
/// and the comparison would then read every excluded file as added or deleted.
#[test]
fn exclusions_read_from_their_saved_spelling() {
    let saved = format!(
        r#"{{"schema_version":{TURN_BASELINE_SCHEMA_VERSION},"id":{},"base":{{"kind":"empty_tree"}},"captured_at_ms":1,"includes_staged":false,"includes_unstaged":false,"included_untracked_files":0,"files":[],"excluded":[{{"path":"archive.md","byte_len":25,"modified_ms":7}},{{"path":"pipe"}}]}}"#,
        serde_json::to_string(&TurnBaselineId::from_session_turn("session", "turn")).unwrap()
    );
    let manifest: TurnBaselineManifest = serde_json::from_str(&saved).unwrap();
    let excluded = &manifest.excluded;
    assert_eq!(excluded.len(), 2);
    assert_eq!(excluded[0].path, PathBuf::from("archive.md"));
    assert_eq!((excluded[0].byte_len, excluded[0].modified_ms), (Some(25), Some(7)));
    assert_eq!((excluded[1].byte_len, excluded[1].modified_ms), (None, None));
}

/// Only the same path at the same size and age reads as untouched; anything unknown is a change.
#[test]
fn an_exclusion_is_unchanged_only_when_everything_it_knows_matches() {
    let at = |path: &str, byte_len, modified_ms| TurnBaselineExclusion {
        path: PathBuf::from(path),
        byte_len,
        modified_ms,
    };
    let kept = at("archive.md", Some(25), Some(7));
    assert!(kept.unchanged_in(&kept.clone()));
    assert!(!kept.unchanged_in(&at("archive.md", Some(26), Some(7))), "a new size");
    assert!(!kept.unchanged_in(&at("archive.md", Some(25), Some(8))), "same size, edited");
    assert!(!kept.unchanged_in(&at("other.md", Some(25), Some(7))), "another file");
    let no_age = at("archive.md", Some(25), None);
    assert!(!no_age.unchanged_in(&no_age.clone()), "an unknown age never matches");
    let no_size = at("archive.md", None, Some(7));
    assert!(!no_size.unchanged_in(&no_size.clone()), "an unknown size never matches");
}

/// A manifest with nothing excluded keeps the shape every retained baseline already has.
#[test]
fn a_manifest_with_no_exclusions_writes_no_key() {
    let id = TurnBaselineId::from_session_turn("session", "turn");
    let manifest =
        TurnBaselineManifest::new(id, ReviewBase::EmptyTree, 1, false, false, 0, vec![]).unwrap();
    let json = serde_json::to_value(&manifest).unwrap();
    let keys: Vec<&String> = json.as_object().unwrap().keys().collect();
    assert_eq!(
        keys,
        [
            "schema_version",
            "id",
            "base",
            "captured_at_ms",
            "includes_staged",
            "includes_unstaged",
            "included_untracked_files",
            "files"
        ]
    );
}
