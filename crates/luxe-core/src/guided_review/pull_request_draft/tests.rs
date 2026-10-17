use super::*;
use crate::guided_review::{
    parse_unified_diff, ReviewBase, ReviewBrief, ReviewChapter, ReviewContextDraft, ReviewId,
    ReviewRevisionId, ReviewSource, ReviewStats, ReviewTarget,
};

fn review(chapters: Vec<ReviewChapter>, intent: Option<&str>) -> GuidedReview {
    let diff = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-old\n+new\n";
    let files = parse_unified_diff(diff).unwrap().files;
    GuidedReview {
        id: ReviewId::new(),
        revision_id: ReviewRevisionId::from_canonical_bytes(diff.as_bytes()),
        target: ReviewTarget::Base { reference: "origin/main".into() },
        source: ReviewSource {
            base: ReviewBase::MergeBase { reference: "origin/main".into(), oid: "b".repeat(40) },
            content_digest: ReviewRevisionId::from_canonical_bytes(diff.as_bytes()),
            includes_staged: false,
            includes_unstaged: false,
            included_untracked_files: 0,
            captured_at_ms: 0,
            uncovered_paths: Vec::new(),
        },
        context: ReviewContextDraft {
            title: "Count the first number in the total".into(),
            ..Default::default()
        }
        .build(),
        stats: ReviewStats::from_files(&files),
        chapters,
        files,
        checks: Default::default(),
        generated_at_ms: 0,
        insight: None,
        provenance: None,
        seen: None,
        brief: intent.map(|intent| ReviewBrief { intent: intent.into(), account: Vec::new() }),
        conversation: None,
    }
}

fn chapter(title: &str, rationale: &str, impact: Option<&str>, kind: ChapterKind) -> ReviewChapter {
    ReviewChapter {
        id: crate::guided_review::ChapterId::from_items(title, &[]),
        title: title.into(),
        rationale: rationale.into(),
        impact: impact.map(str::to_string),
        kind,
        items: Vec::new(),
        notes: Vec::new(),
    }
}

/// A title too long to keep is cut at its first clause, which reads as a title; one that fits is
/// left exactly as written, lowercase convention and all.
#[test]
fn a_long_request_is_titled_by_its_first_clause() {
    assert_eq!(
        title("add a `todo overdue` command: open tasks past their due date, oldest first. with a test"),
        "Add a `todo overdue` command"
    );
    assert_eq!(title("fix: keep the cursor on the row"), "fix: keep the cursor on the row");
    let long = "x".repeat(TITLE_MAX_CHARS + 10);
    assert!(title(&long).ends_with('…'), "no clause to cut at");
    let short_head = format!("fix: {}", "word ".repeat(30));
    assert!(title(&short_head).ends_with('…'), "too few words before the colon to stand alone");
}

#[test]
fn the_draft_tells_the_reviews_story_and_closes_the_issue() {
    let draft = pull_request_draft(
        &review(
            vec![
                chapter(
                    "Sum every element",
                    "The loop started at 1.",
                    Some("total([5]) is 5 now."),
                    ChapterKind::Core,
                ),
                chapter("Cover the edge", "A test for the empty list.", None, ChapterKind::Test),
                chapter("Cargo.lock", "Regenerated.", None, ChapterKind::Generated),
            ],
            Some("Fixes the total that skipped its first number."),
        ),
        Some(&ClosedIssue { number: 123, title: Some("total() skips the first number".into()) }),
    );
    assert_eq!(draft.title, "total() skips the first number", "the issue names it");
    assert_eq!(
        draft.body,
        "Fixes the total that skipped its first number.\n\n\
             ### Sum every element\n\nThe loop started at 1.\n\ntotal([5]) is 5 now.\n\n\
             ### Cover the edge\n\nA test for the empty list.\n\n\
             Also regenerated: Cargo.lock.\n\n\
             Closes #123"
    );
}

#[test]
fn without_an_intent_an_issue_or_generated_content_it_says_only_the_chapters() {
    let draft = pull_request_draft(
        &review(vec![chapter("One thing", "Why.", Some("  "), ChapterKind::Core)], Some("  ")),
        None,
    );
    assert_eq!(draft.body, "### One thing\n\nWhy.");
}

/// Without the issue's title, or without an issue, the review's title stands.
#[test]
fn the_title_falls_back_to_the_reviews() {
    let reviewed = review(vec![chapter("One", "Why.", None, ChapterKind::Core)], None);
    for issue in [
        None,
        Some(ClosedIssue { number: 7, title: None }),
        Some(ClosedIssue { number: 7, title: Some("  ".into()) }),
    ] {
        let draft = pull_request_draft(&reviewed, issue.as_ref());
        assert_eq!(draft.title, "Count the first number in the total", "{issue:?}");
    }
    // A review nothing named is titled for its lead told chapter, not for the tool
    let mut untitled = review(
        vec![
            chapter("Cargo.lock", "Regenerated.", None, ChapterKind::Generated),
            chapter("Sum every element", "Why.", None, ChapterKind::Core),
        ],
        None,
    );
    untitled.context.title = crate::guided_review::UNTITLED_REVIEW.into();
    assert_eq!(pull_request_draft(&untitled, None).title, "Sum every element");
    untitled.chapters.retain(|chapter| chapter.kind == ChapterKind::Generated);
    assert_eq!(
        pull_request_draft(&untitled, None).title,
        crate::guided_review::UNTITLED_REVIEW,
        "with nothing told there is nothing better to say"
    );
}

/// A review of three told chapters and a lockfile is a stack of three: the lockfile rides with
/// the chapter before it, each layer says what part it is, and only the top closes the issue.
#[test]
fn a_review_drafts_as_a_stack_of_its_chapters() {
    let reviewed = review(
        vec![
            chapter("Cargo.lock", "Regenerated.", None, ChapterKind::Generated),
            chapter(
                "Sum every element",
                "The loop started at 1.",
                Some("Totals change."),
                ChapterKind::Core,
            ),
            chapter("Cargo.lock again", "Regenerated.", None, ChapterKind::Generated),
            chapter("Cover the edge", "A test for the empty list.", None, ChapterKind::Test),
            chapter("Say so", "The changelog.", None, ChapterKind::Supporting),
        ],
        None,
    );
    let issue = ClosedIssue { number: 7, title: None };
    let layers = stack_draft(&reviewed, Some(&issue)).expect("three layers");
    let titles: Vec<&str> = layers.iter().map(|layer| layer.title.as_str()).collect();
    assert_eq!(titles, ["Sum every element", "Cover the edge", "Say so"]);
    let sizes: Vec<usize> = layers.iter().map(|layer| layer.chapters.len()).collect();
    assert_eq!(
        sizes,
        [3, 1, 1],
        "a leading lockfile joins the first, a later one the one before it"
    );
    assert_eq!(
        layers[0].body,
        "Also regenerated: Cargo.lock.\n\nThe loop started at 1.\n\nTotals change.\n\n\
             Also regenerated: Cargo.lock again.\n\nPart 1 of 3."
    );
    assert_eq!(layers[1].body, "A test for the empty list.\n\nPart 2 of 3.");
    assert_eq!(layers[2].body, "The changelog.\n\nPart 3 of 3.\n\nCloses #7", "the top closes it");

    let one = review(
        vec![
            chapter("Only", "Why.", None, ChapterKind::Core),
            chapter("Cargo.lock", "Regenerated.", None, ChapterKind::Generated),
        ],
        None,
    );
    assert_eq!(stack_draft(&one, None), None, "one pull request, not a stack");
}

#[test]
fn a_long_title_is_cut_and_a_missing_one_is_named() {
    assert_eq!(title("  \nFirst line\nsecond"), "First line");
    let long = title(&"x".repeat(100));
    assert_eq!(long.chars().count(), TITLE_MAX_CHARS);
    assert!(long.ends_with('…'));
    assert_eq!(title(""), "Changes");
}
