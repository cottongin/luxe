use super::*;

fn write(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, content).unwrap();
}

#[test]
fn a_new_file_is_recorded_as_having_had_nothing_in_it() {
    let dir = tempfile::tempdir().unwrap();
    let work = ExternalWork::new();
    let path = dir.path().join("notes.md");

    work.observe(&path);

    let record = work.record();
    assert_eq!(record.files.len(), 1);
    assert_eq!(record.files[0].baseline, ExternalBaseline::Absent);
    // Nothing was there, so nothing was lost: a creation's deletions are exactly zero, not
    // unknown.
    assert_eq!(record.files[0].baseline.line_count(), Some(0));
}

/// The whole reason this runs on `BeforeToolUse`: the second observation must not overwrite
/// what the session originally found, or a file edited twice reports only the last edit.
#[test]
fn the_first_look_is_the_one_that_counts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    write(&path, "one\ntwo\n");
    let work = ExternalWork::new();

    work.observe(&path);
    write(&path, "one\ntwo\nthree\nfour\n");
    work.observe(&path);

    let record = work.record();
    assert_eq!(
        record.files[0].baseline,
        ExternalBaseline::Text { content: "one\ntwo\n".into() },
        "still the content the session first saw"
    );
    assert_eq!(record.files[0].baseline.line_count(), Some(2));
}

#[test]
fn a_file_in_a_worktree_is_recorded_as_its_root() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("sibling");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    let path = repo.join("src/lib.rs");
    write(&path, "fn main() {}\n");

    let work = ExternalWork::new();
    work.observe(&path);

    let record = work.record();
    assert_eq!(record.roots, vec![repo.clone()], "the checkout, not the file");
    assert!(record.files.is_empty(), "Git reports that tree, so the file is not listed twice");

    // A second file in the same checkout adds nothing new to report.
    work.observe(&repo.join("src/other.rs"));
    assert_eq!(work.record().roots, vec![repo]);
}

/// A linked worktree and a submodule both carry a `.git` *file*, not a directory. Requiring a
/// directory would file every edit in one as a loose file and lose the branch it is on.
#[test]
fn a_linked_worktree_is_still_a_worktree() {
    let dir = tempfile::tempdir().unwrap();
    let linked = dir.path().join("feature-branch");
    std::fs::create_dir_all(&linked).unwrap();
    std::fs::write(linked.join(".git"), "gitdir: /elsewhere/.git/worktrees/feature\n").unwrap();

    let work = ExternalWork::new();
    work.observe(&linked.join("src/lib.rs"));

    assert_eq!(work.record().roots, vec![linked]);
}

/// A loose file observed before its checkout was known must not go on being reported
/// separately once the checkout is — Git already counts it there.
#[test]
fn a_worktree_claims_loose_files_beneath_it() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    let inside = repo.join("deep/file.txt");
    write(&inside, "a\n");

    let work = ExternalWork::new();
    work.observe(&inside);
    assert_eq!(work.record().files.len(), 1, "no .git yet, so it is loose");

    std::fs::create_dir_all(repo.join(".git")).unwrap();
    work.observe(&repo.join("deep/other.txt"));

    let record = work.record();
    assert_eq!(record.roots, vec![repo]);
    assert_eq!(record.files.len(), 1, "the earlier one stays, but no new file joins it");
}

#[test]
fn content_a_line_count_cannot_describe_is_recorded_without_one() {
    let dir = tempfile::tempdir().unwrap();
    let binary = dir.path().join("logo.png");
    std::fs::write(&binary, [0u8, 1, 2, 3]).unwrap();
    let big = dir.path().join("huge.log");
    std::fs::write(&big, vec![b'x'; MAX_RETAINED_BYTES as usize + 1]).unwrap();

    let work = ExternalWork::new();
    work.observe(&binary);
    work.observe(&big);

    for file in work.record().files {
        assert_eq!(file.baseline, ExternalBaseline::Unretained, "{}", file.path.display());
        assert_eq!(file.baseline.line_count(), None, "and no count is invented for it");
        // The distinction that matters: it existed, so this is not a creation.
        assert_ne!(file.baseline, ExternalBaseline::Absent);
    }
}

#[test]
fn the_record_stops_growing_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let work = ExternalWork::new();
    for index in 0..MAX_LOOSE_FILES + 5 {
        work.observe(&dir.path().join(format!("file-{index}.txt")));
    }

    let record = work.record();
    assert_eq!(record.files.len(), MAX_LOOSE_FILES);
    assert!(record.truncated, "and the surface can say the list is short");
}

#[test]
fn line_counts_match_what_a_reader_would_count() {
    assert_eq!(count_lines(""), 0);
    assert_eq!(count_lines("one\n"), 1);
    // A final line without its newline is still a line.
    assert_eq!(count_lines("one\ntwo"), 2);
    assert_eq!(count_lines("one\ntwo\n"), 2);
}

/// Reading is not changing. The guard prompts about an external `read` because looking at
/// someone's files is worth asking about; the Changes surface must not then claim one happened.
#[tokio::test]
async fn only_a_mutating_call_is_recorded() {
    use crate::hook::{Hook, HookCtx, HookEvent, HookPoint};

    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let outside = dir.path().join("elsewhere/notes.md");
    write(&outside, "hello\n");

    let work = ExternalWork::new();
    let hook = ExternalWorkHook::new(&project, work.clone());
    let (event_tx, _rx) = tokio::sync::mpsc::channel(8);
    let cx = HookCtx {
        session_id: "s1".into(),
        event_tx,
        hook_registry: crate::hook::HookRegistry::new(),
    };
    let call = |tool: &str, path: &Path| HookEvent::ToolUse {
        tool_name: tool.into(),
        args: serde_json::json!({ "path": path.to_string_lossy() }),
    };

    hook.on(HookPoint::BeforeToolUse, &call("read", &outside), &cx).await;
    assert!(work.is_empty(), "reading it changed nothing");

    // Inside the project, Git is already the witness.
    hook.on(HookPoint::BeforeToolUse, &call("edit", &project.join("src.rs")), &cx).await;
    assert!(work.is_empty(), "the project reports itself");

    hook.on(HookPoint::BeforeToolUse, &call("edit", &outside), &cx).await;
    assert_eq!(work.record().files.len(), 1, "an external edit is work");
}

#[test]
fn the_record_round_trips() {
    let record = ExternalWorkRecord {
        roots: vec![PathBuf::from("/work/sibling")],
        files: vec![ExternalFile {
            path: "/home/dev/.config/app.toml".into(),
            baseline: ExternalBaseline::Text { content: "a = 1\n".into() },
        }],
        truncated: true,
    };
    let encoded = serde_json::to_string(&record).unwrap();
    assert_eq!(serde_json::from_str::<ExternalWorkRecord>(&encoded).unwrap(), record);

    for baseline in [
        ExternalBaseline::Absent,
        ExternalBaseline::Unretained,
        ExternalBaseline::Text { content: "x".into() },
    ] {
        let encoded = serde_json::to_string(&baseline).unwrap();
        assert_eq!(serde_json::from_str::<ExternalBaseline>(&encoded).unwrap(), baseline);
    }
}
