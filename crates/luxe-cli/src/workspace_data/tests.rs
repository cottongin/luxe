use super::changes::{count_added_lines, parse_numstat, parse_status};
use super::*;
use luxe_core::{FileChangeStatus, TodoStatus, TodoStore};

#[test]
fn compact_path_keeps_external_paths_intact() {
    assert_eq!(luxe_core::paths::home_relative(Path::new("/tmp/luxe")), "/tmp/luxe");
}

#[test]
fn git_parsers_keep_binary_counts_unknown() {
    let status = parse_status(" M src/lib.rs\0?? README.md\0R  new-name\0old-name\0");
    assert_eq!(
        status,
        vec![
            (PathBuf::from("src/lib.rs"), FileChangeStatus::Modified),
            (PathBuf::from("README.md"), FileChangeStatus::Untracked),
            (PathBuf::from("new-name"), FileChangeStatus::Renamed),
        ]
    );

    let counts = parse_numstat("2\t3\tsrc/lib.rs\0-\t-\tassets/logo.png\0");
    assert_eq!(counts[&PathBuf::from("src/lib.rs")], (Some(2), Some(3), false));
    assert_eq!(counts[&PathBuf::from("assets/logo.png")], (None, None, true));
}

/// The seam the unit test above could not see: `parse_status` was always correct, and
/// `run_git` handed it output whose leading space had been trimmed away.
///
/// A worktree-only modification is reported as `" M path"`, so the first record lost its
/// first character (`src/a.txt` → `rc/a.txt`) and, keyed on the mangled path, its numstat
/// counts came back `+0 −0`. Real git, real repository — a fixture string cannot reproduce it.
#[tokio::test]
async fn change_summary_reads_real_git_output_without_eating_the_first_path() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(["-c", "user.email=t@t", "-c", "user.name=t"])
            .args(args)
            .current_dir(&root)
            .output()
            .expect("git runs")
    };
    git(&["init", "-q"]);
    std::fs::create_dir(root.join("src")).unwrap();
    std::fs::write(root.join("src/a.txt"), "alpha\nbeta\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-qm", "init"]);
    // An unstaged edit sorts first in git's output, which is where the damage happened.
    std::fs::write(root.join("src/a.txt"), "alpha\nbeta\ngamma\n").unwrap();
    std::fs::write(root.join("untracked.txt"), "new\n").unwrap();

    let summary = collect_change_summary(root.clone()).await;
    let modified = summary
        .files
        .iter()
        .find(|file| file.status == FileChangeStatus::Modified)
        .expect("the edited file is reported");
    assert_eq!(modified.path, PathBuf::from("src/a.txt"), "the whole path survives");
    assert_eq!(
        (modified.additions, modified.deletions),
        (Some(1), Some(0)),
        "and its counts are found, because the numstat lookup is keyed on that path"
    );
    let untracked = summary
        .files
        .iter()
        .find(|file| file.path == Path::new("untracked.txt"))
        .expect("untracked files are still reported");
    assert_eq!(
        (untracked.additions, untracked.deletions),
        (Some(1), Some(0)),
        "a new file's whole content is an addition — git diff says nothing about it, and the \
             dock used to render the silence as `+0 −0`"
    );
}

#[test]
fn added_line_counts_skip_what_they_cannot_read() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    std::fs::write(root.join("two-lines.txt"), "one\ntwo\n").unwrap();
    assert_eq!(count_added_lines(&root.join("two-lines.txt")), Some(2));

    // A final line without its newline still counts.
    std::fs::write(root.join("no-trailing.txt"), "one\ntwo").unwrap();
    assert_eq!(count_added_lines(&root.join("no-trailing.txt")), Some(2));

    std::fs::write(root.join("empty.txt"), "").unwrap();
    assert_eq!(count_added_lines(&root.join("empty.txt")), Some(0));

    std::fs::write(root.join("binary.bin"), [0u8, 1, 2, 3]).unwrap();
    assert_eq!(count_added_lines(&root.join("binary.bin")), None, "binary has no line count");

    // Git reports an untracked directory as one `dir/` entry; it is not a file.
    std::fs::create_dir(root.join("subdir")).unwrap();
    assert_eq!(count_added_lines(&root.join("subdir")), None);
    assert_eq!(count_added_lines(&root.join("missing.txt")), None);
}

/// Sets up a real Git repository at `root` with one committed file.
fn git_repo_with_a_commit(root: &Path) {
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(["-c", "user.email=t@t", "-c", "user.name=t"])
            .args(args)
            .current_dir(root)
            .output()
            .expect("git runs")
    };
    std::fs::create_dir_all(root).unwrap();
    git(&["init", "-q"]);
    std::fs::write(root.join("lib.rs"), "alpha\nbeta\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-qm", "init"]);
}

/// A checkout is asked the same question the project is asked, which is the whole point of
/// recording the root instead of the file: this change is made with the shell, so nothing luxe
/// tracks per-file could have seen it.
#[tokio::test]
async fn an_external_checkout_is_reported_by_git() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let sibling = dir.path().join("sibling");
    git_repo_with_a_commit(&sibling);

    let work = ExternalWork::new();
    work.observe(&sibling.join("lib.rs"));
    // Not through any luxe tool — exactly what the per-file ledger cannot know about.
    std::fs::write(sibling.join("lib.rs"), "alpha\nbeta\ngamma\n").unwrap();

    let summary = collect_external_summary(work.record(), &project).await;
    assert_eq!(summary.roots.len(), 1);
    let root = &summary.roots[0];
    assert!(root.git, "a worktree reports through Git");
    assert_eq!(root.files[0].path, PathBuf::from("lib.rs"));
    assert_eq!((root.files[0].additions, root.files[0].deletions), (Some(1), Some(0)));
    assert_eq!(summary.additions, Some(1));
    assert_eq!(summary.total_files, 1);
}

/// A repository luxe was opened *inside* reports the project's own files too. They already have
/// a section, and counting them twice would double every number on screen.
#[tokio::test]
async fn a_parent_checkout_does_not_report_the_project_twice() {
    let dir = tempfile::tempdir().unwrap();
    let outer = dir.path().join("outer");
    git_repo_with_a_commit(&outer);
    let project = outer.join("project");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("inside.txt"), "mine\n").unwrap();
    std::fs::write(outer.join("outside.txt"), "theirs\n").unwrap();

    let work = ExternalWork::new();
    work.observe(&outer.join("outside.txt"));

    let summary = collect_external_summary(work.record(), &project).await;
    let listed: Vec<_> =
        summary.roots.iter().flat_map(|root| &root.files).map(|f| &f.path).collect();
    assert_eq!(listed, vec![&PathBuf::from("outside.txt")], "only what is outside the project");
    assert_eq!(summary.total_files, 1, "and the total agrees with the list");
}

/// The counts a loose file can be measured with, each from a different arm.
#[tokio::test]
async fn a_loose_file_is_measured_against_what_the_session_found() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let elsewhere = dir.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();

    let created = elsewhere.join("new.md");
    let edited = elsewhere.join("edited.md");
    let removed = elsewhere.join("gone.md");
    std::fs::write(&edited, "one\ntwo\nthree\n").unwrap();
    std::fs::write(&removed, "a\nb\n").unwrap();

    let work = ExternalWork::new();
    for path in [&created, &edited, &removed] {
        work.observe(path);
    }
    std::fs::write(&created, "fresh\ncontent\n").unwrap();
    std::fs::write(&edited, "one\nTWO\nthree\nfour\n").unwrap();
    std::fs::remove_file(&removed).unwrap();

    let summary = collect_external_summary(work.record(), &project).await;
    assert_eq!(summary.roots.len(), 1, "one group for the directory they share");
    let root = &summary.roots[0];
    assert!(!root.git, "no repository behind them");
    let by_name =
        |name: &str| root.files.iter().find(|f| f.path == Path::new(name)).expect("listed").clone();

    let created = by_name("new.md");
    assert_eq!(created.status, FileChangeStatus::Added);
    assert_eq!((created.additions, created.deletions), (Some(2), Some(0)));

    let edited = by_name("edited.md");
    assert_eq!(edited.status, FileChangeStatus::Modified);
    assert_eq!(
        (edited.additions, edited.deletions),
        (Some(2), Some(1)),
        "one line rewritten and one appended"
    );

    let removed = by_name("gone.md");
    assert_eq!(removed.status, FileChangeStatus::Deleted);
    assert_eq!(
        (removed.additions, removed.deletions),
        (Some(0), Some(2)),
        "everything it held is what was lost"
    );

    assert_eq!(summary.total_files, 3);
    assert_eq!((summary.additions, summary.deletions), (Some(4), Some(3)));
}

/// The section reports what the session *changed*, not what it opened. A file touched and left
/// alone — or created and then removed — leaves the filesystem where it started, and listing it
/// would turn a change list into an activity log.
#[tokio::test]
async fn touching_a_file_without_changing_it_reports_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let elsewhere = dir.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();

    let unchanged = elsewhere.join("same.md");
    std::fs::write(&unchanged, "steady\n").unwrap();
    let transient = elsewhere.join("transient.md");

    let work = ExternalWork::new();
    work.observe(&unchanged);
    work.observe(&transient);
    // Written and then removed again: the tree is exactly as it was found.
    std::fs::write(&transient, "scratch\n").unwrap();
    std::fs::remove_file(&transient).unwrap();

    let summary = collect_external_summary(work.record(), &project).await;
    assert!(summary.is_empty(), "nothing changed, so there is nothing to report");
    assert_eq!(summary.total_files, 0);
}

/// A file whose baseline could not be retained is still a change; it just cannot carry numbers.
/// Reporting `+0 −0` for it would be a claim, and the wrong one.
#[tokio::test]
async fn an_unmeasurable_file_is_listed_without_inventing_counts() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let binary = dir.path().join("assets/logo.png");
    std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
    std::fs::write(&binary, [0u8, 1, 2, 3]).unwrap();

    let work = ExternalWork::new();
    work.observe(&binary);
    std::fs::write(&binary, [0u8, 9, 9, 9]).unwrap();

    let summary = collect_external_summary(work.record(), &project).await;
    let file = &summary.roots[0].files[0];
    assert_eq!(file.status, FileChangeStatus::Modified);
    assert_eq!((file.additions, file.deletions), (None, None));
    assert_eq!(
        (summary.additions, summary.deletions),
        (None, None),
        "a total that omits an unmeasurable row is worse than no total"
    );
}

/// The snapshot is what front-ends read, so the section has to survive the trip through it.
#[tokio::test]
async fn the_collector_publishes_external_work_in_its_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let outside = dir.path().join("notes/todo.md");
    std::fs::create_dir_all(outside.parent().unwrap()).unwrap();

    let work = ExternalWork::new();
    work.observe(&outside);
    std::fs::write(&outside, "buy milk\n").unwrap();

    let mut collector =
        WorkspaceCollector::with_diagnostics_store(project.clone(), DiagnosticsStore::new())
            .with_external_work(work);
    let snapshot = collector.collect().await;

    assert!(snapshot.changes.files.is_empty(), "the project itself is untouched");
    assert_eq!(snapshot.external.total_files, 1);
    assert_eq!(snapshot.external.roots[0].files[0].path, PathBuf::from("todo.md"));
}

#[test]
fn bounded_text_redacts_and_truncates_display_data() {
    let secret = format!("sk-{}", "a".repeat(24));
    let value = bounded_text(&format!("{secret}{}", "x".repeat(200)), 16);
    assert!(!value.contains("sk-"));
    assert!(value.ends_with('…'));
}

#[tokio::test]
async fn collector_revisions_increase_within_one_scope() {
    let root = tempfile::tempdir().unwrap();
    let mut collector = WorkspaceCollector::new(root.path().to_path_buf());
    let first = collector.collect().await;
    let second = collector.collect().await;
    assert_eq!(first.scope_id, second.scope_id);
    assert_eq!(first.revision + 1, second.revision);
    assert_eq!(first.project.git_branch, None);
    assert!(!first.project.git_dirty);
}

/// Seeds a two-item checklist — one running, one waiting on it — for `session_id`.
fn seed_todos(root: &std::path::Path, session_id: &str) {
    TodoStore::new(root.join(".luxe/todos"))
        .write(
            session_id,
            &[
                luxe_core::Todo {
                    id: 1,
                    subject: "Collect the snapshot".into(),
                    status: TodoStatus::InProgress,
                    active_form: None,
                    blocked_by: vec![],
                },
                luxe_core::Todo {
                    id: 2,
                    subject: "Wait for collector".into(),
                    status: TodoStatus::Pending,
                    active_form: None,
                    blocked_by: vec![1],
                },
            ],
        )
        .unwrap();
}

#[tokio::test]
async fn collector_summarizes_the_sessions_todos() {
    let root = tempfile::tempdir().unwrap();
    seed_todos(root.path(), "s1");

    let mut collector = WorkspaceCollector::new(root.path().to_path_buf());
    collector.set_session("s1");
    let snapshot = collector.collect().await;

    assert_eq!(snapshot.todos.total, 2);
    assert_eq!(snapshot.todos.in_progress, 1);
    assert_eq!(snapshot.todos.blocked, 1);
    assert_eq!(snapshot.todos.current.unwrap().id, "1");
    assert_eq!(
        snapshot.todos.items.into_iter().find(|item| item.id == "2").unwrap().blocked_by,
        vec!["1"]
    );

    // Its blocker done, the task is ready: the dock agrees with the todo tool's own list
    let store = TodoStore::new(root.path().join(".luxe/todos"));
    let mut todos = store.read("s1").unwrap();
    todos[0].status = TodoStatus::Completed;
    store.write("s1", &todos).unwrap();
    let snapshot = collector.collect().await;
    assert_eq!(snapshot.todos.blocked, 0);
    assert_eq!(snapshot.todos.pending, 1);
    let item = snapshot.todos.items.into_iter().find(|item| item.id == "2").unwrap();
    assert_eq!(item.status, luxe_core::TodoSummaryStatus::Pending);
    assert!(item.blocked_by.is_empty());
}

/// A session with a worktree keeps its checklist with the project and its changes in the
/// worktree: the collector reads each from its own root, so neither shows the other's.
#[tokio::test]
async fn state_comes_from_the_project_and_changes_from_the_worktree() {
    let project = tempfile::tempdir().unwrap();
    let worktree = tempfile::tempdir().unwrap();
    seed_todos(project.path(), "s1");
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(worktree.path())
            .output()
            .unwrap();
        assert!(output.status.success(), "{args:?}: {output:?}");
    };
    git(&["init", "-q", "-b", "topic"]);
    std::fs::write(worktree.path().join("fix.rs"), "fn fix() {}\n").unwrap();

    let mut collector = WorkspaceCollector::new(worktree.path().to_path_buf())
        .with_state_root(project.path().to_path_buf());
    collector.set_session("s1");
    let snapshot = collector.collect().await;

    assert_eq!(snapshot.todos.total, 2, "the checklist is the project's");
    assert_eq!(
        snapshot.worktree.as_deref(),
        Some(luxe_core::paths::display(worktree.path()).as_str()),
        "outside the project, as written"
    );
    assert_eq!(snapshot.project.display_name, luxe_core::paths::dir_name(project.path()));
    assert_eq!(
        snapshot.project.git_branch.as_deref(),
        Some("topic"),
        "the branch is the worktree's"
    );
    assert!(
        snapshot.changes.files.iter().any(|file| file.path.ends_with("fix.rs")),
        "{:?}",
        snapshot.changes.files
    );
}

#[tokio::test]
async fn the_dock_plan_is_scoped_to_its_own_session() {
    let root = tempfile::tempdir().unwrap();
    // Another session in this repo already has a plan.
    seed_todos(root.path(), "session-elsewhere");

    let mut collector = WorkspaceCollector::new(root.path().to_path_buf());
    collector.set_session("session-fresh");
    let fresh = collector.collect().await;
    assert_eq!(fresh.todos.total, 0, "a fresh session shows no carried-over plan");

    collector.set_session("session-elsewhere");
    let owner = collector.collect().await;
    assert_eq!(owner.todos.total, 2, "its owner still sees it");
}

#[test]
fn bounded_title_collapses_whitespace_and_bounds_length() {
    assert_eq!(
        bounded_title("  Refine   the\n\n workspace  overview "),
        "Refine the workspace overview"
    );
    let title = bounded_title(&"word ".repeat(40));
    assert!(title.chars().count() <= 61, "bounded to 60 + ellipsis");
    assert!(title.ends_with('\u{2026}'));
}

#[tokio::test]
async fn session_title_is_derived_from_the_first_user_message() {
    use luxe_core::message::Message;
    use luxe_core::session::{EntryKind, SessionStore};

    let root = tempfile::tempdir().unwrap();
    let mut store = crate::engine_setup::open_session_store(root.path());
    let id = store.create(&root.path().to_string_lossy()).unwrap();
    store
        .append(&id, None, EntryKind::Message(Message::user("  Refine the\n workspace  overview ")))
        .unwrap();

    let mut collector = WorkspaceCollector::new(root.path().to_path_buf());
    collector.set_session(id.0.clone());
    // Untitled before the first collect derives the title (no raw session id leaked).
    assert!(collector.session.as_ref().unwrap().title.is_empty());

    let snapshot = collector.collect().await;
    assert_eq!(snapshot.session.unwrap().title, "Refine the workspace overview");
}

#[tokio::test]
async fn session_without_a_user_message_stays_untitled() {
    use luxe_core::session::SessionStore;
    let root = tempfile::tempdir().unwrap();
    let mut store = crate::engine_setup::open_session_store(root.path());
    let id = store.create(&root.path().to_string_lossy()).unwrap();

    let mut collector = WorkspaceCollector::new(root.path().to_path_buf());
    collector.set_session(id.0.clone());
    let snapshot = collector.collect().await;
    // A brand-new session shows no derived title (and the dock renders no Session row), rather
    // than leaking the raw session id.
    assert!(snapshot.session.unwrap().title.is_empty());
}

#[tokio::test]
async fn collector_reduces_diagnostics_worst_first_and_scopes_to_root() {
    use luxe_core::lsp::{Diagnostic, Severity};

    fn diag(severity: Severity) -> Diagnostic {
        Diagnostic { severity: Some(severity), line: 0, character: 0, message: "boom".into() }
    }

    let root = tempfile::tempdir().unwrap();
    let store = DiagnosticsStore::new();
    // One warning-only file and one error-heavy file under the root, plus a file outside it.
    store.set(root.path().join("src/lib.rs"), vec![diag(Severity::Warning)]);
    store.set(
        root.path().join("src/main.rs"),
        vec![diag(Severity::Error), diag(Severity::Error), diag(Severity::Warning)],
    );
    store.set(std::path::PathBuf::from("/elsewhere/other.rs"), vec![diag(Severity::Error)]);

    let mut collector =
        WorkspaceCollector::with_diagnostics_store(root.path().to_path_buf(), store);
    let snapshot = collector.collect().await;
    let diagnostics = &snapshot.diagnostics;

    // Only the two in-root files count; the external file is excluded.
    assert_eq!(diagnostics.total_files, 2);
    assert_eq!((diagnostics.errors, diagnostics.warnings), (2, 2));
    // Worst-first: the error-heavy file leads, with a project-relative path.
    assert_eq!(diagnostics.files[0].path, std::path::PathBuf::from("src/main.rs"));
    assert_eq!((diagnostics.files[0].errors, diagnostics.files[0].warnings), (2, 1));
    assert_eq!(diagnostics.files[1].path, std::path::PathBuf::from("src/lib.rs"));
    assert!(!diagnostics.truncated);
}

#[tokio::test]
async fn collector_omits_info_and_hint_only_files_and_clears_fixed_files() {
    use luxe_core::lsp::{Diagnostic, Severity};

    let root = tempfile::tempdir().unwrap();
    let store = DiagnosticsStore::new();
    let noisy = root.path().join("src/notes.rs");
    // Info/hint only: not actionable, so it never appears in the dock summary.
    store.set(
        noisy.clone(),
        vec![Diagnostic {
            severity: Some(Severity::Information),
            line: 0,
            character: 0,
            message: "info".into(),
        }],
    );
    assert!(WorkspaceCollector::with_diagnostics_store(root.path().to_path_buf(), store.clone())
        .collect()
        .await
        .diagnostics
        .is_empty());

    // A real error shows up; clearing the file (fixed) drops it again.
    store.set(
        noisy.clone(),
        vec![Diagnostic {
            severity: Some(Severity::Error),
            line: 1,
            character: 0,
            message: "E".into(),
        }],
    );
    assert_eq!(
        WorkspaceCollector::with_diagnostics_store(root.path().to_path_buf(), store.clone())
            .collect()
            .await
            .diagnostics
            .errors,
        1
    );
    store.set(noisy, Vec::new());
    assert!(WorkspaceCollector::with_diagnostics_store(root.path().to_path_buf(), store)
        .collect()
        .await
        .diagnostics
        .is_empty());
}
