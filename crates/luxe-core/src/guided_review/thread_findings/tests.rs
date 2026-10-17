use super::*;
use crate::github::{ReviewThread, ReviewThreads, ThreadComment, ThreadSide};
use crate::guided_review::{
    earlier_conversation, parse_unified_diff, ReviewBase, ReviewContextDraft, ReviewId,
    ReviewRevisionId, ReviewSource, ReviewStats, ReviewTarget,
};

/// The working tree after a fix: the loop line moved from 7 to 8 and was not changed.
const DIFF: &str = "diff --git a/total.py b/total.py\n--- a/total.py\n+++ b/total.py\n\
                        @@ -0,0 +1,9 @@\n+def total(numbers):\n+    \"\"\"Adds them up.\"\"\"\n\
                        +    # a comment the fix added\n+    result = 0\n+    result = 0\n\
                        +    for index in range(1, len(numbers)):\n+        result += numbers[index]\n\
                        +    return result\n+\n";

fn review(threads: Vec<ReviewThread>) -> GuidedReview {
    let files = parse_unified_diff(DIFF).unwrap().files;
    let total = threads.len();
    GuidedReview {
        id: ReviewId::new(),
        revision_id: ReviewRevisionId::from_canonical_bytes(DIFF.as_bytes()),
        target: ReviewTarget::OwnPullRequest { number: 146 },
        source: ReviewSource {
            base: ReviewBase::MergeBase { reference: "main".into(), oid: "b".repeat(40) },
            content_digest: ReviewRevisionId::from_canonical_bytes(DIFF.as_bytes()),
            includes_staged: false,
            includes_unstaged: true,
            included_untracked_files: 0,
            captured_at_ms: 0,
            uncovered_paths: Vec::new(),
        },
        context: ReviewContextDraft { title: "PR".into(), ..Default::default() }.build(),
        stats: ReviewStats::from_files(&files),
        chapters: crate::guided_review::fallback_chapters(&files).unwrap(),
        conversation: Some(earlier_conversation(
            &files,
            ReviewThreads { threads, total, ..Default::default() },
        )),
        files,
        checks: Default::default(),
        generated_at_ms: 0,
        insight: None,
        provenance: None,
        seen: None,
        brief: None,
    }
}

fn thread(id: &str, line: Option<u32>, text: Option<&str>) -> ReviewThread {
    ReviewThread {
        id: id.into(),
        path: "total.py".into(),
        line,
        start_line: None,
        side: ThreadSide::Right,
        outdated: false,
        resolved: false,
        file_level: false,
        comments: vec![ThreadComment {
            author: "mara".into(),
            body: "This starts at 1, so the first number is never added.".into(),
            created_at: String::new(),
            url: format!("https://github.com/acme/app/pull/146#discussion_{id}"),
        }],
        total_comments: 1,
        anchor_text: text.map(str::to_string),
        can_reply: true,
        can_resolve: true,
    }
}

fn line_of(finding: &ReviewFinding) -> Option<u32> {
    match &finding.anchor {
        FindingAnchor::Line { line, .. } => Some(*line),
        _ => None,
    }
}

const LOOP: &str = "    for index in range(1, len(numbers)):";

#[test]
fn a_thread_lands_on_its_line_when_the_text_there_is_the_same() {
    let findings = thread_findings(&review(vec![thread("t1", Some(6), Some(LOOP))]));
    assert_eq!(findings.len(), 1);
    assert_eq!(line_of(&findings[0]), Some(6));
    let origin = findings[0].github_thread.as_ref().unwrap();
    assert_eq!((origin.id.as_str(), origin.author.as_str()), ("t1", "mara"));
    assert!(origin.url.ends_with("#discussion_t1") && origin.can_reply && origin.can_resolve);
    assert_eq!(findings[0].state, FindingState::Draft);
    assert_eq!(findings[0].severity, FindingSeverity::Issue);
    assert!(findings[0].title.starts_with("mara: This starts at 1"));
    assert!(findings[0].body.starts_with("mara: This starts at 1"), "{}", findings[0].body);
    assert!(!findings[0].body.contains("more on GitHub"), "{}", findings[0].body);
}

/// The working tree moved: the number now points at another line, so the text is followed to
/// the one place it still appears.
#[test]
fn a_moved_line_is_found_by_its_text_when_it_appears_once() {
    let findings = thread_findings(&review(vec![thread("t1", Some(3), Some(LOOP))]));
    assert_eq!(line_of(&findings[0]), Some(6));
    // And an outdated thread's number is not trusted at all, only its text
    let mut outdated = thread("t2", Some(1), Some(LOOP));
    outdated.outdated = true;
    assert_eq!(line_of(&thread_findings(&review(vec![outdated]))[0]), Some(6));
}

/// Text that appears twice, or not at all, proves nothing: the thread sits on the review and
/// says where it was written.
#[test]
fn an_ambiguous_or_missing_line_sits_on_the_review_and_says_where_it_was() {
    for (line, text) in [
        (Some(1), Some("    result = 0")),
        (Some(6), Some("    for i in range(len(n)):")),
        (Some(6), None),
    ] {
        let findings = thread_findings(&review(vec![thread("t", line, text)]));
        assert_eq!(findings[0].anchor, FindingAnchor::Review, "{text:?}");
        assert!(findings[0].body.starts_with("Written on `total.py`"), "{}", findings[0].body);
    }
    // at its own number, where the text is one of the two copies, it still lands
    let exact = thread_findings(&review(vec![thread("t", Some(5), Some("    result = 0"))]));
    assert_eq!(line_of(&exact[0]), Some(5));
    // but not when outdated: its number belongs to a revision that is gone
    let mut outdated = thread("t", Some(5), Some("    result = 0"));
    outdated.outdated = true;
    assert_eq!(thread_findings(&review(vec![outdated]))[0].anchor, FindingAnchor::Review);
}

#[test]
fn a_file_level_thread_or_one_on_another_file_sits_on_the_review() {
    let mut file_level = thread("t1", None, None);
    file_level.file_level = true;
    let mut elsewhere = thread("t2", Some(6), Some(LOOP));
    elsewhere.path = "other.py".into();
    let findings = thread_findings(&review(vec![file_level, elsewhere]));
    assert!(findings.iter().all(|finding| finding.anchor == FindingAnchor::Review));
    assert!(findings[0].body.starts_with("Written on `total.py` as a whole."));
    assert!(findings[1].body.starts_with("Written on `other.py` line 6, at `for index"));

    // A file-level thread is about the file even when GitHub gives it a line
    let mut numbered = thread("t3", Some(3), Some(LOOP));
    numbered.file_level = true;
    let body = &thread_findings(&review(vec![numbered]))[0].body;
    assert!(body.starts_with("Written on `total.py` as a whole."), "{body}");
    // and a line with no text to prove it by says so, whitespace counting as none
    for text in [None, Some("   ")] {
        let body = &thread_findings(&review(vec![thread("t4", Some(40), text)]))[0].body;
        assert!(
            body.starts_with("Written on `total.py` line 40, which this review cannot match"),
            "{body}"
        );
    }
}

/// A resolved thread has nothing left to do, and a thread without an id could never be
/// answered, so neither becomes a finding.
#[test]
fn resolved_threads_and_threads_without_an_id_stay_in_the_conversation() {
    let mut resolved = thread("t1", Some(6), Some(LOOP));
    resolved.resolved = true;
    let nameless = thread("", Some(6), Some(LOOP));
    assert!(thread_findings(&review(vec![resolved, nameless])).is_empty());
    let mut none = review(Vec::new());
    none.conversation = None;
    assert!(thread_findings(&none).is_empty());
}

#[test]
fn the_whole_conversation_is_the_body_and_a_long_one_says_what_it_left() {
    let mut long = thread("t1", Some(6), Some(LOOP));
    long.comments.push(ThreadComment {
        author: String::new(),
        body: "Agreed.".into(),
        created_at: String::new(),
        url: String::new(),
    });
    long.total_comments = 9;
    let body = &thread_findings(&review(vec![long]))[0].body;
    assert!(body.contains("\n\na deleted account: Agreed."), "{body}");
    assert!(body.ends_with("… 7 more on GitHub"), "{body}");
}

#[test]
fn a_long_opening_is_cut_into_a_title() {
    assert_eq!(title("mara", "\n  Short.\nSecond line"), "mara: Short.");
    let long = title("mara", &"x".repeat(200));
    assert_eq!(long.chars().count(), MAX_TITLE_CHARS);
    assert!(long.ends_with('…'));
    assert_eq!(title("", ""), "a deleted account: A comment");
}

#[test]
fn an_old_side_thread_counts_on_the_old_side() {
    let deletion =
        "diff --git a/a.py b/a.py\n--- a/a.py\n+++ b/a.py\n@@ -3,2 +3,1 @@\n keep\n-gone\n";
    let files = parse_unified_diff(deletion).unwrap().files;
    let mut old = thread("t1", Some(4), Some("gone"));
    old.path = "a.py".into();
    old.side = ThreadSide::Left;
    let anchored = place(
        &files,
        &earlier_conversation(
            &files,
            ReviewThreads { threads: vec![old], total: 1, ..Default::default() },
        )
        .threads()[0],
    );
    assert!(matches!(anchored, Some(FindingAnchor::Line { line: 4, side: DiffSide::Old, .. })));
    // the same text asked for on the new side is not there
    let line = files[0].hunks[0].lines.iter().find(|line| line.text == "gone").unwrap();
    assert_eq!(number_on(line, DiffSide::New), None);
    let kept = files[0].hunks[0].lines.iter().find(|line| line.text == "keep").unwrap();
    assert_eq!(
        (number_on(kept, DiffSide::Old), number_on(kept, DiffSide::New)),
        (Some(3), Some(3))
    );
}

/// What was said in the conversation is feedback on the whole review, answered by a new
/// comment and never resolved; one without an id could not be answered, so it does not come.
#[test]
fn the_conversation_becomes_feedback_on_the_whole_review() {
    let mut review = review(Vec::new());
    let said = |id: &str, author: &str| crate::github::ConversationComment {
        id: id.into(),
        author: author.into(),
        bot: false,
        mine: false,
        body: "Could the changelog mention the 422?\nIt is user-facing.".into(),
        created_at: String::new(),
        url: format!("https://github.com/o/r/pull/146#{id}"),
    };
    review.conversation = Some(earlier_conversation(
        &review.files,
        ReviewThreads {
            conversation: vec![said("IC_1", "jon"), said("", "jon"), said("IC_2", "")],
            ..Default::default()
        },
    ));
    let findings = thread_findings(&review);
    assert_eq!(findings.len(), 2, "{findings:?}");
    let jon = &findings[0];
    assert_eq!(jon.anchor, FindingAnchor::Review);
    assert_eq!(jon.state, FindingState::Draft);
    assert_eq!(jon.title, "jon: Could the changelog mention the 422?");
    assert_eq!(jon.body, "jon: Could the changelog mention the 422?\nIt is user-facing.");
    let thread = jon.github_thread.as_ref().unwrap();
    assert_eq!((thread.id.as_str(), thread.author.as_str()), ("IC_1", "jon"));
    assert_eq!(thread.url, "https://github.com/o/r/pull/146#IC_1");
    assert!(thread.conversation && thread.can_reply && !thread.can_resolve);
    assert!(findings[1].body.starts_with("a deleted account: "), "{}", findings[1].body);
    assert!(jon.validate().is_ok());
}
