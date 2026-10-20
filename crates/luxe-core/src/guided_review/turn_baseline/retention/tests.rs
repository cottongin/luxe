use super::*;
use crate::guided_review::{ReviewBase, TurnBaselineArtifact};

fn baseline(turn: &str, files: &[(&str, &[u8])]) -> TurnBaselineArtifact {
    let files = files.iter().map(|(path, bytes)| (PathBuf::from(path), bytes.to_vec())).collect();
    let id = TurnBaselineId::from_session_turn("session", turn);
    TurnBaselineArtifact::capture(id, ReviewBase::EmptyTree, 1, false, false, 0, files)
        .expect("a fixture of normal paths is a valid baseline")
}

/// Where `artifact`'s copy of `bytes` sits once persisted under `project`.
#[cfg(unix)]
fn payload(project: &Path, artifact: &TurnBaselineArtifact, bytes: &[u8]) -> PathBuf {
    let digest = super::super::payload_digest(bytes);
    let root = project.join(TURN_BASELINE_CACHE_RELATIVE_DIR);
    root.join(artifact.manifest.id.cache_key()).join(TURN_BASELINE_PAYLOAD_DIR).join(digest)
}

fn persisted(project: &Path, turn: &str, files: &[(&str, &[u8])]) -> TurnBaselineArtifact {
    let artifact = baseline(turn, files);
    artifact.persist(project).expect("a valid baseline persists");
    artifact
}

fn baseline_dirs(project: &Path) -> Vec<String> {
    let root = project.join(TURN_BASELINE_CACHE_RELATIVE_DIR);
    let mut names: Vec<String> = fs::read_dir(root)
        .expect("the cache exists")
        .map(|entry| entry.expect("listable").file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[cfg(unix)]
fn inode(path: &Path) -> u64 {
    std::os::unix::fs::MetadataExt::ino(&fs::symlink_metadata(path).expect("the payload exists"))
}

// The break this catches: every capture writing and syncing every file again, which cost a
// project of 4,000 files about ten seconds a turn and a full copy of it on disk each time
#[cfg(unix)]
#[test]
fn a_baseline_links_a_file_the_last_one_already_holds() {
    let project = tempfile::tempdir().expect("a temporary project");
    let first = persisted(project.path(), "1", &[("src/main.rs", b"fn main() {}\n")]);
    let second_files: &[(&str, &[u8])] = &[("src/main.rs", b"fn main() {}\n"), ("new.rs", b"new")];
    let second = persisted(project.path(), "2", second_files);
    let main = b"fn main() {}\n".as_slice();
    assert_eq!(
        inode(&payload(project.path(), &first, main)),
        inode(&payload(project.path(), &second, main))
    );
    let loaded = TurnBaselineArtifact::load(project.path(), &second.manifest.id);
    assert_eq!(loaded.expect("the linked baseline loads"), second);
}

// The break this catches: linking whatever file sits under the digest's name, so a truncated
// copy carries into every later baseline and none of them load
#[cfg(unix)]
#[test]
fn an_earlier_copy_of_the_wrong_size_is_written_fresh() {
    let project = tempfile::tempdir().expect("a temporary project");
    let first = persisted(project.path(), "1", &[("a.rs", b"whole")]);
    let first_copy = payload(project.path(), &first, b"whole");
    fs::write(&first_copy, b"cut").expect("the fixture truncates the earlier copy");
    let second = persisted(project.path(), "2", &[("a.rs", b"whole")]);
    assert_ne!(inode(&first_copy), inode(&payload(project.path(), &second, b"whole")));
    assert!(TurnBaselineArtifact::load(project.path(), &second.manifest.id).is_ok());
}

// The break this catches: linking a symlink planted under the digest's name, which hard-links
// the symlink itself into the new baseline. Its target is as long as the payload, since a
// symlink's own size is its target's length, so the size check alone can't turn it away
#[cfg(unix)]
#[test]
fn an_earlier_copy_that_is_a_symlink_is_written_fresh() {
    let project = tempfile::tempdir().expect("a temporary project");
    let first = persisted(project.path(), "1", &[("a.rs", b"whole")]);
    let first_copy = payload(project.path(), &first, b"whole");
    fs::remove_file(&first_copy).expect("the fixture replaces the earlier copy");
    std::os::unix::fs::symlink("abcde", &first_copy).expect("the fixture plants a symlink");
    let second = persisted(project.path(), "2", &[("a.rs", b"whole")]);
    let second_copy = fs::symlink_metadata(payload(project.path(), &second, b"whole"));
    assert!(second_copy.expect("the payload exists").is_file());
}

// The break this catches: linking from the oldest baseline rather than the newest, which holds
// least of what the workspace has now
#[cfg(unix)]
#[test]
fn links_come_from_the_newest_baseline() {
    let project = tempfile::tempdir().expect("a temporary project");
    let older = persisted(project.path(), "1", &[("a.rs", b"a")]);
    let newer = persisted(project.path(), "2", &[("a.rs", b"b")]);
    let root = project.path().join(TURN_BASELINE_CACHE_RELATIVE_DIR);
    let older_dir = root.join(older.manifest.id.cache_key());
    let hours_ago = SystemTime::now() - Duration::from_secs(2 * 60 * 60);
    fs::File::open(&older_dir).and_then(|dir| dir.set_modified(hours_ago)).expect("backdated");
    let newest = newest_payload_dir(&root).expect("there are baselines");
    assert_eq!(newest, root.join(newer.manifest.id.cache_key()).join(TURN_BASELINE_PAYLOAD_DIR));
}

// The break this catches: pruning that ignores what the last-turn record names, losing the one
// review the cache exists for, or that deletes nothing at all
#[test]
fn pruning_deletes_old_baselines_but_the_ones_kept() {
    let project = tempfile::tempdir().expect("a temporary project");
    let before = persisted(project.path(), "1", &[("a.rs", b"a")]);
    let after = persisted(project.path(), "2", &[("a.rs", b"b")]);
    persisted(project.path(), "3", &[("a.rs", b"c")]);
    let keep = [&before.manifest.id, &after.manifest.id];
    assert_eq!(prune_turn_baselines(project.path(), &keep, Duration::ZERO), 1);
    let mut kept = vec![before.manifest.id.cache_key(), after.manifest.id.cache_key()];
    kept.sort();
    assert_eq!(baseline_dirs(project.path()), kept);
}

// The break this catches: pruning by the record alone, which deletes the baseline of a turn
// another session in the project is still running
#[test]
fn pruning_spares_a_baseline_younger_than_the_age() {
    let project = tempfile::tempdir().expect("a temporary project");
    persisted(project.path(), "1", &[("a.rs", b"a")]);
    assert_eq!(prune_turn_baselines(project.path(), &[], UNREFERENCED_BASELINE_AGE), 0);
    assert_eq!(baseline_dirs(project.path()).len(), 1);
}

// The break this catches: staging a crashed capture left behind piling up forever, since no
// record ever names it
#[test]
fn pruning_deletes_abandoned_staging() {
    let project = tempfile::tempdir().expect("a temporary project");
    let staging = project.path().join(TURN_BASELINE_CACHE_RELATIVE_DIR).join(".staging-crashed");
    fs::create_dir_all(staging.join(TURN_BASELINE_PAYLOAD_DIR)).expect("the fixture stages");
    assert_eq!(prune_turn_baselines(project.path(), &[], Duration::ZERO), 1);
    assert!(!staging.exists());
}

// The break this catches: a project that has never captured a baseline failing its first turn
#[test]
fn pruning_a_project_with_no_cache_does_nothing() {
    let project = tempfile::tempdir().expect("a temporary project");
    assert_eq!(prune_turn_baselines(project.path(), &[], Duration::ZERO), 0);
}
