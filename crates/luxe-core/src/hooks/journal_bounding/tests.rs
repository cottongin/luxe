use super::*;
use tempfile::tempdir;

#[tokio::test]
async fn test_journal_bounding_hook_not_exceeding() {
    let dir = tempdir().unwrap();
    let journal_path = dir.path().join("PROGRESS.md");
    let archive_path = dir.path().join("PROGRESS.archive.md");

    // Create a small journal.
    std::fs::write(&journal_path, "Small journal content").unwrap();

    let hook = JournalBoundingHook::with_defaults(journal_path.clone(), archive_path);

    // Should not archive (small journal).
    assert!(!hook.should_archive().await);
}

#[tokio::test]
async fn test_journal_bounding_hook_exceeding() {
    let dir = tempdir().unwrap();
    let journal_path = dir.path().join("PROGRESS.md");
    let archive_path = dir.path().join("PROGRESS.archive.md");

    // Create a large journal (30 KB).
    let large_content = "x".repeat(30 * 1024);
    std::fs::write(&journal_path, &large_content).unwrap();

    let hook = JournalBoundingHook::with_defaults(journal_path.clone(), archive_path.clone());

    // Should archive (large journal).
    assert!(hook.should_archive().await);
}

#[tokio::test]
async fn test_journal_bounding_hook_archive() {
    let dir = tempdir().unwrap();
    let journal_path = dir.path().join("PROGRESS.md");
    let archive_path = dir.path().join("PROGRESS.archive.md");

    // Create a large journal.
    let large_content = "x".repeat(30 * 1024);
    std::fs::write(&journal_path, &large_content).unwrap();

    let hook = JournalBoundingHook::with_defaults(journal_path.clone(), archive_path.clone());

    // Archive the journal.
    hook.archive_journal().await.unwrap();

    // The journal keeps only a pointer to where its entries went
    let journal_content = std::fs::read_to_string(&journal_path).unwrap();
    assert!(!journal_content.contains("xxx"), "{journal_content}");
    assert!(journal_content.contains("PROGRESS.archive.md"), "{journal_content}");

    // Archive should exist with original content.
    let archive_content = std::fs::read_to_string(&archive_path).unwrap();
    assert_eq!(archive_content, large_content);
}

#[tokio::test]
async fn test_journal_bounding_hook_on_agent_end() {
    let dir = tempdir().unwrap();
    let journal_path = dir.path().join("PROGRESS.md");
    let archive_path = dir.path().join("PROGRESS.archive.md");

    // Create a large journal.
    let large_content = "x".repeat(30 * 1024);
    std::fs::write(&journal_path, &large_content).unwrap();

    let hook = JournalBoundingHook::with_defaults(journal_path.clone(), archive_path.clone());

    // Fire the hook.
    let outcome = hook
        .on(
            HookPoint::AgentEnd,
            &HookEvent::Other(serde_json::json!({})),
            &HookCtx {
                session_id: "test".to_string(),
                event_tx: tokio::sync::mpsc::channel(1).0,
                hook_registry: crate::hook::HookRegistry::new(),
            },
        )
        .await;

    // Should return Continue.
    assert!(matches!(outcome, HookOutcome::Continue));

    // Journal should be archived.
    let journal_content = std::fs::read_to_string(&journal_path).unwrap();
    assert!(!journal_content.contains("xxx"), "{journal_content}");
    assert!(archive_path.exists());
}

/// The bug this replaced: the archive step renamed the journal *over* the archive, so the
/// second crossing destroyed everything the first one had saved.
#[tokio::test]
async fn a_second_archive_keeps_the_first_one() {
    let dir = tempdir().unwrap();
    let journal_path = dir.path().join("PROGRESS.md");
    let archive_path = dir.path().join("PROGRESS.archive.md");
    let hook = JournalBoundingHook::with_defaults(journal_path.clone(), archive_path.clone());

    std::fs::write(&journal_path, "first run's entries").unwrap();
    hook.archive_journal().await.unwrap();
    std::fs::write(&journal_path, "second run's entries").unwrap();
    hook.archive_journal().await.unwrap();

    let archived = std::fs::read_to_string(&archive_path).unwrap();
    assert!(archived.contains("first run's entries"), "history was overwritten: {archived:?}");
    assert!(archived.contains("second run's entries"), "{archived:?}");
    assert!(
        archived.find("first") < archived.find("second"),
        "and it stayed in order: {archived:?}"
    );
}

/// An archive that does not exist yet is created rather than being an error.
#[tokio::test]
async fn the_first_archive_creates_the_file() {
    let dir = tempdir().unwrap();
    let journal_path = dir.path().join("PROGRESS.md");
    let archive_path = dir.path().join("PROGRESS.archive.md");
    std::fs::write(&journal_path, "entries").unwrap();

    JournalBoundingHook::with_defaults(journal_path, archive_path.clone())
        .archive_journal()
        .await
        .unwrap();

    assert_eq!(std::fs::read_to_string(&archive_path).unwrap(), "entries");
}

/// An archived journal keeps its preamble, a pointer to the archive and its newest section; the
/// older sections move, in order.
///
/// The break this catches: archiving left `PROGRESS.md` empty, so the next turn read a journal
/// that said nothing — not where its history went, and not what the run had just done.
#[tokio::test]
async fn an_archived_journal_keeps_its_pointer_and_newest_section() {
    let dir = tempdir().unwrap();
    let journal_path = dir.path().join("PROGRESS.md");
    let archive_path = dir.path().join("PROGRESS.archive.md");
    let journal = "# Loop journal\n\nWhat this loop has done.\n\n## Phase 1\n\none\n\n\
                   ## Phase 2\n\ntwo with a ## inside\n\n## Phase 3\n\nthree\n";
    std::fs::write(&journal_path, journal).unwrap();

    JournalBoundingHook::with_defaults(journal_path.clone(), archive_path.clone())
        .archive_journal()
        .await
        .unwrap();

    assert_eq!(
        std::fs::read_to_string(&journal_path).unwrap(),
        "# Loop journal\n\nWhat this loop has done.\n\nEarlier entries are in \
         `PROGRESS.archive.md` beside this file.\n\n## Phase 3\n\nthree\n"
    );
    assert_eq!(
        std::fs::read_to_string(&archive_path).unwrap(),
        "## Phase 1\n\none\n\n## Phase 2\n\ntwo with a ## inside\n\n"
    );
}

/// A preamble that already points at the archive is not given a second pointer.
#[test]
fn a_pointer_is_added_once() {
    let journal = "# Journal\n\nOlder ones are in `A.md`.\n\n## One\n\n## Two\n";
    let (_, kept) = split_for_archive(journal, "A.md");
    assert_eq!(kept, "# Journal\n\nOlder ones are in `A.md`.\n\n## Two\n");
}

/// A journal of one section moves it whole, or a journal past its cap could never shrink; the
/// preamble and the pointer stay.
#[test]
fn a_single_section_moves_and_the_pointer_stays() {
    let journal = "# Journal\n\n## Only\n\nall of it\n";
    let (archived, kept) = split_for_archive(journal, "A.md");
    assert_eq!(archived, "## Only\n\nall of it\n");
    assert_eq!(kept, "# Journal\n\nEarlier entries are in `A.md` beside this file.\n\n");
}
