//! Threads — review threads and the conversation around them, read and answered.

use super::*;

/// Threads on removed lines count on the left, from `astral-sh/uv#21908`.
#[test]
fn a_thread_on_removed_lines_is_on_the_left() {
    let threads = parse_review_threads(THREADS_LEFT).unwrap().threads;
    let left: Vec<_> = threads.iter().filter(|thread| thread.side == ThreadSide::Left).collect();
    assert_eq!(left.len(), 3);
    assert_eq!((left[0].start_line, left[0].line), (Some(1576), Some(1583)));
}

/// A pull request GraphQL cannot see is not found, not an empty list.
#[test]
fn a_missing_pull_request_has_no_threads_rather_than_none() {
    let body = r#"{"data":{"repository":{"pullRequest":null},"rateLimit":{"cost":1,"remaining":4990,"resetAt":"2026-09-24T06:55:28Z"}}}"#;
    assert!(matches!(parse_review_threads(body), Err(GitHubError::NotFound)));
}

#[test]
fn gh_names_a_repository_off_github_com_by_host() {
    let mut repository = parse_repository(REPO_VIEW).unwrap();
    assert_eq!(repo_arg(&repository), "nora-quinn/luxe-dev");
    repository.host = "ghe.example.com".into();
    assert_eq!(repo_arg(&repository), "ghe.example.com/nora-quinn/luxe-dev");
    assert!(repository.is("Nora-Quinn/Luxe-Dev"));
    assert!(!repository.is("nora-quinn/luxe"));
}

// From a scratch pull request, posted to and closed for this.
pub(super) const PR_FILES: &str = include_str!("../../fixtures/pr_files.jsonl");
pub(super) const THREADS_OWN: &str = include_str!("../../fixtures/review_threads_own.json");
pub(super) const THREAD_REPLY: &str = include_str!("../../fixtures/thread_reply.json");
pub(super) const THREADS_CONVERSATION: &str =
    include_str!("../../fixtures/review_threads_conversation.json");
pub(super) const THREADS_BOT: &str = include_str!("../../fixtures/review_threads_bot.json");
pub(super) const PR_COMMENT: &str = include_str!("../../fixtures/pr_comment.json");
pub(super) const THREAD_RESOLVE: &str = include_str!("../../fixtures/thread_resolve.json");

/// A thread carries what answering it needs: its id, whether this account may reply and
/// resolve, and the line it was written on, from its patch.
#[test]
fn threads_carry_their_id_permissions_and_line() {
    let threads = parse_review_threads(THREADS_OWN).unwrap().threads;
    assert_eq!(threads.len(), 3);
    let loop_thread = threads.iter().find(|thread| thread.line == Some(7)).unwrap();
    assert!(loop_thread.id.starts_with("PRRT_"), "{}", loop_thread.id);
    assert!(loop_thread.can_reply && loop_thread.can_resolve);
    assert_eq!(
        loop_thread.anchor_text.as_deref(),
        Some("    for index in range(1, len(numbers)):")
    );
    let docstring = threads.iter().find(|thread| thread.line == Some(1)).unwrap();
    assert_eq!(
        docstring.anchor_text.as_deref(),
        Some(r#""""Scratch file for a review round trip. It is deleted with its pull request.""""#)
    );
    // Someone else's pull request: readable, not resolvable from here
    let theirs = parse_review_threads(THREADS).unwrap().threads;
    assert!(theirs.iter().all(|thread| !thread.can_resolve));
    assert!(theirs.iter().all(|thread| thread.id.starts_with("PRRT_")));
    assert!(theirs.iter().filter(|thread| !thread.file_level).all(|t| t.anchor_text.is_some()));
}

#[test]
fn a_patch_gives_the_line_without_its_marker() {
    assert_eq!(anchor_text("@@ -1 +1 @@\n-old\n+new").as_deref(), Some("new"));
    assert_eq!(anchor_text("@@ -1 +1 @@\n-gone\n").as_deref(), Some("gone"));
    assert_eq!(anchor_text("@@ -1,2 +1,2 @@\n kept").as_deref(), Some("kept"));
    assert_eq!(anchor_text("@@ -0,0 +1 @@"), None, "a file-level thread has no line");
    assert_eq!(anchor_text(""), None);
}

#[test]
fn a_reply_answers_where_it_can_be_read_and_a_resolve_must_stick() {
    assert_eq!(
        parse_reply(THREAD_REPLY).unwrap(),
        "https://github.com/nora-quinn/luxe-dev/pull/146#discussion_r4098691176"
    );
    assert!(parse_resolved(THREAD_RESOLVE).is_ok());
    let still_open = r#"{"data":{"resolveReviewThread":{"thread":{"isResolved":false}}}}"#;
    assert!(matches!(parse_resolved(still_open), Err(GitHubError::Rejected { .. })));
    let gone = r#"{"data":{"resolveReviewThread":null}}"#;
    assert!(matches!(parse_resolved(gone), Err(GitHubError::NotFound)));
    assert!(matches!(
        parse_reply(r#"{"data":{"addPullRequestReviewThreadReply":null}}"#),
        Err(GitHubError::NotFound)
    ));
    // `gh`'s own line for an id GitHub does not know
    assert!(matches!(
        classify_failure("gh: Could not resolve to a node with the global id of 'PRRT_x'", ""),
        GitHubError::NotFound
    ));
}

/// The conversation comes with the threads, as one list oldest first: a comment and a review's
/// summary alike, a minimized comment left out, and a bot marked as one. An answer from before
/// the query asked for it reads as a conversation with nothing said.
#[test]
fn the_conversation_comes_with_the_threads() {
    let read = parse_review_threads(THREADS_CONVERSATION).unwrap();
    assert!(read.threads.is_empty());
    let said: Vec<(&str, &str)> = read
        .conversation
        .iter()
        .map(|c| (c.id.as_str(), c.body.lines().next().unwrap_or_default()))
        .collect();
    assert_eq!(
        said,
        [
            (
                "IC_kwDOTyoMn88AAAABXB8h6w",
                "In `scratch-note.md`, please change \"Hello\" to \"Hello from the other side!\""
            ),
            ("PRR_kwDOTyoMn88AAAABPU2kMA", "Also, end the file with a newline."),
            (
                "IC_kwDOTyoMn88AAAABXCnvuQ",
                "> In `scratch-note.md`, please change \"Hello\" to \"Hello from the other side!\""
            ),
        ],
        "the two minimized ones are gone, and the rest are in the order they were said"
    );
    assert_eq!(read.conversation[0].author, "nora-quinn");
    assert!(read.conversation.iter().all(|comment| !comment.bot && comment.mine));
    assert!(read.conversation[1].url.ends_with("#pullrequestreview-5323465776"));

    // A review submitted with only line comments has no summary to say
    let bot = parse_review_threads(THREADS_BOT).unwrap();
    assert_eq!(bot.conversation.len(), 1, "{:?}", bot.conversation);
    assert!(bot.conversation[0].bot && bot.conversation[0].author == "github-actions");
    assert!(!bot.conversation[0].mine);

    let before = parse_review_threads(THREADS_OWN).unwrap();
    assert!(before.conversation.is_empty() && !before.threads.is_empty());
}

/// On a real pull request the reader answered the first comment from luxe, so it and the answer
/// stay quiet, and the review's summary is what is still asked.
#[test]
fn an_answered_comment_on_a_real_pull_request_asks_nothing() {
    let read = parse_review_threads(THREADS_CONVERSATION).unwrap();
    let conversation = crate::guided_review::earlier_conversation(&[], read);
    let ids: Vec<&str> = conversation.said().iter().map(|entry| entry.id.as_str()).collect();
    assert_eq!(ids, ["PRR_kwDOTyoMn88AAAABPU2kMA"]);
}

/// Comments and review summaries interleave by when they were written, not by kind.
#[test]
fn the_conversation_is_in_the_order_it_was_said() {
    let node = |id: &str, at: &str| ConversationNode {
        id: id.into(),
        author: None,
        body: "Said.".into(),
        created_at: at.into(),
        url: String::new(),
        is_minimized: false,
        viewer_did_author: false,
    };
    let said = conversation(
        vec![node("IC_late", "2026-05-02T12:00:00Z")],
        vec![node("PRR_early", "2026-05-02T09:00:00Z")],
    );
    let ids: Vec<&str> = said.iter().map(|comment| comment.id.as_str()).collect();
    assert_eq!(ids, ["PRR_early", "IC_late"]);
    assert!(said[0].author.is_empty() && !said[0].bot, "a deleted account is nobody, not a bot");
}

#[test]
fn a_conversation_comment_answers_where_it_can_be_read() {
    assert_eq!(
        parse_commented(PR_COMMENT).unwrap(),
        "https://github.com/nora-quinn/luxe-dev/pull/169#issuecomment-5840873033"
    );
    assert!(matches!(parse_commented("{}"), Err(GitHubError::Unreadable(_))));
}

/// A comment on the conversation reaches `gh` in a file rather than on the command line.
#[cfg(unix)]
#[tokio::test]
async fn a_conversation_comment_travels_in_a_file() {
    let dir = tempfile::tempdir().unwrap();
    let argv = dir.path().join("argv");
    let seen = dir.path().join("seen");
    let script = format!(
        "printf '%s\\n' \"$@\" > '{argv}'\n\
             prev=; for arg in \"$@\"; do [ \"$prev\" = --input ] && cat \"$arg\" > '{seen}'; prev=$arg; done\n\
             cat '{reply}'",
        argv = argv.display(),
        seen = seen.display(),
        reply = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/github/fixtures/pr_comment.json").display(),
    );
    let gh = GhCli { deadline: GH_DEADLINE, program: Some(fake_gh(dir.path(), &script)) };
    let repository = Repository {
        host: "github.com".into(),
        owner: "o".into(),
        name: "r".into(),
        default_branch: String::new(),
        remote: None,
    };
    let url = gh.comment_on_pull_request(&repository, 7, "Added it.").await.unwrap();
    assert!(url.ends_with("#issuecomment-5840873033"), "{url}");
    let argv = std::fs::read_to_string(&argv).unwrap();
    assert!(!argv.contains("Added it."), "{argv}");
    assert!(argv.contains("repos/o/r/issues/7/comments"), "{argv}");
    let seen: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&seen).unwrap()).unwrap();
    assert_eq!(seen, serde_json::json!({ "body": "Added it." }));
}

/// The reply's words reach `gh` through a file named with `@`, never as an argument another
/// user could read in a process listing.
#[cfg(unix)]
#[tokio::test]
async fn a_reply_travels_in_a_file_not_on_the_command_line() {
    let dir = tempfile::tempdir().unwrap();
    let argv = dir.path().join("argv");
    let seen = dir.path().join("seen");
    let script = format!(
        "printf '%s\\n' \"$@\" > '{argv}'\n\
             for arg in \"$@\"; do case \"$arg\" in body=@*) cat \"${{arg#body=@}}\" > '{seen}' ;; esac; done\n\
             cat '{reply}'",
        argv = argv.display(),
        seen = seen.display(),
        reply = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/github/fixtures/thread_reply.json").display(),
    );
    let gh = GhCli { deadline: GH_DEADLINE, program: Some(fake_gh(dir.path(), &script)) };
    let repository = Repository {
        host: "github.com".into(),
        owner: "o".into(),
        name: "r".into(),
        default_branch: String::new(),
        remote: None,
    };
    let url = gh.reply_to_thread(&repository, "PRRT_1", "Fixed in the next push.").await.unwrap();
    assert!(url.contains("#discussion_r"));
    let argv = std::fs::read_to_string(&argv).unwrap();
    assert!(!argv.contains("Fixed in the next push."), "{argv}");
    assert!(argv.contains("thread=PRRT_1"), "{argv}");
    assert_eq!(std::fs::read_to_string(&seen).unwrap(), "Fixed in the next push.");
}

/// A resolve names the thread, and an answer that leaves it open is a failure, not a success.
#[cfg(unix)]
#[tokio::test]
async fn a_resolve_names_its_thread_and_must_come_back_resolved() {
    let dir = tempfile::tempdir().unwrap();
    let repository = Repository {
        host: "github.com".into(),
        owner: "o".into(),
        name: "r".into(),
        default_branch: String::new(),
        remote: None,
    };
    let open = r#"{"data":{"resolveReviewThread":{"thread":{"isResolved":false}}}}"#;
    let script = format!(
        "case \"$*\" in *thread=PRRT_1*) cat '{resolved}' ;; *) echo '{open}' ;; esac",
        resolved = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/github/fixtures/thread_resolve.json")
            .display(),
    );
    let gh = GhCli { deadline: GH_DEADLINE, program: Some(fake_gh(dir.path(), &script)) };
    assert!(gh.resolve_thread(&repository, "PRRT_1").await.is_ok());
    assert!(matches!(
        gh.resolve_thread(&repository, "PRRT_2").await,
        Err(GitHubError::Rejected { .. })
    ));
}
