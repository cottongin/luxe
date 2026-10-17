//! Writes — opening a pull request, and what a refusal says.

use super::*;

pub(super) const PR_OPEN: &str = include_str!("../../fixtures/pr_open.json");
pub(super) const PR_OPEN_NO_COMMITS: &str =
    include_str!("../../fixtures/pr_open_422_no_commits.json");
pub(super) const PR_OPEN_EXISTS: &str = include_str!("../../fixtures/pr_open_422_exists.json");

#[test]
fn opening_a_pull_request_answers_its_number_and_refusals_say_why() {
    let opened = parse_opened(PR_OPEN).unwrap();
    assert_eq!(opened.number, 150);
    assert!(opened.url.ends_with("/pull/150"));
    for (body, reason) in [
        (PR_OPEN_NO_COMMITS, "No commits between master and master"),
        (PR_OPEN_EXISTS, "A pull request already exists for"),
    ] {
        let Some(GitHubError::Rejected { reasons }) = rejection(body) else {
            panic!("{body}");
        };
        assert!(reasons[0].starts_with(reason), "{reasons:?}");
    }
}

/// The body travels in a private file named with `--input`, never on the command line, and a
/// refusal comes back as GitHub's reason.
#[cfg(unix)]
#[tokio::test]
async fn opening_sends_the_request_in_a_file_and_reads_a_refusal() {
    let dir = tempfile::tempdir().unwrap();
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/github/fixtures");
    let argv = dir.path().join("argv");
    let sent = dir.path().join("sent");
    let script = format!(
        "printf '%s\\n' \"$@\" > '{argv}'\n\
             while [ \"$1\" != \"--input\" ]; do shift; done; cat \"$2\" > '{sent}'\n\
             if grep -q exists '{sent}'; then cat '{exists}'; exit 1; fi\n\
             cat '{opened}'",
        argv = argv.display(),
        sent = sent.display(),
        exists = fixtures.join("pr_open_422_exists.json").display(),
        opened = fixtures.join("pr_open.json").display(),
    );
    let gh = GhCli { deadline: GH_DEADLINE, program: Some(fake_gh(dir.path(), &script)) };
    let repository = Repository {
        host: "github.com".into(),
        owner: "o".into(),
        name: "r".into(),
        default_branch: String::new(),
        remote: None,
    };
    let request = NewPullRequest {
        head: "me:topic".into(),
        base: "main".into(),
        title: "Fix the loop".into(),
        body: "Closes #7 — the body stays private.".into(),
        draft: false,
    };
    let opened = gh.open_pull_request(&repository, &request).await.unwrap();
    assert_eq!(opened.number, 150);
    let argv = std::fs::read_to_string(&argv).unwrap();
    assert!(argv.contains("repos/o/r/pulls") && !argv.contains("the body stays private"), "{argv}");
    let sent: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&sent).unwrap()).unwrap();
    assert_eq!(sent["head"], "me:topic");
    assert_eq!(sent["draft"], false);
    assert_eq!(sent["body"], "Closes #7 — the body stays private.");

    let refused = NewPullRequest { title: "exists".into(), ..request };
    assert!(matches!(
        gh.open_pull_request(&repository, &refused).await,
        Err(GitHubError::Rejected { reasons }) if reasons[0].contains("already exists")
    ));
}

/// Nine threads on `cli/cli#14475`: multi-line, single-line, outdated, resolved and not.
#[test]
fn review_threads_are_read_from_graphql() {
    let read = parse_review_threads(THREADS).unwrap();
    assert_eq!(read.total, 9);
    let threads = read.threads;
    assert_eq!(threads.len(), 9);
    assert_eq!(threads.iter().filter(|thread| thread.resolved).count(), 5);

    let first = &threads[0];
    assert_eq!(first.path, ".github/skills/cli-exercise/scripts/terminal.mjs");
    assert_eq!((first.start_line, first.line), (Some(16), Some(25)), "a multi-line anchor");
    assert_eq!(first.side, ThreadSide::Right);
    assert_eq!(first.comments.len(), 2);
    assert_eq!(first.total_comments, 2);
    assert!(!first.comments[0].author.is_empty());

    let outdated = threads.iter().find(|thread| thread.outdated).expect("one is outdated");
    assert_eq!(outdated.line, None, "an outdated thread has no line at the current head");

    let single = threads.iter().find(|thread| thread.line == Some(169)).expect("line 169");
    assert_eq!(single.start_line, None, "a one-line anchor has no separate start");
    assert!(threads.iter().all(|thread| !thread.file_level));
}
