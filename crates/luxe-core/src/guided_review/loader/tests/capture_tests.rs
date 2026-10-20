//! Tests — real Git captures of the shapes a complete source has to read: payload that looks like
//! a header, quoted paths, empty and binary additions and deletions, and output a user's config
//! must not reach.

use super::super::*;
use crate::guided_review::{ReviewFile, ReviewFileStatus, ReviewLineKind};
use std::path::Path;

async fn repository() -> (tempfile::TempDir, LocalGit) {
    let project = tempfile::tempdir().unwrap();
    let git = LocalGit::new(project.path());
    git.run("initialize repository", &["init", "--quiet"]).await.unwrap();
    git.run("name the author", &["config", "user.name", "luxe Tests"]).await.unwrap();
    git.run("mail the author", &["config", "user.email", "tests@luxe.invalid"]).await.unwrap();
    std::fs::write(project.path().join("keep.txt"), "kept\n").unwrap();
    commit_all(&git, "initial").await;
    (project, git)
}

async fn commit_all(git: &LocalGit, message: &str) -> String {
    git.run("stage everything", &["add", "--all"]).await.unwrap();
    git.run("commit", &["commit", "--quiet", "-m", message]).await.unwrap();
    let head = git.run("read HEAD", &["rev-parse", "HEAD"]).await.unwrap();
    String::from_utf8(head).unwrap().trim().to_string()
}

/// The one file a commit's complete source reads back, captured and parsed as a load does.
async fn committed_file(git: &LocalGit, message: &str) -> ReviewFile {
    let oid = commit_all(git, message).await;
    let capture = git.capture_and_persist_commit_source(&oid).await.expect("a complete source");
    let source = std::str::from_utf8(&capture.artifact.canonical_bytes).unwrap();
    let parsed = parse_unified_diff(source).unwrap();
    let [file] = &parsed.files[..] else { panic!("one file: {source}") };
    file.clone()
}

/// A removed line `-- text` and an added `++ text` read as payload, in a real Git patch.
///
/// The break this catches: inside a hunk, a `--- ` or `+++ ` line was refused as a stray header,
/// so one changed line of SQL or Lua comments failed the whole review.
#[tokio::test]
async fn header_looking_payload_in_a_real_patch_parses() {
    let (project, git) = repository().await;
    std::fs::write(project.path().join("query.sql"), "-- old comment\nselect 1;\n").unwrap();
    commit_all(&git, "sql").await;
    std::fs::write(project.path().join("query.sql"), "select 1;\n++ new line\n").unwrap();

    let file = committed_file(&git, "comment").await;
    let lines: Vec<(ReviewLineKind, &str)> =
        file.hunks[0].lines.iter().map(|line| (line.kind, line.text.as_str())).collect();
    assert_eq!(
        lines,
        [
            (ReviewLineKind::Deletion, "-- old comment"),
            (ReviewLineKind::Context, "select 1;"),
            (ReviewLineKind::Addition, "++ new line"),
        ]
    );
    assert_eq!((file.additions, file.deletions), (Some(1), Some(1)));
}

/// An empty file a commit adds is an addition with nothing in it.
///
/// The break this catches: Git writes no `---`/`+++` and no hunk for it, only `new file mode`,
/// and the parser refused the block, so the whole commit could not be reviewed.
#[tokio::test]
async fn an_empty_addition_is_an_added_file() {
    let (project, git) = repository().await;
    std::fs::write(project.path().join("empty.txt"), "").unwrap();

    let file = committed_file(&git, "empty").await;
    assert_eq!(file.status, ReviewFileStatus::Added);
    assert_eq!((file.old_path, file.new_path), (None, Some(PathBuf::from("empty.txt"))));
    assert_eq!((file.additions, file.deletions), (Some(0), Some(0)));
    assert!(file.hunks.is_empty() && file.non_text.is_none());
}

/// An empty file a commit deletes is a deletion with nothing in it.
#[tokio::test]
async fn an_empty_deletion_is_a_deleted_file() {
    let (project, git) = repository().await;
    std::fs::write(project.path().join("empty.txt"), "").unwrap();
    commit_all(&git, "empty").await;
    std::fs::remove_file(project.path().join("empty.txt")).unwrap();

    let file = committed_file(&git, "gone").await;
    assert_eq!(file.status, ReviewFileStatus::Deleted);
    assert_eq!((file.old_path, file.new_path), (Some(PathBuf::from("empty.txt")), None));
    assert_eq!((file.additions, file.deletions), (Some(0), Some(0)));
}

/// A binary file a commit adds is an addition, with no old path.
///
/// The break this catches: without `---`/`+++`, both paths came from `diff --git` and the new
/// file read as a modification of itself.
#[tokio::test]
async fn a_binary_addition_is_an_added_file() {
    let (project, git) = repository().await;
    std::fs::write(project.path().join("logo.bin"), [0u8, 1, 2, 255]).unwrap();

    let file = committed_file(&git, "logo").await;
    assert_eq!(file.status, ReviewFileStatus::Added);
    assert_eq!((file.old_path, file.new_path), (None, Some(PathBuf::from("logo.bin"))));
    assert!(file.binary);
}

/// A binary file a commit deletes is a deletion, with no new path.
#[tokio::test]
async fn a_binary_deletion_is_a_deleted_file() {
    let (project, git) = repository().await;
    std::fs::write(project.path().join("logo.bin"), [0u8, 1, 2, 255]).unwrap();
    commit_all(&git, "logo").await;
    std::fs::remove_file(project.path().join("logo.bin")).unwrap();

    let file = committed_file(&git, "gone").await;
    assert_eq!(file.status, ReviewFileStatus::Deleted);
    assert_eq!((file.old_path, file.new_path), (Some(PathBuf::from("logo.bin")), None));
    assert!(file.binary);
}

/// An empty file whose name holds a space, which Git does not quote, is read whole.
///
/// The break this catches: with no `---`/`+++`, the path came from `diff --git` split on the first
/// space, so `my notes.txt` failed the load.
#[tokio::test]
async fn an_empty_file_with_a_space_in_its_name_is_read_whole() {
    let (project, git) = repository().await;
    std::fs::write(project.path().join("my notes.txt"), "").unwrap();

    let file = committed_file(&git, "notes").await;
    assert_eq!(file.new_path, Some(PathBuf::from("my notes.txt")));
    assert_eq!(file.status, ReviewFileStatus::Added);
}

/// A commit adding an empty file and a binary one loads as a whole review, each its own item.
#[tokio::test]
async fn a_commit_of_empty_and_binary_additions_loads_whole() {
    let (project, git) = repository().await;
    std::fs::write(project.path().join("empty.txt"), "").unwrap();
    std::fs::write(project.path().join("logo.bin"), [0u8, 1, 2, 255]).unwrap();
    let oid = commit_all(&git, "assets").await;

    let review = git
        .load_target_review(
            crate::guided_review::ReviewId::new(),
            crate::guided_review::ReviewTarget::Commit { oid },
            crate::guided_review::ReviewContextDraft::default().build(),
            UntrackedSourcePolicy::default(),
        )
        .await
        .expect("a complete review");
    let statuses: Vec<_> = review.files.iter().map(|file| file.status).collect();
    assert_eq!(statuses, [ReviewFileStatus::Added, ReviewFileStatus::Added]);
    assert!(!review.chapters.is_empty());
}

/// luxe's own rendering of an empty file that is gone parses as the deletion it describes.
#[test]
fn an_empty_whole_file_deletion_parses() {
    let rendered = crate::guided_review::render_whole_file_deletion(Path::new("gone.txt"), "");
    let parsed = parse_unified_diff(&rendered).expect("its own rendering parses");
    let file = &parsed.files[0];
    assert_eq!(file.status, ReviewFileStatus::Deleted);
    assert_eq!(
        (file.old_path.clone(), file.new_path.clone()),
        (Some(PathBuf::from("gone.txt")), None)
    );
    assert_eq!((file.additions, file.deletions), (Some(0), Some(0)));
}

/// A user's diff config cannot change a captured source or run a program: no color, no external
/// diff, no textconv.
///
/// The break this catches: complete captures ran plain `git diff`, which honors `color.ui=always`
/// and `diff.external`, so a reviewer's own config could put escape codes in the source or run a
/// diff helper in their place.
#[cfg(unix)]
#[tokio::test]
async fn a_users_diff_config_cannot_reach_a_capture() {
    let (project, git) = repository().await;
    let outside = tempfile::tempdir().unwrap();
    let marker = outside.path().join("helper-ran");
    let helper = outside.path().join("helper.sh");
    std::fs::write(&helper, format!("#!/bin/sh\ntouch '{}'\necho HELPER\n", marker.display()))
        .unwrap();
    let executable = std::os::unix::fs::PermissionsExt::from_mode(0o755);
    std::fs::set_permissions(&helper, executable).unwrap();
    git.run("color always", &["config", "color.ui", "always"]).await.unwrap();
    git.run("external diff", &["config", "diff.external", helper.to_str().unwrap()]).await.unwrap();
    git.run("textconv", &["config", "diff.shout.textconv", "tr a-z A-Z <"]).await.unwrap();
    std::fs::write(project.path().join(".gitattributes"), "*.txt diff=shout\n").unwrap();
    commit_all(&git, "attributes").await;
    std::fs::write(project.path().join("keep.txt"), "changed\n").unwrap();

    let base = git.run("read HEAD", &["rev-parse", "HEAD"]).await.unwrap();
    let base = String::from_utf8(base).unwrap();
    let tracked = git.capture_tracked_diff(base.trim()).await.unwrap();
    // a root commit goes through `diff-tree`, which reads none of these settings, so only a
    // commit with a parent shows the pins at work
    let committed = committed_file(&git, "changed").await;

    let tracked = String::from_utf8(tracked).unwrap();
    assert!(!tracked.contains('\u{1b}') && !tracked.contains("HELPER"), "{tracked:?}");
    assert!(tracked.contains("+changed") && !tracked.contains("CHANGED"), "no textconv: {tracked}");
    assert_eq!(committed.status, ReviewFileStatus::Modified);
    let added: Vec<&str> = committed.hunks[0]
        .lines
        .iter()
        .filter(|line| line.kind == ReviewLineKind::Addition)
        .map(|line| line.text.as_str())
        .collect();
    assert_eq!(added, ["changed"], "no textconv in the commit capture");
    assert!(!marker.exists(), "the external diff ran");
}

/// Snapshot roots diffed with `--no-index`: `before/` against `after/`, normalized as a load does.
async fn snapshot_source(files: &[(&str, &str, &str)]) -> String {
    let root = tempfile::tempdir().unwrap();
    for (name, before, after) in files {
        for (side, contents) in [("before", before), ("after", after)] {
            let path = root.path().join(side).join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, contents).unwrap();
        }
    }
    let git = LocalGit::new(root.path());
    // a repository only for its config: quoted names whatever this machine's `core.quotePath`
    git.run("init", &["init", "--quiet"]).await.unwrap();
    git.run("quote paths", &["config", "core.quotePath", "true"]).await.unwrap();
    git.diff_snapshot_roots(root.path(), "before", "after").await.expect("normalized")
}

/// A path Git quotes, here a name with a non-ASCII letter, normalizes and parses.
///
/// The break this catches: the normalizer took a header only when it began `a/<root>/`, so
/// Git's quoted `"a/<root>/..."` was refused before the quote-reading parser could see it.
#[tokio::test]
async fn a_quoted_snapshot_path_normalizes() {
    let source = snapshot_source(&[("na\u{ef}ve.txt", "one\n", "two\n")]).await;
    assert!(source.contains("\"a/na\\303\\257ve.txt\""), "Git quoted it: {source}");
    let parsed = parse_unified_diff(&source).unwrap();
    assert_eq!(parsed.files[0].new_path, Some(PathBuf::from("na\u{ef}ve.txt")), "{source}");
}

/// A name holding the separator Git puts between its two paths splits where the names agree.
///
/// The break this catches: taking the first ` b/<root>/` as where the new path begins reads
/// `x b/after/y.txt` as the file `x`.
#[tokio::test]
async fn a_name_holding_the_separator_splits_where_the_names_agree() {
    // the second sits under a directory named `a`, which a repeated prefix strip ate
    for name in ["x b/after/y.txt", "a/x b/after/y.txt"] {
        let source = snapshot_source(&[(name, "one\n", "two\n")]).await;
        let header = source.lines().next().unwrap();
        assert_eq!(header, format!("diff --git a/{name} b/{name}"));
        let parsed = parse_unified_diff(&source).unwrap();
        let file = &parsed.files[0];
        let expected = Some(PathBuf::from(name));
        assert_eq!((&file.old_path, &file.new_path), (&expected, &expected), "{source}");
    }
}

/// A name with quotes in it, which Git escapes inside its quoted form, normalizes and parses.
#[cfg(unix)]
#[tokio::test]
async fn a_snapshot_path_with_quotes_normalizes() {
    let source = snapshot_source(&[("say \"hi\".txt", "one\n", "two\n")]).await;
    let parsed = parse_unified_diff(&source).unwrap();
    assert_eq!(parsed.files[0].new_path, Some(PathBuf::from("say \"hi\".txt")), "{source}");
}

/// A path that holds a root's name further in keeps it: only the root at the start goes.
///
/// The break this catches: the root was taken out wherever `<side>/<root>/` first appeared on the
/// line, so `lib/before/x.txt` under the `before` root came back as `lib/x.txt`.
#[tokio::test]
async fn a_root_name_inside_a_path_stays() {
    let source = snapshot_source(&[("lib/before/x.txt", "one\n", "two\n")]).await;
    let parsed = parse_unified_diff(&source).unwrap();
    assert_eq!(parsed.files[0].new_path, Some(PathBuf::from("lib/before/x.txt")), "{source}");
    assert!(source.starts_with("diff --git a/lib/before/x.txt b/lib/before/x.txt\n"), "{source}");
}

/// Payload that looks like a snapshot header keeps its text: only structural headers move.
///
/// The break this catches: every `--- ` and `+++ ` line was rewritten, so a removed line
/// `-- a/before/name` came back as `-- a/name`, changing the source a reviewer reads.
#[tokio::test]
async fn header_looking_snapshot_payload_keeps_its_text() {
    let source =
        snapshot_source(&[("notes.txt", "keep\n-- a/before/name\n", "keep\n++ b/after/name\n")])
            .await;
    let parsed = parse_unified_diff(&source).unwrap();
    let texts: Vec<&str> =
        parsed.files[0].hunks[0].lines.iter().map(|line| line.text.as_str()).collect();
    assert_eq!(texts, ["keep", "-- a/before/name", "++ b/after/name"], "{source}");
}

/// A repository whose clean filter runs `filter`, applied to a tracked file changed in the tree.
#[cfg(unix)]
async fn filtered_change(filter: &str) -> (tempfile::TempDir, LocalGit) {
    let (project, git) = repository().await;
    git.run("filter", &["config", "filter.slow.clean", filter]).await.unwrap();
    std::fs::write(project.path().join(".gitattributes"), "keep.txt filter=slow\n").unwrap();
    std::fs::write(project.path().join("keep.txt"), "changed\n").unwrap();
    (project, git)
}

#[cfg(unix)]
async fn load_working_tree(
    git: &LocalGit,
) -> Result<crate::guided_review::GuidedReview, GuidedReviewError> {
    git.load_target_review(
        crate::guided_review::ReviewId::new(),
        crate::guided_review::ReviewTarget::WorkingTree,
        crate::guided_review::ReviewContextDraft::default().build(),
        UntrackedSourcePolicy::default(),
    )
    .await
}

/// A Git command that hangs, here a clean filter, is stopped at its deadline and said to have.
///
/// The break this catches: review captures awaited Git with no deadline, so a filter or signer
/// that never returned held the review open forever.
#[cfg(unix)]
#[tokio::test]
async fn a_hanging_git_times_out() {
    let (_project, git) = filtered_change("sleep 30").await;
    let git = git.with_git_deadline(std::time::Duration::from_millis(500));

    let started = std::time::Instant::now();
    let error = load_working_tree(&git).await.expect_err("it gives up");
    assert!(matches!(error, GuidedReviewError::GitTimeout { .. }), "{error}");
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
}

/// A capture given up on takes its Git, and what Git started, with it.
///
/// The break this catches: a dropped capture left Git and its filter running, since nothing tied
/// the processes to the future that was waiting on them.
#[cfg(unix)]
#[tokio::test]
async fn a_dropped_capture_leaves_no_child() {
    let marker = tempfile::tempdir().unwrap();
    let pid_file = marker.path().join("pid");
    let filter = format!("sh -c 'echo $$ > {}; exec sleep 30'", pid_file.display());
    let (_project, git) = filtered_change(&filter).await;
    let capture = tokio::spawn(async move { load_working_tree(&git).await });
    let pid = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if let Some(pid) =
                std::fs::read_to_string(&pid_file).ok().filter(|p| !p.trim().is_empty())
            {
                return pid.trim().to_string();
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the filter started");

    capture.abort();
    let _ = capture.await;
    let gone = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while crate::program::tests::still_running(pid.parse().unwrap()) {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(gone.is_ok(), "the filter {pid} outlived the capture");
}
