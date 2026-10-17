use super::*;
use crate::github::CommentableLines;
use crate::guided_review::{
    parse_unified_diff, FindingState, ReviewContextDraft, ReviewId, ReviewRevisionId, ReviewSource,
    ReviewStats, ReviewTarget,
};

fn review() -> GuidedReview {
    let diff =
        "diff --git a/src/router.rs b/src/router.rs\n--- a/src/router.rs\n+++ b/src/router.rs\n\
                    @@ -10,3 +10,4 @@ fn route\n context\n-old\n+new\n+added\n tail\n\
                    diff --git a/src/gone.rs b/src/gone.rs\n--- a/src/gone.rs\n+++ b/src/gone.rs\n\
                    @@ -5,2 +4,0 @@\n-first removed\n-second removed\n\
                    diff --git a/src/one.rs b/src/one.rs\n--- a/src/one.rs\n+++ b/src/one.rs\n\
                    @@ -3,0 +4 @@\n+only line\n";
    let files = parse_unified_diff(diff).unwrap().files;
    GuidedReview {
        id: ReviewId::new(),
        revision_id: ReviewRevisionId::from_canonical_bytes(diff.as_bytes()),
        target: ReviewTarget::PullRequest { number: 7, repository: None },
        source: ReviewSource {
            base: ReviewBase::PullRequest {
                number: 7,
                repository: "acme/app".into(),
                base_ref: "main".into(),
                head_ref: "topic".into(),
                merge_base: "b".repeat(40),
                head_oid: "a".repeat(40),
            },
            content_digest: ReviewRevisionId::from_canonical_bytes(diff.as_bytes()),
            includes_staged: false,
            includes_unstaged: false,
            included_untracked_files: 0,
            captured_at_ms: 0,
            uncovered_paths: Vec::new(),
        },
        context: ReviewContextDraft { title: "PR".into(), ..Default::default() }.build(),
        stats: ReviewStats::from_files(&files),
        chapters: crate::guided_review::fallback_chapters(&files).unwrap(),
        files,
        checks: Default::default(),
        generated_at_ms: 0,
        insight: None,
        provenance: None,
        seen: None,
        brief: None,
        conversation: None,
    }
}

/// GitHub's patch agrees with Luxe's diff unless a test says otherwise.
fn patches_matching(review: &GuidedReview) -> Vec<FilePatch> {
    review
        .files
        .iter()
        .map(|file| {
            let mut commentable = CommentableLines::default();
            for line in file.hunks.iter().flat_map(|hunk| &hunk.lines) {
                commentable.left.extend(line.old_line);
                commentable.right.extend(line.new_line);
            }
            FilePatch { path: file_path(file), previous_path: None, commentable }
        })
        .collect()
}

fn finding(anchor: FindingAnchor, severity: FindingSeverity, body: &str) -> ReviewFinding {
    ReviewFinding {
        id: FindingId::new(),
        label: String::new(),
        anchor,
        severity,
        title: body.into(),
        body: body.into(),
        proposed_fix: None,
        state: FindingState::Draft,
        thread: Vec::new(),
        posted: None,
        github_thread: None,
    }
}

fn line(review: &GuidedReview, file: usize, side: DiffSide, line: u32) -> FindingAnchor {
    let file = &review.files[file];
    FindingAnchor::Line {
        file_id: file.id.clone(),
        hunk_id: file.hunks[0].id.clone(),
        side,
        line,
        context_hash: "x".into(),
    }
}

/// Lines on either side, a hunk as a run, and the review's own words in the body.
#[test]
fn comments_land_where_github_will_take_them() {
    let review = review();
    let findings = vec![
        finding(line(&review, 0, DiffSide::New, 11), FindingSeverity::Issue, "Why new?"),
        finding(line(&review, 0, DiffSide::Old, 11), FindingSeverity::Issue, "Old was fine."),
        finding(
            FindingAnchor::Hunk {
                file_id: review.files[0].id.clone(),
                hunk_id: review.files[0].hunks[0].id.clone(),
            },
            FindingSeverity::Issue,
            "The whole hunk.",
        ),
        finding(FindingAnchor::Review, FindingSeverity::Issue, "Overall, good."),
    ];

    let plan =
        plan_post(&review, &findings, &patches_matching(&review), ReviewEvent::Comment).unwrap();

    assert_eq!(plan.review.commit_id, "a".repeat(40), "anchored to the revision read");
    assert_eq!(plan.review.comments.len(), 3);
    assert_eq!(
        (plan.review.comments[0].line, plan.review.comments[0].side),
        (11, ThreadSide::Right)
    );
    assert_eq!(
        (plan.review.comments[1].line, plan.review.comments[1].side),
        (11, ThreadSide::Left)
    );
    assert_eq!(
        (plan.review.comments[2].start_line, plan.review.comments[2].line),
        (Some(10), 13),
        "a hunk spans its new lines"
    );
    assert_eq!(plan.review.body, "Overall, good.", "the reviewer's words, and nothing else");
    assert_eq!(plan.placed.len(), 4);
}

/// A line GitHub's patch does not show goes in the body, quoted and located, instead of
/// taking the whole review down with a refusal.
#[test]
fn a_line_github_does_not_show_goes_in_the_body() {
    let review = review();
    let mut patches = patches_matching(&review);
    patches[0].commentable.right.remove(&11);
    let findings =
        vec![finding(line(&review, 0, DiffSide::New, 11), FindingSeverity::Issue, "Why new?")];

    let plan = plan_post(&review, &findings, &patches, ReviewEvent::Comment).unwrap();

    assert!(plan.review.comments.is_empty());
    assert_eq!(plan.review.body, "**`src/router.rs` 11**\n\n> `new`\n\nWhy new?");
    assert_eq!(
        plan.placed[0].place,
        Placement::Body(BodyReason::OffDiff { path: "src/router.rs".into(), lines: "11".into() })
    );
}

/// An old-side line off GitHub's diff is quoted from the old side.
#[test]
fn an_old_line_off_the_diff_is_quoted_from_the_old_side() {
    let review = review();
    let mut patches = patches_matching(&review);
    patches[0].commentable.left.remove(&11);
    let findings =
        vec![finding(line(&review, 0, DiffSide::Old, 11), FindingSeverity::Issue, "Keep this.")];
    let plan = plan_post(&review, &findings, &patches, ReviewEvent::Comment).unwrap();
    assert_eq!(plan.review.body, "**`src/router.rs` 11**\n\n> `old`\n\nKeep this.");
}

/// A one-line hunk is a line comment when GitHub shows it, and a single line number when not.
#[test]
fn a_one_line_hunk_is_a_line_or_one_line_in_the_body() {
    let review = review();
    let one_line = || {
        vec![finding(
            FindingAnchor::Hunk {
                file_id: review.files[2].id.clone(),
                hunk_id: review.files[2].hunks[0].id.clone(),
            },
            FindingSeverity::Issue,
            "Just this.",
        )]
    };
    let mut patches = patches_matching(&review);
    let plan = plan_post(&review, &one_line(), &patches, ReviewEvent::Comment).unwrap();
    assert_eq!(
        plan.placed[0].place,
        Placement::Line { path: "src/one.rs".into(), line: 4, side: DiffSide::New }
    );
    assert_eq!(plan.review.comments[0].start_line, None, "one line, not a run of one");

    patches[2].commentable.right.clear();
    let plan = plan_post(&review, &one_line(), &patches, ReviewEvent::Comment).unwrap();
    assert_eq!(
        plan.placed[0].place,
        Placement::Body(BodyReason::OffDiff { path: "src/one.rs".into(), lines: "4".into() })
    );
}

/// A gap inside a hunk's run — GitHub splitting the region — is refused by its API even when
/// both ends are fine, so the run must be whole.
#[test]
fn a_run_github_splits_goes_in_the_body() {
    let review = review();
    let mut patches = patches_matching(&review);
    patches[0].commentable.right.remove(&12);
    let findings = vec![finding(
        FindingAnchor::Hunk {
            file_id: review.files[0].id.clone(),
            hunk_id: review.files[0].hunks[0].id.clone(),
        },
        FindingSeverity::Issue,
        "The whole hunk.",
    )];
    let plan = plan_post(&review, &findings, &patches, ReviewEvent::Comment).unwrap();
    assert!(plan.review.comments.is_empty());
    assert!(
        plan.review.body.starts_with("**`src/router.rs` 10\u{2013}13**"),
        "{}",
        plan.review.body
    );
}

/// A pure deletion has no new lines, so its run counts on the old side.
#[test]
fn a_deleted_hunk_spans_its_old_lines() {
    let review = review();
    let findings = vec![finding(
        FindingAnchor::Hunk {
            file_id: review.files[1].id.clone(),
            hunk_id: review.files[1].hunks[0].id.clone(),
        },
        FindingSeverity::Issue,
        "Why delete these?",
    )];
    let plan =
        plan_post(&review, &findings, &patches_matching(&review), ReviewEvent::Comment).unwrap();
    let comment = &plan.review.comments[0];
    assert_eq!((comment.side, comment.start_line, comment.line), (ThreadSide::Left, Some(5), 6));
}

/// Questions are the agent's and a posted comment has gone; neither is sent.
#[test]
fn questions_and_posted_comments_stay_behind() {
    let review = review();
    let mut posted = finding(FindingAnchor::Review, FindingSeverity::Issue, "Already said.");
    posted.posted = Some(crate::guided_review::PostedComment { review_id: 1, url: String::new() });
    let findings = vec![
        finding(FindingAnchor::Review, FindingSeverity::Question, "Why?"),
        posted,
        finding(FindingAnchor::Review, FindingSeverity::Issue, "New thought."),
    ];
    let plan =
        plan_post(&review, &findings, &patches_matching(&review), ReviewEvent::Comment).unwrap();
    assert_eq!(plan.review.body, "New thought.");
    assert_eq!(plan.placed.len(), 1);
}

/// Chapter comments go in the body under the chapter's title, after the review's own words.
#[test]
fn a_chapter_comment_is_headed_by_its_chapter() {
    let review = review();
    let title = review.chapters[0].title.clone();
    let findings = vec![
        finding(
            FindingAnchor::Chapter { chapter_id: review.chapters[0].id.clone() },
            FindingSeverity::Issue,
            "This chapter does too much.",
        ),
        finding(FindingAnchor::Review, FindingSeverity::Issue, "Overall, fine."),
    ];
    let plan =
        plan_post(&review, &findings, &patches_matching(&review), ReviewEvent::Comment).unwrap();
    assert_eq!(
        plan.review.body,
        format!("Overall, fine.\n\n**{title}**\n\nThis chapter does too much.")
    );
}

/// What GitHub would refuse is refused here first, with a sentence it does not give.
#[test]
fn a_review_github_would_refuse_is_refused_first() {
    let review = review();
    let patches = patches_matching(&review);
    assert_eq!(
        plan_post(&review, &[], &patches, ReviewEvent::Comment),
        Err(PostPlanError::Nothing)
    );
    assert!(plan_post(&review, &[], &patches, ReviewEvent::Approve).is_ok(), "approval alone");
    let only_a_line =
        vec![finding(line(&review, 0, DiffSide::New, 11), FindingSeverity::Issue, "Hm.")];
    assert!(plan_post(&review, &only_a_line, &patches, ReviewEvent::Comment).is_ok());
    assert_eq!(
        plan_post(&review, &only_a_line, &patches, ReviewEvent::RequestChanges),
        Err(PostPlanError::NeedsBody)
    );

    let mut local = review.clone();
    local.source.base = ReviewBase::EmptyTree;
    assert_eq!(
        plan_post(&local, &only_a_line, &patches, ReviewEvent::Comment),
        Err(PostPlanError::NotAPullRequest)
    );
}
