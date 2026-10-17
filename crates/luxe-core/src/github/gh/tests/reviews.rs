//! Reviews — commentable lines from a patch, a posted review, and the shape GitHub accepted.

use super::*;

pub(super) const REVIEW_POSTED: &str = include_str!("../../fixtures/review_post.json");
pub(super) const REVIEWS: &str = include_str!("../../fixtures/viewer_reviews.jsonl");
pub(super) const REFUSED_OWN_APPROVE: &str =
    include_str!("../../fixtures/review_post_422_own_approve.json");
pub(super) const REFUSED_OUTSIDE_PATCH: &str =
    include_str!("../../fixtures/review_post_422_outside_patch.json");
pub(super) const REFUSED_FILE_LEVEL: &str =
    include_str!("../../fixtures/review_post_422_file_level.json");
/// GitHub's own hunks decide which lines take a comment, on each side.
#[test]
fn commentable_lines_come_from_githubs_patch() {
    let patches = parse_patches(PR_FILES).unwrap();
    assert_eq!(patches.len(), 2);
    let config = patches.iter().find(|file| file.path == "rustfmt.toml").unwrap();
    assert_eq!(config.commentable.left, [1, 2, 3, 4].into());
    assert_eq!(config.commentable.right, [1, 2, 3, 4].into());
    assert!(config.commentable.accepts(ThreadSide::Left, 1), "the removed line");
    let added = patches.iter().find(|file| file.path == "scratch-review-fixture.txt").unwrap();
    assert_eq!(added.commentable.right, (1..=8).collect());
    assert!(added.commentable.left.is_empty(), "a new file has no old side");
    assert!(!added.commentable.accepts(ThreadSide::Right, 40), "outside the patch");
}

/// Multiple hunks, a no-newline marker, and a file too large for a patch.
#[test]
fn a_patch_with_several_hunks_counts_each_from_its_header() {
    let lines = commentable_lines(
        "@@ -1,2 +1,2 @@\n-a\n+b\n c\n@@ -40,2 +40,3 @@ fn tail\n x\n+y\n z\n\\ No newline at end of file",
    );
    assert_eq!(lines.left, [1, 2, 40, 41].into());
    assert_eq!(lines.right, [1, 2, 40, 41, 42].into());
    let huge = parse_patches(
        r#"{"filename":"big.bin","previous_filename":null,"status":"modified","patch":null}"#,
    )
    .unwrap();
    assert_eq!(huge[0].commentable, CommentableLines::default(), "no patch, no line comments");
}

#[test]
fn a_posted_review_and_the_reviews_on_a_pull_request_are_read() {
    let posted = parse_posted_review(REVIEW_POSTED).unwrap();
    assert_eq!(posted.id, 5307695620);
    assert!(posted.url.ends_with("#pullrequestreview-5307695620"), "{}", posted.url);
    let reviews = parse_reviews(REVIEWS).unwrap();
    assert_eq!(reviews.len(), 1);
    assert_eq!(reviews[0].author, "nora-quinn");
    assert_eq!(reviews[0].commit_id, "6277c5f225665e1d99617a5dbe7d0a416e4c3697");
    assert!(reviews[0].body.starts_with("Fixture review"));
}

/// Each refusal GitHub really sent comes back as its own reasons.
#[test]
fn a_refused_post_says_what_github_said() {
    for (body, reason) in [
        (REFUSED_OWN_APPROVE, "Can not approve your own pull request"),
        (REFUSED_OUTSIDE_PATCH, "Line could not be resolved"),
        (REFUSED_FILE_LEVEL, "subjectType"),
    ] {
        let Some(GitHubError::Rejected { reasons }) = rejection(body) else {
            panic!("a refusal: {body}");
        };
        assert!(reasons.iter().any(|r| r.contains(reason)), "{reasons:?}");
    }
    assert!(rejection("not json").is_none());
    assert!(rejection(REVIEW_POSTED).is_none(), "a success is not a refusal");
}

/// The body GitHub accepted for the scratch review, byte for byte in meaning: the request is a
/// published shape, and a renamed key would be refused rather than ignored.
#[test]
fn the_review_request_is_the_shape_github_accepted() {
    let review = SubmitReview {
        commit_id: "6277c5f225665e1d99617a5dbe7d0a416e4c3697".into(),
        event: ReviewEvent::Comment,
        body: "Fixture review: a body and two line comments, one on each side.".into(),
        comments: vec![
            super::super::super::DraftComment {
                path: "scratch-review-fixture.txt".into(),
                line: 4,
                side: ThreadSide::Right,
                start_line: None,
                body: "A new-side line comment.".into(),
            },
            super::super::super::DraftComment {
                path: "rustfmt.toml".into(),
                line: 1,
                side: ThreadSide::Left,
                start_line: None,
                body: "An old-side line comment.".into(),
            },
            super::super::super::DraftComment {
                path: "scratch-review-fixture.txt".into(),
                line: 7,
                side: ThreadSide::Right,
                start_line: Some(5),
                body: "A multi-line comment.".into(),
            },
        ],
    };
    let accepted: serde_json::Value = serde_json::from_str(
        r#"{"commit_id":"6277c5f225665e1d99617a5dbe7d0a416e4c3697","event":"COMMENT","body":"Fixture review: a body and two line comments, one on each side.","comments":[{"path":"scratch-review-fixture.txt","line":4,"side":"RIGHT","body":"A new-side line comment."},{"path":"rustfmt.toml","line":1,"side":"LEFT","body":"An old-side line comment."},{"path":"scratch-review-fixture.txt","start_line":5,"start_side":"RIGHT","line":7,"side":"RIGHT","body":"A multi-line comment."}]}"#,
    )
    .unwrap();
    assert_eq!(review_payload(&review), accepted);
    for (event, spelled) in
        [(ReviewEvent::Approve, "APPROVE"), (ReviewEvent::RequestChanges, "REQUEST_CHANGES")]
    {
        assert_eq!(review_payload(&SubmitReview { event, ..review.clone() })["event"], spelled);
    }
}

/// Writes an executable stand-in for `gh` that answers from a script.
#[cfg(unix)]
pub(super) fn fake_gh(dir: &Path, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("gh");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}
